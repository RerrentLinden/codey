use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use codey_runtime_core::settings::RelayProtocol;
use directories::BaseDirs;
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use serde_json::Value;
use toml_edit::{DocumentMut, Item, TableLike};

use crate::config::{
    CodeyConfig, DEFAULT_SUBAGENT_MODEL, DEFAULT_SUBAGENT_REASONING_EFFORT, ProviderProfile,
};
use crate::model_catalog;
use crate::sqlite_util::table_columns;

const APP_TYPE: &str = "codex";
const LOCAL_OFFICIAL_PROVIDER_ID: &str = "local-official";
const PROXY_MANAGED_TOKEN: &str = "PROXY_MANAGED";
const PROXY_OFFICIAL_PROVIDER_ID: &str = "cc-switch-official";
const CC_SWITCH_APP_ID: &str = "com.ccswitch.desktop";
const CC_SWITCH_PATH_STORE: &str = "app_paths.json";
const CC_SWITCH_CONFIG_DIR_KEY: &str = "app_config_dir_override";
const CC_SWITCH_DB_FILE: &str = "cc-switch.db";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CurrentProvider {
    pub id: String,
    pub name: String,
    pub official: bool,
    pub supports_remote_compaction: bool,
    pub base_url: String,
    pub protocol: RelayProtocol,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchStatus {
    pub available: bool,
    pub path: String,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub provider: CurrentProvider,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteTakeoverState {
    pub managed: bool,
    pub live: bool,
}

pub fn default_db_path() -> PathBuf {
    let explicit_db_path = std::env::var_os("CC_SWITCH_DB_PATH").map(PathBuf::from);
    let Some(dirs) = BaseDirs::new() else {
        return explicit_db_path
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from(".cc-switch/cc-switch.db"));
    };

    #[cfg(windows)]
    let legacy_home = std::env::var_os("HOME").map(PathBuf::from);
    #[cfg(not(windows))]
    let legacy_home: Option<PathBuf> = None;

    default_db_path_from(
        explicit_db_path,
        dirs.home_dir(),
        dirs.data_dir(),
        legacy_home.as_deref(),
    )
}

fn default_db_path_from(
    explicit_db_path: Option<PathBuf>,
    home_dir: &Path,
    data_dir: &Path,
    legacy_home: Option<&Path>,
) -> PathBuf {
    if let Some(path) = explicit_db_path.filter(|path| !path.as_os_str().is_empty()) {
        return path;
    }

    if let Some(config_dir) = cc_switch_config_dir_override(data_dir, home_dir) {
        return config_dir.join(CC_SWITCH_DB_FILE);
    }

    let default_db = home_dir.join(".cc-switch").join(CC_SWITCH_DB_FILE);
    legacy_cc_switch_db_path(&default_db, legacy_home).unwrap_or(default_db)
}

fn cc_switch_config_dir_override(data_dir: &Path, home_dir: &Path) -> Option<PathBuf> {
    let store_path = data_dir.join(CC_SWITCH_APP_ID).join(CC_SWITCH_PATH_STORE);
    let store = fs::read(store_path).ok()?;
    let document: Value = serde_json::from_slice(&store).ok()?;
    let raw_path = document
        .get(CC_SWITCH_CONFIG_DIR_KEY)
        .and_then(Value::as_str)?
        .trim();
    if raw_path.is_empty() {
        return None;
    }

    let path = resolve_cc_switch_store_path(raw_path, home_dir);
    path.exists().then_some(path)
}

fn resolve_cc_switch_store_path(raw_path: &str, home_dir: &Path) -> PathBuf {
    if raw_path == "~" {
        return home_dir.to_path_buf();
    }
    if let Some(path) = raw_path
        .strip_prefix("~/")
        .or_else(|| raw_path.strip_prefix("~\\"))
    {
        return home_dir.join(path);
    }
    PathBuf::from(raw_path)
}

fn legacy_cc_switch_db_path(default_db: &Path, legacy_home: Option<&Path>) -> Option<PathBuf> {
    if default_db.is_file() {
        return None;
    }
    let legacy_db = legacy_home?.join(".cc-switch").join(CC_SWITCH_DB_FILE);
    legacy_db.is_file().then_some(legacy_db)
}

pub fn route_takeover_state(codex_home: &Path) -> Result<RouteTakeoverState> {
    route_takeover_state_from_paths(&default_db_path(), codex_home)
}

fn route_takeover_state_from_paths(
    db_path: &Path,
    codex_home: &Path,
) -> Result<RouteTakeoverState> {
    let managed = if db_path.is_file() {
        read_route_takeover_managed(db_path)?
    } else {
        false
    };
    let live = live_config_uses_proxy_route(codex_home)?;
    Ok(RouteTakeoverState { managed, live })
}

fn live_auth_uses_proxy_route(codex_home: &Path) -> Result<bool> {
    let auth_path = codex_home.join("auth.json");
    let auth = match fs::read(&auth_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 Codex Live 认证失败：{}", auth_path.display()));
        }
    };
    let document = serde_json::from_slice::<Value>(&auth)
        .with_context(|| format!("解析 Codex Live 认证失败：{}", auth_path.display()))?;
    Ok(document
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .is_some_and(|token| token.trim() == PROXY_MANAGED_TOKEN))
}

fn read_route_takeover_managed(path: &Path) -> Result<bool> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("打开 cc-switch 数据库失败：{}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(2))?;

    let proxy_columns = table_columns(&connection, "proxy_config")?;
    let proxy_enabled = if proxy_columns.contains("enabled") {
        let enabled = if proxy_columns.contains("app_type") {
            connection.query_row(
                "SELECT COALESCE(MAX(enabled), 0) FROM proxy_config WHERE app_type=?1",
                params![APP_TYPE],
                |row| row.get::<_, i64>(0),
            )?
        } else {
            connection.query_row(
                "SELECT COALESCE(MAX(enabled), 0) FROM proxy_config",
                [],
                |row| row.get::<_, i64>(0),
            )?
        };
        enabled != 0
    } else if proxy_columns.contains("live_takeover_active") {
        connection.query_row(
            "SELECT COALESCE(MAX(live_takeover_active), 0) FROM proxy_config",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0
    } else {
        false
    };

    let backup_columns = table_columns(&connection, "proxy_live_backup")?;
    let has_live_backup = if backup_columns.contains("app_type") {
        connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM proxy_live_backup WHERE app_type=?1
             )",
            params![APP_TYPE],
            |row| row.get::<_, i64>(0),
        )? != 0
    } else if backup_columns.is_empty() {
        false
    } else {
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM proxy_live_backup)",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0
    };

    let settings_columns = table_columns(&connection, "settings")?;
    let legacy_enabled = if settings_columns.contains("key") && settings_columns.contains("value") {
        connection.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM settings
                WHERE key='proxy_takeover_codex'
                  AND lower(trim(value)) IN ('true', '1')
             )",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0
    } else {
        false
    };

    Ok(proxy_enabled || has_live_backup || legacy_enabled)
}

fn live_config_uses_proxy_route(codex_home: &Path) -> Result<bool> {
    // CC Switch keeps the placeholder in auth.json unless its optional
    // "preserve Codex official auth" mode is enabled. In that default mode
    // config.toml can point at a third-party loopback provider without carrying
    // its own experimental_bearer_token, so auth.json is the ownership marker.
    if live_auth_uses_proxy_route(codex_home)? {
        return Ok(true);
    }
    let config_path = codex_home.join("config.toml");
    let config = match fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 Codex Live 配置失败：{}", config_path.display()));
        }
    };
    let document = DocumentMut::from_str(&config)
        .with_context(|| format!("解析 Codex Live 配置失败：{}", config_path.display()))?;
    let provider_id = document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    let Some(provider_id) = provider_id else {
        return Ok(false);
    };
    let provider = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table_like);
    let managed_token = provider
        .and_then(|provider| provider.get("experimental_bearer_token"))
        .and_then(Item::as_str)
        .or_else(|| {
            document
                .get("experimental_bearer_token")
                .and_then(Item::as_str)
        })
        .is_some_and(|token| token.trim() == PROXY_MANAGED_TOKEN);
    let official_loopback = provider_id == PROXY_OFFICIAL_PROVIDER_ID
        && provider
            .and_then(|provider| provider.get("base_url"))
            .and_then(Item::as_str)
            .is_some_and(is_loopback_url);
    Ok(managed_token || official_loopback)
}

fn is_loopback_url(url: &str) -> bool {
    let authority_and_path = url
        .trim()
        .split_once("://")
        .map_or(url.trim(), |(_, rest)| rest);
    let authority = authority_and_path
        .split('/')
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

pub fn sync_current_provider(
    config: &CodeyConfig,
    codex_home: &Path,
) -> Result<(CodeyConfig, CcSwitchStatus)> {
    let (provider, api_key) = local_provider(codex_home)?;
    let profile = profile_from_provider(&provider, api_key);

    let previous_provider_id = config.current_provider_id().map(ToString::to_string);
    let mut next = config.clone();
    next.active_profile_id = profile.id.clone();
    next.profiles = vec![profile];
    if previous_provider_id.as_deref() != Some(provider.id.as_str()) {
        reset_subagent_defaults_for_current_provider(&mut next, codex_home, provider.official);
    }
    next = next.normalize();
    let changed = &next != config;
    let status = CcSwitchStatus {
        available: false,
        path: codex_home.join("config.toml").to_string_lossy().to_string(),
        changed,
        message: Some("已直接读取本地 Codex 登录与 API 配置".to_string()),
        provider,
    };
    Ok((next, status))
}

fn reset_subagent_defaults_for_current_provider(
    config: &mut CodeyConfig,
    codex_home: &Path,
    official_provider: bool,
) {
    let (model, reasoning_effort) =
        subagent_defaults_for_current_provider(config, codex_home, official_provider);
    config.subagent_model = model;
    config.subagent_reasoning_effort = reasoning_effort;
}

fn subagent_defaults_for_current_provider(
    config: &CodeyConfig,
    codex_home: &Path,
    official_provider: bool,
) -> (String, String) {
    let Ok(state) = model_catalog::selection_state_with_manual_models(
        codex_home,
        official_provider,
        config.upstream_models_snapshot(),
        config.selected_models(),
        config.manual_third_party_models(),
        Some(DEFAULT_SUBAGENT_MODEL),
    ) else {
        return (
            DEFAULT_SUBAGENT_MODEL.to_string(),
            DEFAULT_SUBAGENT_REASONING_EFFORT.to_string(),
        );
    };
    let model = if subagent_model_available(&state, DEFAULT_SUBAGENT_MODEL) {
        DEFAULT_SUBAGENT_MODEL.to_string()
    } else if !state.default_model.trim().is_empty() {
        state.default_model.clone()
    } else {
        DEFAULT_SUBAGENT_MODEL.to_string()
    };
    let reasoning_effort = subagent_reasoning_effort_for_model(&state, &model);
    (model, reasoning_effort)
}

fn subagent_model_available(state: &model_catalog::ModelSelectionState, model: &str) -> bool {
    state
        .official_models
        .iter()
        .any(|candidate| candidate.supported && candidate.slug == model)
        || state
            .third_party_models
            .iter()
            .any(|candidate| candidate == model)
}

fn subagent_reasoning_effort_for_model(
    state: &model_catalog::ModelSelectionState,
    model: &str,
) -> String {
    if let Some(official_model) = state
        .official_models
        .iter()
        .find(|candidate| candidate.supported && candidate.slug == model)
    {
        if official_model
            .supported_reasoning_efforts
            .iter()
            .any(|effort| effort == DEFAULT_SUBAGENT_REASONING_EFFORT)
        {
            return DEFAULT_SUBAGENT_REASONING_EFFORT.to_string();
        }
        if !official_model.default_reasoning_effort.trim().is_empty() {
            return official_model.default_reasoning_effort.clone();
        }
    }
    DEFAULT_SUBAGENT_REASONING_EFFORT.to_string()
}

pub fn status_from_config(config: &CodeyConfig) -> CcSwitchStatus {
    let profile = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id)
        .or_else(|| config.profiles.first());
    let provider = profile
        .map(|profile| CurrentProvider {
            id: profile.id.clone(),
            name: profile.name.clone(),
            official: profile.cc_switch_read_only,
            supports_remote_compaction: profile.supports_remote_compaction,
            base_url: profile.base_url.clone(),
            protocol: profile.protocol,
            source: "local".to_string(),
        })
        .unwrap_or_else(|| CurrentProvider {
            id: LOCAL_OFFICIAL_PROVIDER_ID.to_string(),
            name: "OpenAI 官方直登".to_string(),
            official: true,
            supports_remote_compaction: true,
            base_url: String::new(),
            protocol: RelayProtocol::Responses,
            source: "local".to_string(),
        });
    CcSwitchStatus {
        available: false,
        path: crate::codex_config::codex_home()
            .join("config.toml")
            .to_string_lossy()
            .to_string(),
        changed: false,
        message: Some("当前使用本地 Codex 登录与 API 配置".to_string()),
        provider,
    }
}

fn profile_from_provider(provider: &CurrentProvider, api_key: String) -> ProviderProfile {
    ProviderProfile {
        id: provider.id.clone(),
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        api_key,
        protocol: provider.protocol,
        cc_switch_provider_id: None,
        cc_switch_read_only: provider.official,
        supports_remote_compaction: provider.supports_remote_compaction,
    }
}

fn local_provider(codex_home: &Path) -> Result<(CurrentProvider, String)> {
    let config_path = codex_home.join("config.toml");
    let config = match fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取本地 Codex 配置失败：{}", config_path.display()));
        }
    };
    let document = DocumentMut::from_str(&config).unwrap_or_default();
    let provider_id = document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(LOCAL_OFFICIAL_PROVIDER_ID);
    let table = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table_like);
    let mut base_url = table
        .and_then(|provider| provider.get("base_url"))
        .and_then(Item::as_str)
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_string();
    let name = table
        .and_then(|provider| provider.get("name"))
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(provider_id);
    let wire_api = table
        .and_then(|provider| provider.get("wire_api"))
        .and_then(Item::as_str)
        .unwrap_or("responses");
    let auth = fs::read(codex_home.join("auth.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let auth_mode = auth
        .as_ref()
        .and_then(|auth| auth.get("auth_mode"))
        .and_then(Value::as_str);
    let auth_api_key = auth
        .as_ref()
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let config_api_key = provider_config_api_key(&document, table);
    // The provider-scoped token describes the active route and must win over
    // any long-lived auth.json login retained alongside it.
    let api_key = config_api_key
        .or_else(|| auth_api_key.map(ToString::to_string))
        .unwrap_or_default();
    let official_endpoint = base_url.is_empty() || is_official_base_url(&base_url);
    let official = official_endpoint && (auth_mode == Some("chatgpt") || api_key.is_empty());
    if !official && base_url.is_empty() {
        base_url = "https://api.openai.com/v1".to_string();
    }
    let provider = CurrentProvider {
        id: if official && provider_id == LOCAL_OFFICIAL_PROVIDER_ID {
            LOCAL_OFFICIAL_PROVIDER_ID.to_string()
        } else {
            provider_id.to_string()
        },
        name: if official {
            "OpenAI 官方直登".to_string()
        } else if name == LOCAL_OFFICIAL_PROVIDER_ID {
            "OpenAI API".to_string()
        } else {
            name.to_string()
        },
        official,
        supports_remote_compaction: official || name == "OpenAI",
        base_url,
        protocol: protocol_from_wire_api(wire_api),
        source: "local".to_string(),
    };
    Ok((provider, if official { String::new() } else { api_key }))
}

fn provider_config_api_key(
    document: &DocumentMut,
    provider: Option<&dyn TableLike>,
) -> Option<String> {
    const PROVIDER_KEYS: &[&str] = &[
        "experimental_bearer_token",
        "api_key",
        "apikey",
        "bearer_token",
        "token",
    ];
    PROVIDER_KEYS
        .iter()
        .find_map(|key| {
            provider
                .and_then(|provider| provider.get(key))
                .and_then(Item::as_str)
        })
        .or_else(|| {
            document
                .get("experimental_bearer_token")
                .and_then(Item::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn is_official_base_url(base_url: &str) -> bool {
    let base_url = base_url.to_ascii_lowercase();
    base_url.contains("chatgpt.com/backend-api/codex") || base_url.contains("api.openai.com")
}

fn protocol_from_wire_api(value: &str) -> RelayProtocol {
    if value.to_ascii_lowercase().contains("chat") {
        RelayProtocol::ChatCompletions
    } else {
        RelayProtocol::Responses
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cc-switch.db");
        let home = directory.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE providers (
                    id TEXT NOT NULL,
                    app_type TEXT NOT NULL,
                    name TEXT NOT NULL,
                    settings_config TEXT NOT NULL,
                    category TEXT,
                    created_at INTEGER,
                    sort_index INTEGER,
                    is_current BOOLEAN NOT NULL DEFAULT 0,
                    PRIMARY KEY (id, app_type)
                );",
            )
            .unwrap();
        (directory, path, home)
    }

    #[test]
    fn custom_cc_switch_data_directory_is_read_from_tauri_store() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let data = directory.path().join("data");
        let custom = directory.path().join("synced").join(".cc-switch");
        fs::create_dir_all(data.join(CC_SWITCH_APP_ID)).unwrap();
        fs::create_dir_all(&custom).unwrap();
        fs::write(
            data.join(CC_SWITCH_APP_ID).join(CC_SWITCH_PATH_STORE),
            json!({CC_SWITCH_CONFIG_DIR_KEY: custom}).to_string(),
        )
        .unwrap();

        let path = default_db_path_from(None, &home, &data, None);

        assert_eq!(path, custom.join(CC_SWITCH_DB_FILE));
    }

    #[test]
    fn explicit_cc_switch_db_path_wins_over_store_override() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let data = directory.path().join("data");
        let custom = directory.path().join("custom");
        let explicit = directory.path().join("explicit.db");
        fs::create_dir_all(data.join(CC_SWITCH_APP_ID)).unwrap();
        fs::create_dir_all(&custom).unwrap();
        fs::write(
            data.join(CC_SWITCH_APP_ID).join(CC_SWITCH_PATH_STORE),
            json!({CC_SWITCH_CONFIG_DIR_KEY: custom}).to_string(),
        )
        .unwrap();

        let path = default_db_path_from(Some(explicit.clone()), &home, &data, None);

        assert_eq!(path, explicit);
    }

    #[test]
    fn legacy_home_database_is_used_only_when_default_database_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let data = directory.path().join("data");
        let legacy_home = directory.path().join("legacy-home");
        let legacy_db = legacy_home.join(".cc-switch").join(CC_SWITCH_DB_FILE);
        fs::create_dir_all(legacy_db.parent().unwrap()).unwrap();
        fs::write(&legacy_db, b"legacy").unwrap();

        assert_eq!(
            default_db_path_from(None, &home, &data, Some(&legacy_home)),
            legacy_db
        );

        let default_db = home.join(".cc-switch").join(CC_SWITCH_DB_FILE);
        fs::create_dir_all(default_db.parent().unwrap()).unwrap();
        fs::write(&default_db, b"default").unwrap();
        assert_eq!(
            default_db_path_from(None, &home, &data, Some(&legacy_home)),
            default_db
        );
    }

    fn install_proxy_schema(path: &Path) {
        Connection::open(path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE proxy_config (
                    app_type TEXT PRIMARY KEY,
                    enabled INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE proxy_live_backup (
                    app_type TEXT PRIMARY KEY,
                    original_config TEXT NOT NULL,
                    backed_up_at TEXT NOT NULL
                );
                CREATE TABLE settings (
                    key TEXT PRIMARY KEY,
                    value TEXT
                );",
            )
            .unwrap();
    }

    fn write_live_route(home: &Path, provider_id: &str, base_url: &str, token: &str) {
        fs::write(
            home.join("config.toml"),
            format!(
                "model_provider = \"{provider_id}\"\n\n\
                 [model_providers.{provider_id}]\n\
                 base_url = \"{base_url}\"\n\
                 experimental_bearer_token = \"{token}\"\n"
            ),
        )
        .unwrap();
    }

    fn insert_provider(path: &Path, id: &str, name: &str, url: &str, current: bool) {
        let settings = json!({
            "auth": {"OPENAI_API_KEY": format!("{id}-secret")},
            "config": format!(
                "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"custom\"\nbase_url = \"{url}\"\nwire_api = \"responses\"\n"
            )
        });
        Connection::open(path)
            .unwrap()
            .execute(
                "INSERT INTO providers
                 (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES (?1, 'codex', ?2, ?3, 0, ?4)",
                params![id, name, settings.to_string(), current],
            )
            .unwrap();
    }

    fn saved_route_profile(id: &str) -> ProviderProfile {
        ProviderProfile {
            id: id.to_string(),
            name: format!("线路 {id}"),
            base_url: format!("https://{id}.example/v1"),
            api_key: format!("{id}-secret"),
            protocol: RelayProtocol::Responses,
            cc_switch_provider_id: Some(id.to_string()),
            cc_switch_read_only: false,
            supports_remote_compaction: false,
        }
    }

    #[test]
    fn codex_config_wins_when_cc_switch_database_has_a_current_provider() {
        let (_directory, path, home) = fixture();
        insert_provider(
            &path,
            "cc-switch-route",
            "CC Switch 线路",
            "https://cc-switch.example/v1",
            true,
        );
        fs::write(
            home.join("config.toml"),
            r#"
model_provider = "codex-local"

[model_providers.codex-local]
name = "Codex Local"
base_url = "https://codex-local.example/v1"
wire_api = "responses"
experimental_bearer_token = "sk-codex-local"
"#,
        )
        .unwrap();

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(!status.available);
        assert_eq!(status.provider.id, "codex-local");
        assert_eq!(status.provider.source, "local");
        assert_eq!(
            status.path,
            home.join("config.toml").to_string_lossy().to_string()
        );
        assert_eq!(synced.profiles.len(), 1);
        assert_eq!(
            synced.profiles[0].base_url,
            "https://codex-local.example/v1"
        );
        assert_eq!(synced.profiles[0].api_key, "sk-codex-local");
        assert!(synced.profiles[0].cc_switch_provider_id.is_none());
    }

    #[test]
    fn official_tokens_are_never_copied_into_a_provider_profile() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("auth.json"),
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"secret"}}"#,
        )
        .unwrap();

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(status.provider.official);
        assert!(synced.profiles[0].api_key.is_empty());
    }

    #[test]
    fn preserved_chatgpt_login_does_not_replace_the_codex_api_route() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            r#"
model_provider = "custom"

[model_providers.custom]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "sk-relay"
"#,
        )
        .unwrap();
        let auth = br#"{"auth_mode":"chatgpt","tokens":{"access_token":"free-account-token"}}"#;
        fs::write(home.join("auth.json"), auth).unwrap();

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(!status.provider.official);
        assert_eq!(status.provider.base_url, "https://relay.example/v1");
        assert_eq!(synced.profiles[0].api_key, "sk-relay");
        assert!(!synced.profiles[0].cc_switch_read_only);
        let patched = crate::codex_config::patch_config(
            "model_provider = \"custom\"\n",
            &synced.profiles[0],
            "custom",
            false,
        )
        .unwrap();
        assert!(patched.contains("base_url = \"https://relay.example/v1\""));
        assert!(patched.contains("experimental_bearer_token = \"sk-relay\""));
        assert!(!patched.contains("base_url = \"https://chatgpt.com/backend-api/codex\""));
        assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth);
    }

    #[test]
    fn remote_compaction_identity_survives_local_config_read_and_runtime_patch() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            r#"
model_provider = "custom"

[model_providers.custom]
name = "OpenAI"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "sk-relay"
"#,
        )
        .unwrap();

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(!status.provider.official);
        assert!(status.provider.supports_remote_compaction);
        assert!(synced.profiles[0].supports_remote_compaction);

        let patched = crate::codex_config::patch_config(
            "model_provider = \"custom\"\n",
            &synced.profiles[0],
            "custom",
            false,
        )
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
        assert_eq!(
            patched["model_providers"]["custom"]["name"].as_str(),
            Some("OpenAI")
        );
        assert_eq!(
            patched["model_providers"]["custom"]["base_url"].as_str(),
            Some("https://relay.example/v1")
        );
    }

    #[test]
    fn reads_local_official_login() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("auth.json"),
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"secret"}}"#,
        )
        .unwrap();

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(!status.available);
        assert!(status.provider.official);
        assert_eq!(status.provider.source, "local");
        assert!(synced.profiles[0].api_key.is_empty());
    }

    #[test]
    fn saved_cc_switch_metadata_is_not_reported_as_the_config_source() {
        let config = CodeyConfig {
            active_profile_id: "route-a".into(),
            profiles: vec![saved_route_profile("route-a")],
            ..CodeyConfig::default()
        };
        let status = status_from_config(&config);

        assert!(!status.available);
        assert_eq!(status.provider.source, "local");
        assert_eq!(status.provider.id, "route-a");
    }

    #[test]
    fn local_api_route_uses_provider_token_while_preserving_chatgpt_login() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            r#"
model_provider = "custom"

[model_providers.custom]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "sk-provider"
"#,
        )
        .unwrap();
        let auth = br#"{"auth_mode":"chatgpt","tokens":{"access_token":"free-account-token"}}"#;
        fs::write(home.join("auth.json"), auth).unwrap();

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(!status.available);
        assert!(!status.provider.official);
        assert_eq!(status.provider.base_url, "https://relay.example/v1");
        assert_eq!(synced.profiles[0].api_key, "sk-provider");
        let patched = crate::codex_config::patch_config(
            "model_provider = \"custom\"\n",
            &synced.profiles[0],
            "custom",
            false,
        )
        .unwrap();
        assert!(patched.contains("base_url = \"https://relay.example/v1\""));
        assert!(patched.contains("experimental_bearer_token = \"sk-provider\""));
        assert!(!patched.contains("base_url = \"https://chatgpt.com/backend-api/codex\""));
        assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth);
    }

    #[test]
    fn manual_api_route_reads_auth_json_api_key() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("config.toml"),
            r#"
model_provider = "manual"

[model_providers.manual]
name = "Manual Relay"
base_url = "https://manual.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#,
        )
        .unwrap();
        let auth = br#"{"OPENAI_API_KEY":"sk-manual"}"#;
        fs::write(home.join("auth.json"), auth).unwrap();

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(!status.available);
        assert!(!status.provider.official);
        assert_eq!(status.provider.source, "local");
        assert_eq!(status.provider.base_url, "https://manual.example/v1");
        assert_eq!(synced.profiles[0].api_key, "sk-manual");
        let patched = crate::codex_config::patch_config(
            "model_provider = \"manual\"\n",
            &synced.profiles[0],
            "manual",
            false,
        )
        .unwrap();
        assert!(patched.contains("base_url = \"https://manual.example/v1\""));
        assert!(patched.contains("experimental_bearer_token = \"sk-manual\""));
        assert!(!patched.contains("base_url = \"https://chatgpt.com/backend-api/codex\""));
        assert_eq!(fs::read(home.join("auth.json")).unwrap(), auth);
    }

    #[test]
    fn model_selections_survive_provider_synchronization() {
        let (_directory, _path, home) = fixture();
        write_live_route(
            &home,
            "route-a",
            "https://route-a.example/v1",
            "route-a-secret",
        );
        let mut config = CodeyConfig::default();
        config
            .selected_models_by_provider
            .insert("route-a".into(), vec!["custom-model".into()]);

        let (synced, _) = sync_current_provider(&config, &home).unwrap();

        assert_eq!(synced.selected_models(), &["custom-model"]);
    }

    #[test]
    fn provider_switch_resets_subagent_defaults_when_default_model_is_supported() {
        let (_directory, _path, home) = fixture();
        write_live_route(
            &home,
            "route-b",
            "https://route-b.example/v1",
            "route-b-secret",
        );
        let mut config = CodeyConfig {
            active_profile_id: "route-a".into(),
            profiles: vec![saved_route_profile("route-a")],
            subagent_model: "provider-old-model".into(),
            subagent_reasoning_effort: "xhigh".into(),
            ..CodeyConfig::default()
        };
        config
            .upstream_models_by_provider
            .insert("route-b".into(), vec![DEFAULT_SUBAGENT_MODEL.into()]);

        let (synced, status) = sync_current_provider(&config, &home).unwrap();

        assert_eq!(status.provider.id, "route-b");
        assert_eq!(synced.subagent_model, DEFAULT_SUBAGENT_MODEL);
        assert_eq!(
            synced.subagent_reasoning_effort,
            DEFAULT_SUBAGENT_REASONING_EFFORT
        );
    }

    #[test]
    fn provider_switch_uses_available_subagent_model_when_default_is_unsupported() {
        let (_directory, _path, home) = fixture();
        write_live_route(
            &home,
            "route-b",
            "https://route-b.example/v1",
            "route-b-secret",
        );
        let mut config = CodeyConfig {
            active_profile_id: "route-a".into(),
            profiles: vec![saved_route_profile("route-a")],
            subagent_model: "provider-old-model".into(),
            subagent_reasoning_effort: "xhigh".into(),
            ..CodeyConfig::default()
        };
        config
            .upstream_models_by_provider
            .insert("route-b".into(), vec!["gpt-5.6-sol".into()]);

        let (synced, status) = sync_current_provider(&config, &home).unwrap();

        assert_eq!(status.provider.id, "route-b");
        assert_eq!(synced.subagent_model, "gpt-5.6-sol");
        assert_eq!(
            synced.subagent_reasoning_effort,
            DEFAULT_SUBAGENT_REASONING_EFFORT
        );
    }

    #[test]
    fn provider_sync_preserves_subagent_defaults_when_provider_is_unchanged() {
        let (_directory, _path, home) = fixture();
        write_live_route(
            &home,
            "route-a",
            "https://route-a.example/v1",
            "route-a-secret",
        );
        let config = CodeyConfig {
            active_profile_id: "route-a".into(),
            profiles: vec![saved_route_profile("route-a")],
            subagent_model: "provider-custom-model".into(),
            subagent_reasoning_effort: "high".into(),
            ..CodeyConfig::default()
        };

        let (synced, status) = sync_current_provider(&config, &home).unwrap();

        assert_eq!(status.provider.id, "route-a");
        assert_eq!(synced.subagent_model, "provider-custom-model");
        assert_eq!(synced.subagent_reasoning_effort, "high");
    }

    #[test]
    fn route_takeover_reads_proxy_config_and_live_marker() {
        let (_directory, path, home) = fixture();
        install_proxy_schema(&path);
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO proxy_config (app_type, enabled) VALUES ('codex', 1)",
                [],
            )
            .unwrap();
        write_live_route(
            &home,
            "relay",
            "http://127.0.0.1:15721/v1",
            PROXY_MANAGED_TOKEN,
        );

        assert_eq!(
            route_takeover_state_from_paths(&path, &home).unwrap(),
            RouteTakeoverState {
                managed: true,
                live: true,
            }
        );
    }

    #[test]
    fn route_takeover_recognizes_the_auth_placeholder_used_by_default_cc_switch_mode() {
        let (_directory, path, home) = fixture();
        install_proxy_schema(&path);
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO proxy_config (app_type, enabled) VALUES ('codex', 1)",
                [],
            )
            .unwrap();
        write_live_route(&home, "deepseek", "http://127.0.0.1:15721/v1", "");
        fs::write(
            home.join("auth.json"),
            br#"{"OPENAI_API_KEY":"PROXY_MANAGED"}"#,
        )
        .unwrap();

        assert_eq!(
            route_takeover_state_from_paths(&path, &home).unwrap(),
            RouteTakeoverState {
                managed: true,
                live: true,
            }
        );
    }

    #[test]
    fn ordinary_auth_api_key_does_not_claim_cc_switch_takeover() {
        let (_directory, path, home) = fixture();
        fs::write(
            home.join("auth.json"),
            br#"{"OPENAI_API_KEY":"sk-user-route"}"#,
        )
        .unwrap();

        assert_eq!(
            route_takeover_state_from_paths(&path, &home).unwrap(),
            RouteTakeoverState::default()
        );
    }

    #[test]
    fn route_takeover_treats_a_live_backup_as_managed() {
        let (_directory, path, home) = fixture();
        install_proxy_schema(&path);
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO proxy_live_backup (app_type, original_config, backed_up_at)
                 VALUES ('codex', '{}', 'now')",
                [],
            )
            .unwrap();

        assert_eq!(
            route_takeover_state_from_paths(&path, &home).unwrap(),
            RouteTakeoverState {
                managed: true,
                live: false,
            }
        );
    }

    #[test]
    fn official_proxy_provider_requires_a_loopback_endpoint() {
        let (_directory, path, home) = fixture();
        write_live_route(
            &home,
            PROXY_OFFICIAL_PROVIDER_ID,
            "http://localhost:15721/v1",
            "",
        );
        assert!(route_takeover_state_from_paths(&path, &home).unwrap().live);

        write_live_route(
            &home,
            PROXY_OFFICIAL_PROVIDER_ID,
            "https://relay.example/v1",
            "",
        );
        assert!(!route_takeover_state_from_paths(&path, &home).unwrap().live);
    }

    #[test]
    fn ordinary_loopback_provider_is_not_mistaken_for_cc_switch_routing() {
        let (_directory, path, home) = fixture();
        write_live_route(
            &home,
            "my-local-relay",
            "http://127.0.0.1:8080/v1",
            "sk-local",
        );

        assert_eq!(
            route_takeover_state_from_paths(&path, &home).unwrap(),
            RouteTakeoverState::default()
        );
    }

    #[test]
    fn route_takeover_safely_degrades_for_an_old_database_schema() {
        let (_directory, path, home) = fixture();

        assert_eq!(
            route_takeover_state_from_paths(&path, &home).unwrap(),
            RouteTakeoverState::default()
        );
    }
}
