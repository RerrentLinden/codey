use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use codey_runtime_core::settings::RelayProtocol;
use directories::BaseDirs;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use toml_edit::{DocumentMut, Item, TableLike};

use crate::config::{CodeyConfig, ProviderProfile};
use crate::sqlite_util::table_columns;

const APP_TYPE: &str = "codex";
const OFFICIAL_PROVIDER_ID: &str = "codex-official";
const LOCAL_OFFICIAL_PROVIDER_ID: &str = "local-official";
const PROXY_MANAGED_TOKEN: &str = "PROXY_MANAGED";
const PROXY_OFFICIAL_PROVIDER_ID: &str = "cc-switch-official";

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

#[derive(Debug, Clone)]
struct ProviderRecord {
    id: String,
    name: String,
    settings_config: String,
    category: Option<String>,
}

#[derive(Debug, Clone)]
struct ProviderConnection {
    base_url: String,
    api_key: String,
    protocol: RelayProtocol,
    official: bool,
    supports_remote_compaction: bool,
}

pub fn default_db_path() -> PathBuf {
    if let Some(path) = std::env::var_os("CC_SWITCH_DB_PATH") {
        return PathBuf::from(path);
    }
    BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".cc-switch/cc-switch.db"))
        .unwrap_or_else(|| PathBuf::from(".cc-switch/cc-switch.db"))
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
    sync_current_provider_from_paths(config, &default_db_path(), codex_home)
}

fn sync_current_provider_from_paths(
    config: &CodeyConfig,
    db_path: &Path,
    codex_home: &Path,
) -> Result<(CodeyConfig, CcSwitchStatus)> {
    let (profile, provider, available, message) = if db_path.is_file() {
        let record = read_current_provider(db_path)?.ok_or_else(|| {
            anyhow::anyhow!("cc-switch 没有选中的 Codex 配置，请先在 cc-switch 中选择线路")
        })?;
        let connection = provider_connection(&record)?;
        let provider = CurrentProvider {
            id: record.id.clone(),
            name: record.name.clone(),
            official: connection.official,
            supports_remote_compaction: connection.supports_remote_compaction,
            base_url: connection.base_url.clone(),
            protocol: connection.protocol,
            source: "cc-switch".to_string(),
        };
        let profile = profile_from_provider(&provider, connection.api_key, true);
        (profile, provider, true, None)
    } else {
        let (provider, api_key) = local_provider(codex_home)?;
        let profile = profile_from_provider(&provider, api_key, false);
        (
            profile,
            provider,
            false,
            Some("未检测到 cc-switch，已读取本地 Codex 直登配置".to_string()),
        )
    };

    let mut next = config.clone();
    next.active_profile_id = profile.id.clone();
    next.profiles = vec![profile];
    next = next.normalize();
    let changed = &next != config;
    let status = CcSwitchStatus {
        available,
        path: db_path.to_string_lossy().to_string(),
        changed,
        message,
        provider,
    };
    Ok((next, status))
}

pub fn status_from_config(config: &CodeyConfig) -> CcSwitchStatus {
    let profile = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id)
        .or_else(|| config.profiles.first());
    let available = profile
        .and_then(|profile| profile.cc_switch_provider_id.as_ref())
        .is_some();
    let provider = profile
        .map(|profile| CurrentProvider {
            id: profile
                .cc_switch_provider_id
                .clone()
                .unwrap_or_else(|| profile.id.clone()),
            name: profile.name.clone(),
            official: profile.cc_switch_read_only,
            supports_remote_compaction: profile.supports_remote_compaction,
            base_url: profile.base_url.clone(),
            protocol: profile.protocol,
            source: if available { "cc-switch" } else { "local" }.to_string(),
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
        available,
        path: default_db_path().to_string_lossy().to_string(),
        changed: false,
        message: (!available).then(|| "当前使用本地 Codex 直登配置".to_string()),
        provider,
    }
}

fn profile_from_provider(
    provider: &CurrentProvider,
    api_key: String,
    cc_switch_managed: bool,
) -> ProviderProfile {
    ProviderProfile {
        id: provider.id.clone(),
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        api_key,
        protocol: provider.protocol,
        cc_switch_provider_id: cc_switch_managed.then(|| provider.id.clone()),
        cc_switch_read_only: provider.official,
        supports_remote_compaction: provider.supports_remote_compaction,
    }
}

fn read_current_provider(path: &Path) -> Result<Option<ProviderRecord>> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("打开 cc-switch 数据库失败：{}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection
        .query_row(
            "SELECT id, name, settings_config, category
             FROM providers
             WHERE app_type=?1 AND is_current=1
             ORDER BY CASE WHEN sort_index IS NULL THEN 1 ELSE 0 END, sort_index, created_at, name
             LIMIT 1",
            params![APP_TYPE],
            |row| {
                Ok(ProviderRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    settings_config: row.get(2)?,
                    category: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn provider_connection(record: &ProviderRecord) -> Result<ProviderConnection> {
    let settings = serde_json::from_str::<Value>(&record.settings_config)
        .context("解析 cc-switch 当前线路失败")?;
    let config_text = settings
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let document = DocumentMut::from_str(config_text).unwrap_or_default();
    let provider_key = document
        .get("model_provider")
        .and_then(Item::as_str)
        .unwrap_or_default();
    let provider_table = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_key))
        .and_then(Item::as_table_like);
    let base_url = provider_table
        .and_then(|table| table.get("base_url"))
        .and_then(Item::as_str)
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_string();
    let wire_api = provider_table
        .and_then(|table| table.get("wire_api"))
        .and_then(Item::as_str)
        .unwrap_or("responses");
    let provider_name = provider_table
        .and_then(|table| table.get("name"))
        .and_then(Item::as_str)
        .map(str::trim)
        .unwrap_or_default();
    let auth = settings.get("auth").and_then(Value::as_object);
    let auth_api_key = auth
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let config_api_key = provider_config_api_key(&document, provider_table);
    // CC Switch keeps the canonical provider key in auth.OPENAI_API_KEY, but
    // its "keep official login" compatibility mode may leave ChatGPT OAuth in
    // auth.json and put the active third-party token in config.toml instead.
    let api_key = auth_api_key
        .map(ToString::to_string)
        .or(config_api_key)
        .unwrap_or_default();
    let auth_mode = auth
        .and_then(|auth| auth.get("auth_mode"))
        .and_then(Value::as_str);
    let official_auth_route =
        auth_mode == Some("chatgpt") && is_official_base_url(&base_url) && api_key.is_empty();
    let official = record.id == OFFICIAL_PROVIDER_ID
        || record.category.as_deref() == Some("official")
        || provider_key.is_empty()
        || base_url.is_empty()
        || official_auth_route;
    if !(official || base_url.starts_with("http://") || base_url.starts_with("https://")) {
        bail!("cc-switch 当前第三方线路缺少有效的 API 地址");
    }
    Ok(ProviderConnection {
        base_url,
        api_key: if official { String::new() } else { api_key },
        protocol: protocol_from_wire_api(wire_api),
        official,
        supports_remote_compaction: official || provider_name == "OpenAI",
    })
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
    // The provider-scoped token describes the active live route and must win
    // over a long-lived auth.json login retained by CC Switch.
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

    #[test]
    fn imports_only_the_current_cc_switch_provider() {
        let (_directory, path, home) = fixture();
        insert_provider(&path, "route-a", "线路 A", "https://a.example/v1", false);
        insert_provider(&path, "route-b", "线路 B", "https://b.example/v1", true);

        let (synced, status) =
            sync_current_provider_from_paths(&CodeyConfig::default(), &path, &home).unwrap();

        assert!(status.available);
        assert_eq!(status.provider.id, "route-b");
        assert_eq!(synced.profiles.len(), 1);
        assert_eq!(synced.profiles[0].base_url, "https://b.example/v1");
        assert_eq!(synced.profiles[0].api_key, "route-b-secret");
    }

    #[test]
    fn official_tokens_are_never_copied_into_a_provider_profile() {
        let (_directory, path, home) = fixture();
        let settings = json!({
            "auth": {"auth_mode": "chatgpt", "tokens": {"access_token": "secret"}},
            "config": ""
        });
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO providers
                 (id, app_type, name, settings_config, category, sort_index, is_current)
                 VALUES ('codex-official', 'codex', 'OpenAI Official', ?1, 'official', 0, 1)",
                [settings.to_string()],
            )
            .unwrap();

        let (synced, status) =
            sync_current_provider_from_paths(&CodeyConfig::default(), &path, &home).unwrap();

        assert!(status.provider.official);
        assert!(synced.profiles[0].api_key.is_empty());
    }

    #[test]
    fn preserved_chatgpt_login_does_not_replace_a_cc_switch_api_route() {
        let (_directory, path, home) = fixture();
        let settings = json!({
            "auth": {
                "auth_mode": "chatgpt",
                "tokens": {"access_token": "free-account-token"}
            },
            "config": r#"
model_provider = "custom"

[model_providers.custom]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
experimental_bearer_token = "sk-relay"
"#
        });
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO providers
                 (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES ('route-a', 'codex', '线路 A', ?1, 0, 1)",
                [settings.to_string()],
            )
            .unwrap();
        let auth = br#"{"auth_mode":"chatgpt","tokens":{"access_token":"free-account-token"}}"#;
        fs::write(home.join("auth.json"), auth).unwrap();

        let (synced, status) =
            sync_current_provider_from_paths(&CodeyConfig::default(), &path, &home).unwrap();

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
    fn remote_compaction_identity_survives_cc_switch_import_and_runtime_patch() {
        let (_directory, path, home) = fixture();
        let settings = json!({
            "auth": {"OPENAI_API_KEY": "sk-relay"},
            "config": r#"
model_provider = "custom"

[model_providers.custom]
name = "OpenAI"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
        });
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO providers
                 (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES ('remote-compact', 'codex', '远程压缩线路', ?1, 0, 1)",
                [settings.to_string()],
            )
            .unwrap();

        let (synced, status) =
            sync_current_provider_from_paths(&CodeyConfig::default(), &path, &home).unwrap();

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
    fn falls_back_to_local_official_login_without_cc_switch() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("codex-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("auth.json"),
            br#"{"auth_mode":"chatgpt","tokens":{"access_token":"secret"}}"#,
        )
        .unwrap();

        let (synced, status) = sync_current_provider_from_paths(
            &CodeyConfig::default(),
            &directory.path().join("missing.db"),
            &home,
        )
        .unwrap();

        assert!(!status.available);
        assert!(status.provider.official);
        assert_eq!(status.provider.source, "local");
        assert!(synced.profiles[0].api_key.is_empty());
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

        let (synced, status) = sync_current_provider_from_paths(
            &CodeyConfig::default(),
            &directory.path().join("missing.db"),
            &home,
        )
        .unwrap();

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
    fn manual_api_route_reads_auth_json_api_key_without_cc_switch() {
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

        let (synced, status) = sync_current_provider_from_paths(
            &CodeyConfig::default(),
            &directory.path().join("missing.db"),
            &home,
        )
        .unwrap();

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
        let (_directory, path, home) = fixture();
        insert_provider(&path, "route-a", "线路 A", "https://a.example/v1", true);
        let mut config = CodeyConfig::default();
        config
            .selected_models_by_provider
            .insert("route-a".into(), vec!["custom-model".into()]);

        let (synced, _) = sync_current_provider_from_paths(&config, &path, &home).unwrap();

        assert_eq!(synced.selected_models(), &["custom-model"]);
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
