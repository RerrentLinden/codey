use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use codey_runtime_core::config_manager::ConfigManager;
use directories::BaseDirs;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use toml_edit::{DocumentMut, Item, TableLike};

use crate::codex_config::is_reserved_provider_id;
use crate::config::{CodeyConfig, ProviderProfile};
use crate::sqlite_util::table_columns;

const APP_TYPE: &str = "codex";
const OFFICIAL_PROVIDER_ID: &str = "codex-official";
const LOCAL_OFFICIAL_PROVIDER_ID: &str = "local-official";
const PROXY_MANAGED_TOKEN: &str = "PROXY_MANAGED";
const PROXY_OFFICIAL_PROVIDER_ID: &str = "cc-switch-official";
const CODE_SWITCH_R_PROVIDER_IDS: &[&str] = &["code-switch-r", "code-switch"];
const CODE_SWITCH_R_TOKEN: &str = "code-switch-r";
const CC_SWITCH_APP_ID: &str = "com.ccswitch.desktop";
const CC_SWITCH_PATH_STORE: &str = "app_paths.json";
const CC_SWITCH_CONFIG_DIR_KEY: &str = "app_config_dir_override";
const CC_SWITCH_DB_FILE: &str = "cc-switch.db";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaVariant {
    AppScoped,
    LegacyUnscoped,
}

impl SchemaVariant {
    fn from_columns(columns: &HashSet<String>) -> Self {
        if columns.contains("app_type") {
            Self::AppScoped
        } else {
            Self::LegacyUnscoped
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProviderSchema {
    variant: SchemaVariant,
    has_id: bool,
    has_settings_config: bool,
    has_is_current: bool,
    has_category: bool,
}

impl ProviderSchema {
    fn inspect(connection: &Connection) -> Result<Self> {
        let columns = table_columns(connection, "providers")?;
        Ok(Self {
            variant: SchemaVariant::from_columns(&columns),
            has_id: columns.contains("id"),
            has_settings_config: columns.contains("settings_config"),
            has_is_current: columns.contains("is_current"),
            has_category: columns.contains("category"),
        })
    }

    fn supports_source_api(self) -> bool {
        self.has_id && self.has_settings_config && self.has_is_current
    }

    fn category_projection(self) -> &'static str {
        if self.has_category {
            "category"
        } else {
            "NULL"
        }
    }

    fn query_current<T>(
        self,
        connection: &Connection,
        projection: &str,
        map: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>> {
        let scoped_query = format!(
            "SELECT {projection}
             FROM providers
             WHERE app_type=?1 AND is_current != 0
             LIMIT 1"
        );
        let legacy_query = format!(
            "SELECT {projection}
             FROM providers
             WHERE is_current != 0
             LIMIT 1"
        );
        let result = match self.variant {
            SchemaVariant::AppScoped => connection.query_row(&scoped_query, params![APP_TYPE], map),
            SchemaVariant::LegacyUnscoped => connection.query_row(&legacy_query, [], map),
        };
        Ok(result.optional()?)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CurrentProvider {
    pub id: String,
    pub name: String,
    pub official: bool,
    pub supports_remote_compaction: bool,
    pub base_url: String,
    pub local_route: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CcSwitchStatus {
    pub changed: bool,
    pub provider: CurrentProvider,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteTakeoverState {
    pub managed: bool,
    pub live: bool,
}

pub struct LiveRouteSnapshot {
    provider: CurrentProvider,
    profile: ProviderProfile,
    config_contents: Vec<u8>,
    auth_contents: Option<Vec<u8>>,
}

impl LiveRouteSnapshot {
    pub fn provider_id(&self) -> &str {
        &self.provider.id
    }

    pub fn profile(&self) -> &ProviderProfile {
        &self.profile
    }

    pub fn config_contents(&self) -> &[u8] {
        &self.config_contents
    }

    pub fn auth_contents(&self) -> Option<&[u8]> {
        self.auth_contents.as_deref()
    }
}

pub struct StartupRouteState {
    pub takeover: RouteTakeoverState,
    pub live_route: Option<LiveRouteSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CcSwitchSourceApi {
    base_url: String,
    api_key: String,
    model_request_headers: BTreeMap<String, String>,
}

struct ProviderRequestExtensions {
    api_key: Option<String>,
    headers: BTreeMap<String, String>,
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

pub fn startup_route_state(codex_home: &Path) -> Result<StartupRouteState> {
    startup_route_state_from_paths(&default_db_path(), codex_home)
}

pub fn provider_model_fetch_profile(
    profile: &ProviderProfile,
    codex_home: &Path,
) -> Result<ProviderProfile> {
    provider_model_fetch_profile_from_paths(profile, codex_home, &default_db_path())
}

fn provider_model_fetch_profile_from_paths(
    profile: &ProviderProfile,
    codex_home: &Path,
    db_path: &Path,
) -> Result<ProviderProfile> {
    let takeover = route_takeover_state_from_paths(db_path, codex_home)?;
    if !(takeover.managed && takeover.live) {
        let mut fetch_profile = profile.clone();
        if let Some(extensions) = local_provider_model_request_extensions(codex_home, profile)? {
            if let Some(api_key) = extensions.api_key {
                fetch_profile.api_key = api_key;
            }
            fetch_profile.model_request_headers = extensions.headers;
        }
        return Ok(fetch_profile);
    }

    let source = read_current_cc_switch_source_api(db_path)?;
    let mut fetch_profile = profile.clone();
    fetch_profile.base_url = source.base_url;
    fetch_profile.api_key = source.api_key;
    fetch_profile.model_request_headers = source.model_request_headers;
    Ok(fetch_profile)
}

fn local_provider_model_request_extensions(
    codex_home: &Path,
    profile: &ProviderProfile,
) -> Result<Option<ProviderRequestExtensions>> {
    let config_path = codex_home.join("config.toml");
    let snapshot = ConfigManager::new(&config_path).load()?;
    if !snapshot.exists() {
        return Ok(None);
    }
    let document = snapshot.document();
    let provider_id = document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(LOCAL_OFFICIAL_PROVIDER_ID);
    if provider_id != profile.id {
        return Ok(None);
    }
    let provider = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table_like);
    Ok(provider.map(|provider| ProviderRequestExtensions {
        api_key: provider_config_api_key(document, Some(provider)),
        headers: provider_model_request_headers(provider),
    }))
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

fn startup_route_state_from_paths(db_path: &Path, codex_home: &Path) -> Result<StartupRouteState> {
    let managed = if db_path.is_file() {
        read_route_takeover_managed(db_path)?
    } else {
        false
    };
    let auth_path = codex_home.join("auth.json");
    let auth_contents = match fs::read(&auth_path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取 Codex Live 认证失败：{}", auth_path.display()));
        }
    };
    let auth = auth_contents
        .as_deref()
        .map(|contents| {
            serde_json::from_slice::<Value>(contents)
                .with_context(|| format!("解析 Codex Live 认证失败：{}", auth_path.display()))
        })
        .transpose()?;
    let auth_managed = auth.as_ref().is_some_and(auth_uses_proxy_route);

    let config_path = codex_home.join("config.toml");
    let snapshot = ConfigManager::new(&config_path).load()?;
    let config_contents = snapshot.exists().then(|| snapshot.raw().to_vec());
    let document = snapshot.exists().then(|| snapshot.document().clone());
    let config_managed = document.as_ref().is_some_and(document_uses_proxy_route);
    let live = auth_managed || config_managed;
    let takeover = RouteTakeoverState { managed, live };
    if !live {
        return Ok(StartupRouteState {
            takeover,
            live_route: None,
        });
    }

    let document = document.ok_or_else(|| {
        anyhow::anyhow!(
            "CC Switch 路由已标记为 Live，但 Codex config.toml 不存在；请在 CC Switch 中关闭并重新开启 Codex 路由后重试"
        )
    })?;
    let config_contents = config_contents.expect("parsed Live config has source bytes");
    let live_route =
        validated_live_route_snapshot(document, config_contents, auth_contents, auth.as_ref())?;
    Ok(StartupRouteState {
        takeover,
        live_route: Some(live_route),
    })
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
    Ok(auth_uses_proxy_route(&document))
}

fn auth_uses_proxy_route(document: &Value) -> bool {
    document
        .get("OPENAI_API_KEY")
        .and_then(Value::as_str)
        .is_some_and(|token| token.trim() == PROXY_MANAGED_TOKEN)
}

fn read_route_takeover_managed(path: &Path) -> Result<bool> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("打开 cc-switch 数据库失败：{}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(2))?;

    let proxy_columns = table_columns(&connection, "proxy_config")?;
    let proxy_schema = SchemaVariant::from_columns(&proxy_columns);
    let proxy_enabled = if proxy_columns.contains("enabled") {
        let enabled = match proxy_schema {
            SchemaVariant::AppScoped => connection.query_row(
                "SELECT COALESCE(MAX(enabled), 0) FROM proxy_config WHERE app_type=?1",
                params![APP_TYPE],
                |row| row.get::<_, i64>(0),
            )?,
            SchemaVariant::LegacyUnscoped => connection.query_row(
                "SELECT COALESCE(MAX(enabled), 0) FROM proxy_config",
                [],
                |row| row.get::<_, i64>(0),
            )?,
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
    let backup_schema = SchemaVariant::from_columns(&backup_columns);
    let has_live_backup = if backup_columns.is_empty() {
        false
    } else {
        match backup_schema {
            SchemaVariant::AppScoped => {
                connection.query_row(
                    "SELECT EXISTS(
                    SELECT 1 FROM proxy_live_backup WHERE app_type=?1
                 )",
                    params![APP_TYPE],
                    |row| row.get::<_, i64>(0),
                )? != 0
            }
            SchemaVariant::LegacyUnscoped => {
                connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM proxy_live_backup)",
                    [],
                    |row| row.get::<_, i64>(0),
                )? != 0
            }
        }
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
    let snapshot = ConfigManager::new(&config_path).load()?;
    if !snapshot.exists() {
        return Ok(false);
    }
    Ok(document_uses_proxy_route(snapshot.document()))
}

fn document_uses_proxy_route(document: &DocumentMut) -> bool {
    let Some(provider_id) = document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    else {
        return false;
    };
    let provider = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table_like);
    let route_token = provider
        .and_then(|provider| provider.get("experimental_bearer_token"))
        .and_then(Item::as_str)
        .or_else(|| {
            document
                .get("experimental_bearer_token")
                .and_then(Item::as_str)
        })
        .map(str::trim);
    let loopback = provider
        .and_then(|provider| provider.get("base_url"))
        .and_then(Item::as_str)
        .is_some_and(is_loopback_url);
    let cc_switch_route = route_token == Some(PROXY_MANAGED_TOKEN)
        || (provider_id == PROXY_OFFICIAL_PROVIDER_ID && loopback);
    // Code Switch R owns a fixed provider ID and points it at its loopback
    // router. Its placeholder is intentionally different from CC Switch's
    // PROXY_MANAGED marker, so provider identity plus loopback is the stable
    // ownership signal (including its preserve-official-auth mode).
    let code_switch_r_route = loopback
        && CODE_SWITCH_R_PROVIDER_IDS
            .iter()
            .any(|candidate| provider_id.eq_ignore_ascii_case(candidate));
    cc_switch_route || code_switch_r_route
}

fn is_loopback_url(url: &str) -> bool {
    let authority_and_path = url
        .trim()
        .split_once("://")
        .map_or(url.trim(), |(_, rest)| rest);
    let authority = authority_and_path
        .split_once('/')
        .map_or(authority_and_path, |(authority, _)| authority);
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, authority)| authority);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map_or(rest, |(host, _)| host)
    } else {
        authority
            .split_once(':')
            .map_or(authority, |(host, _)| host)
    };
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host.starts_with("127.")
}

pub fn sync_current_provider(
    config: &CodeyConfig,
    codex_home: &Path,
) -> Result<(CodeyConfig, CcSwitchStatus)> {
    let (provider, api_key) = local_provider(codex_home)?;
    let profile = profile_from_provider(&provider, api_key);

    let mut next = config.clone();
    next.active_profile_id = profile.id.clone();
    next.profiles = vec![profile];
    next = next.normalize();
    let changed = &next != config;
    let status = CcSwitchStatus { changed, provider };
    Ok((next, status))
}

fn read_current_cc_switch_source_api(db_path: &Path) -> Result<CcSwitchSourceApi> {
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("打开 cc-switch 数据库失败：{}", db_path.display()))?;
    connection.busy_timeout(Duration::from_secs(2))?;

    let provider_schema = ProviderSchema::inspect(&connection)?;
    if !provider_schema.supports_source_api() {
        bail!("CC Switch 路由已开启，但数据库缺少当前源 API 配置");
    }
    let current_provider = provider_schema.query_current(
        &connection,
        &format!(
            "id, settings_config, {}",
            provider_schema.category_projection()
        ),
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    )?;
    let Some((provider_id, settings_config, category)) = current_provider else {
        bail!("CC Switch 路由已开启，但找不到当前 Codex 源线路");
    };

    cc_switch_source_api(&provider_id, category.as_deref(), &settings_config)
}

fn cc_switch_source_api(
    provider_id: &str,
    category: Option<&str>,
    settings_config: &str,
) -> Result<CcSwitchSourceApi> {
    if provider_id == OFFICIAL_PROVIDER_ID
        || category.is_some_and(|value| value.eq_ignore_ascii_case("official"))
    {
        bail!("CC Switch 当前为官方线路，无需从第三方源 API 同步模型");
    }

    let settings =
        serde_json::from_str::<Value>(settings_config).context("解析 CC Switch 当前源线路失败")?;
    let config_text = settings
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let document =
        DocumentMut::from_str(config_text).context("解析 CC Switch 当前源 API 配置失败")?;
    let source_provider_id = document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("CC Switch 当前源线路缺少 model_provider")?;
    let provider = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(source_provider_id))
        .and_then(Item::as_table_like)
        .context("CC Switch 当前源线路缺少 provider 配置")?;
    let base_url = provider
        .get("base_url")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .context("CC Switch 当前源线路缺少 API 地址")?;
    if !is_safe_source_api_url(&base_url) {
        bail!("CC Switch 当前源线路缺少有效的非回环 HTTP(S) API 地址");
    }

    let auth = settings.get("auth").and_then(Value::as_object);
    let auth_api_key = auth
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let config_api_key = provider_config_api_key(&document, Some(provider));
    let model_request_headers = provider_model_request_headers(provider);
    let proxy_marker_present = auth_api_key.as_deref() == Some(PROXY_MANAGED_TOKEN)
        || config_api_key.as_deref() == Some(PROXY_MANAGED_TOKEN);
    let api_key = [config_api_key, auth_api_key]
        .into_iter()
        .flatten()
        .find(|value| value != PROXY_MANAGED_TOKEN)
        .unwrap_or_default();
    if api_key.is_empty() && proxy_marker_present {
        bail!("CC Switch 当前源线路仅包含路由占位凭据，无法安全同步模型");
    }

    Ok(CcSwitchSourceApi {
        base_url,
        api_key,
        model_request_headers,
    })
}

fn is_safe_source_api_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    !host.eq_ignore_ascii_case("localhost")
        && !host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
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
            local_route: profile_uses_local_route(profile),
        })
        .unwrap_or_else(|| CurrentProvider {
            id: LOCAL_OFFICIAL_PROVIDER_ID.to_string(),
            name: "OpenAI 官方直登".to_string(),
            official: true,
            supports_remote_compaction: true,
            base_url: String::new(),
            local_route: false,
        });
    CcSwitchStatus {
        changed: false,
        provider,
    }
}

fn profile_uses_local_route(profile: &ProviderProfile) -> bool {
    if !is_loopback_url(&profile.base_url) {
        return false;
    }
    let api_key = profile.api_key.trim();
    api_key == PROXY_MANAGED_TOKEN
        || api_key == CODE_SWITCH_R_TOKEN
        || profile.id == PROXY_OFFICIAL_PROVIDER_ID
        || CODE_SWITCH_R_PROVIDER_IDS
            .iter()
            .any(|candidate| profile.id.eq_ignore_ascii_case(candidate))
}

fn profile_from_provider(provider: &CurrentProvider, api_key: String) -> ProviderProfile {
    ProviderProfile {
        id: provider.id.clone(),
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        api_key,
        model_request_headers: BTreeMap::new(),
        cc_switch_provider_id: None,
        cc_switch_read_only: provider.official,
        supports_remote_compaction: provider.supports_remote_compaction,
    }
}

fn validated_live_route_snapshot(
    document: DocumentMut,
    config_contents: Vec<u8>,
    auth_contents: Option<Vec<u8>>,
    auth: Option<&Value>,
) -> Result<LiveRouteSnapshot> {
    let provider_id = document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "CC Switch Live 配置缺少活动 model_provider；请关闭并重新开启 Codex 路由后重试"
            )
        })?;
    let table = document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table_like)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "CC Switch Live 配置中的活动 Provider「{provider_id}」不存在；已停止启动以避免损坏会话或把密钥发往错误地址，请关闭并重新开启 Codex 路由后重试"
            )
        })?;
    let base_url = table
        .get("base_url")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "CC Switch Live Provider「{provider_id}」缺少 API 地址；已停止启动以避免把密钥发往错误地址"
            )
        })?;
    let parsed_base_url = reqwest::Url::parse(&base_url)
        .with_context(|| format!("CC Switch Live Provider「{provider_id}」的 API 地址无效"))?;
    if !matches!(parsed_base_url.scheme(), "http" | "https") || parsed_base_url.host_str().is_none()
    {
        bail!("CC Switch Live Provider「{provider_id}」的 API 地址必须是有效的 HTTP(S) 地址");
    }

    let name = table
        .get("name")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(provider_id);
    let wire_api = table
        .get("wire_api")
        .and_then(Item::as_str)
        .unwrap_or("responses");
    ensure_responses_wire_api(wire_api)?;
    let auth_mode = auth
        .and_then(|auth| auth.get("auth_mode"))
        .and_then(Value::as_str);
    let auth_api_key = auth
        .and_then(|auth| auth.get("OPENAI_API_KEY"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let config_api_key = provider_config_api_key(&document, Some(table));
    let api_key = config_api_key
        .or_else(|| auth_api_key.map(ToString::to_string))
        .unwrap_or_default();
    let official_endpoint = is_official_base_url(&base_url);
    let official = official_endpoint && (auth_mode == Some("chatgpt") || api_key.is_empty());
    if !official && is_reserved_provider_id(provider_id) {
        bail!(
            "CC Switch Live 第三方线路使用了 Codex 保留 Provider ID「{provider_id}」；Codex 可能忽略自定义地址并把密钥发往内置服务，已停止启动。请在 CC Switch 中改用非保留的自定义 Provider ID"
        );
    }

    let provider = CurrentProvider {
        id: provider_id.to_string(),
        name: if official {
            "OpenAI 官方直登".to_string()
        } else {
            name.to_string()
        },
        official,
        supports_remote_compaction: official || name == "OpenAI",
        local_route: is_loopback_url(&base_url),
        base_url,
    };
    let profile = profile_from_provider(&provider, if official { String::new() } else { api_key });

    Ok(LiveRouteSnapshot {
        provider,
        profile,
        config_contents,
        auth_contents,
    })
}

fn local_provider(codex_home: &Path) -> Result<(CurrentProvider, String)> {
    let config_path = codex_home.join("config.toml");
    let snapshot = ConfigManager::new(&config_path).load()?;
    let document = snapshot.document();
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
    ensure_responses_wire_api(wire_api)?;
    let auth_path = codex_home.join("auth.json");
    let auth = match fs::read(&auth_path) {
        Ok(bytes) => Some(
            serde_json::from_slice::<Value>(&bytes)
                .with_context(|| format!("解析本地 Codex 认证失败：{}", auth_path.display()))?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("读取本地 Codex 认证失败：{}", auth_path.display()));
        }
    };
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
    let config_api_key = provider_config_api_key(document, table);
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
    let local_route = is_loopback_url(&base_url)
        && (document_uses_proxy_route(document)
            || auth.as_ref().is_some_and(auth_uses_proxy_route));
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
        local_route,
    };
    Ok((provider, if official { String::new() } else { api_key }))
}

fn provider_config_api_key(
    document: &DocumentMut,
    provider: Option<&dyn TableLike>,
) -> Option<String> {
    provider_config_api_key_with_env(document, provider, &|name| std::env::var(name).ok())
}

fn provider_config_api_key_with_env(
    document: &DocumentMut,
    provider: Option<&dyn TableLike>,
    env_value: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    const PROVIDER_KEYS: &[&str] = &[
        "experimental_bearer_token",
        "api_key",
        "apikey",
        "bearer_token",
        "token",
    ];
    const PROVIDER_ENV_KEYS: &[&str] = &[
        "env_key",
        "api_key_env",
        "api_key_env_var",
        "key_env",
        "bearer_token_env",
    ];
    PROVIDER_KEYS
        .iter()
        .find_map(|key| {
            provider
                .and_then(|provider| provider.get(key))
                .and_then(Item::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            PROVIDER_ENV_KEYS.iter().find_map(|key| {
                let name = provider
                    .and_then(|provider| provider.get(key))
                    .and_then(Item::as_str)?
                    .trim();
                if name.is_empty() {
                    return None;
                }
                env_value(name)
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
        })
        .or_else(|| {
            document
                .get("experimental_bearer_token")
                .and_then(Item::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
}

fn provider_model_request_headers(provider: &dyn TableLike) -> BTreeMap<String, String> {
    provider_model_request_headers_with_env(provider, &|name| std::env::var(name).ok())
}

fn provider_model_request_headers_with_env(
    provider: &dyn TableLike,
    env_value: &impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    if let Some(configured) = provider.get("http_headers").and_then(Item::as_table_like) {
        for (name, item) in configured.iter() {
            if let Some(value) = item.as_str() {
                insert_model_request_header(&mut headers, name, value);
            }
        }
    }
    if let Some(configured) = provider
        .get("env_http_headers")
        .and_then(Item::as_table_like)
    {
        for (name, item) in configured.iter() {
            let Some(env_name) = item.as_str().map(str::trim).filter(|name| !name.is_empty())
            else {
                continue;
            };
            let Some(value) = env_value(env_name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            insert_model_request_header(&mut headers, name, &value);
        }
    }
    headers
}

fn insert_model_request_header(headers: &mut BTreeMap<String, String>, name: &str, value: &str) {
    let name = name.trim();
    if name.is_empty() || (name.eq_ignore_ascii_case("authorization") && value.trim().is_empty()) {
        return;
    }
    if let Some(existing) = headers
        .keys()
        .find(|existing| existing.eq_ignore_ascii_case(name))
        .cloned()
    {
        headers.remove(&existing);
    }
    headers.insert(name.to_string(), value.to_string());
}

fn is_official_base_url(base_url: &str) -> bool {
    let base_url = base_url.to_ascii_lowercase();
    base_url.contains("chatgpt.com/backend-api/codex") || base_url.contains("api.openai.com")
}

fn ensure_responses_wire_api(value: &str) -> Result<()> {
    if value.trim().to_ascii_lowercase().contains("chat") {
        bail!("Codey 已移除协议转换能力；当前线路不是 Responses API，请改用第三方网关完成协议路由");
    }
    Ok(())
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
                    meta TEXT NOT NULL DEFAULT '{}',
                    category TEXT,
                    created_at INTEGER,
                    sort_index INTEGER,
                    is_current BOOLEAN NOT NULL DEFAULT 0,
                    PRIMARY KEY (id, app_type)
                );
                CREATE TABLE provider_endpoints (
                    id INTEGER PRIMARY KEY,
                    provider_id TEXT NOT NULL,
                    app_type TEXT NOT NULL,
                    url TEXT NOT NULL
                );",
            )
            .unwrap();
        (directory, path, home)
    }

    #[test]
    fn local_provider_rejects_malformed_config() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        fs::write(home.join("config.toml"), "model_provider = [").unwrap();

        let error = local_provider(home).unwrap_err();

        assert!(format!("{error:#}").contains("解析 "));
        assert!(format!("{error:#}").contains("config.toml"));
    }

    #[test]
    fn local_provider_rejects_malformed_auth() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        write_live_route(home, "third-party", "https://third-party.example/v1", "");
        fs::write(home.join("auth.json"), "{").unwrap();

        let error = local_provider(home).unwrap_err();

        assert!(format!("{error:#}").contains("解析本地 Codex 认证失败"));
        assert!(format!("{error:#}").contains("auth.json"));
    }

    #[test]
    fn local_provider_propagates_non_not_found_auth_read_errors() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        write_live_route(home, "third-party", "https://third-party.example/v1", "");
        fs::create_dir(home.join("auth.json")).unwrap();

        let error = local_provider(home).unwrap_err();

        assert!(format!("{error:#}").contains("读取本地 Codex 认证失败"));
        assert!(format!("{error:#}").contains("auth.json"));
    }

    #[test]
    fn legacy_unscoped_schema_reads_current_provider_and_endpoints() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE providers (
                    id TEXT PRIMARY KEY,
                    settings_config TEXT NOT NULL,
                    meta TEXT NOT NULL DEFAULT '{}',
                    is_current BOOLEAN NOT NULL DEFAULT 0
                );
                CREATE TABLE provider_endpoints (
                    id INTEGER PRIMARY KEY,
                    provider_id TEXT NOT NULL,
                    url TEXT NOT NULL
                );",
            )
            .unwrap();
        let base_url = "https://legacy.example/v1";
        let settings = json!({
            "auth": {"OPENAI_API_KEY": "legacy-secret"},
            "config": format!(
                "model_provider = \"legacy\"\n\n[model_providers.legacy]\nbase_url = \"{base_url}\"\nwire_api = \"responses\"\n"
            )
        })
        .to_string();
        connection
            .execute(
                "INSERT INTO providers (id, settings_config, meta, is_current)
                 VALUES (?1, ?2, ?3, 1)",
                params![
                    "legacy-provider",
                    settings,
                    json!({"apiFormat": "openai_chat"}).to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO provider_endpoints (provider_id, url) VALUES (?1, ?2)",
                params!["legacy-provider", base_url],
            )
            .unwrap();
        drop(connection);

        let source = read_current_cc_switch_source_api(&path).unwrap();
        assert_eq!(source.base_url, base_url);
        assert_eq!(source.api_key, "legacy-secret");
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

    fn insert_provider_with_api_format(
        path: &Path,
        id: &str,
        name: &str,
        url: &str,
        current: bool,
        api_format: Option<&str>,
    ) {
        let settings = json!({
            "auth": {"OPENAI_API_KEY": format!("{id}-secret")},
            "config": format!(
                "model_provider = \"custom\"\n\n[model_providers.custom]\nname = \"custom\"\nbase_url = \"{url}\"\nwire_api = \"responses\"\n"
            )
        });
        let meta = api_format
            .map(|api_format| json!({"apiFormat": api_format}))
            .unwrap_or_else(|| json!({}));
        Connection::open(path)
            .unwrap()
            .execute(
                "INSERT INTO providers
                 (id, app_type, name, settings_config, meta, sort_index, is_current)
                 VALUES (?1, 'codex', ?2, ?3, ?4, 0, ?5)",
                params![id, name, settings.to_string(), meta.to_string(), current],
            )
            .unwrap();
    }

    fn insert_provider_endpoint(path: &Path, provider_id: &str, url: &str) {
        Connection::open(path)
            .unwrap()
            .execute(
                "INSERT INTO provider_endpoints (provider_id, app_type, url)
                 VALUES (?1, 'codex', ?2)",
                params![provider_id, url],
            )
            .unwrap();
    }

    fn saved_route_profile(id: &str) -> ProviderProfile {
        ProviderProfile {
            id: id.to_string(),
            name: format!("线路 {id}"),
            base_url: format!("https://{id}.example/v1"),
            api_key: format!("{id}-secret"),
            model_request_headers: BTreeMap::new(),
            cc_switch_provider_id: Some(id.to_string()),
            cc_switch_read_only: false,
            supports_remote_compaction: false,
        }
    }

    #[test]
    fn codex_config_wins_when_cc_switch_database_has_a_current_provider() {
        let (_directory, path, home) = fixture();
        insert_provider_with_api_format(
            &path,
            "cc-switch-route",
            "CC Switch 线路",
            "https://cc-switch.example/v1",
            true,
            Some("openai_chat"),
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

        assert_eq!(status.provider.id, "codex-local");
        assert_eq!(synced.profiles.len(), 1);
        assert_eq!(
            synced.profiles[0].base_url,
            "https://codex-local.example/v1"
        );
        assert_eq!(synced.profiles[0].api_key, "sk-codex-local");
        assert!(synced.profiles[0].cc_switch_provider_id.is_none());
    }

    #[test]
    fn deprecated_cc_switch_api_format_metadata_is_ignored() {
        let (_directory, path, home) = fixture();
        insert_provider_with_api_format(
            &path,
            "chat-route",
            "Chat 线路",
            "https://chat.example/v1",
            true,
            Some("openai_chat"),
        );
        fs::write(
            home.join("config.toml"),
            r#"
model_provider = "custom"

[model_providers.custom]
name = "custom"
base_url = "https://chat.example/v1"
wire_api = "responses"
experimental_bearer_token = "sk-chat"
"#,
        )
        .unwrap();

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert_eq!(status.provider.base_url, "https://chat.example/v1");
        assert!(synced.profiles[0].cc_switch_provider_id.is_none());
        assert_eq!(synced.profiles[0].base_url, "https://chat.example/v1");
        assert_eq!(synced.profiles[0].api_key, "sk-chat");
    }

    #[test]
    fn matching_cc_switch_database_metadata_does_not_tag_the_local_profile() {
        let (_directory, path, home) = fixture();
        insert_provider_with_api_format(
            &path,
            "responses-route",
            "Responses 线路",
            "https://responses.example/v1",
            true,
            Some("openai_responses"),
        );
        write_live_route(
            &home,
            "custom",
            "https://responses.example/v1",
            "sk-responses",
        );

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert_eq!(status.provider.base_url, "https://responses.example/v1");
        assert!(synced.profiles[0].cc_switch_provider_id.is_none());
    }

    #[test]
    fn selected_cc_switch_endpoint_does_not_override_the_codex_provider() {
        let (_directory, path, home) = fixture();
        insert_provider_with_api_format(
            &path,
            "chat-route",
            "Chat 线路",
            "https://default-chat.example/v1",
            true,
            Some("openai_chat"),
        );
        insert_provider_endpoint(&path, "chat-route", "https://selected-chat.example/v1/");
        write_live_route(
            &home,
            "custom",
            "https://selected-chat.example/v1",
            "sk-chat",
        );

        let (synced, _) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(synced.profiles[0].cc_switch_provider_id.is_none());
    }

    #[test]
    fn cc_switch_loopback_route_stays_owned_by_cc_switch() {
        let (_directory, path, home) = fixture();
        insert_provider_with_api_format(
            &path,
            "chat-route",
            "Chat 线路",
            "https://chat.example/v1",
            true,
            Some("openai_chat"),
        );
        write_live_route(
            &home,
            PROXY_OFFICIAL_PROVIDER_ID,
            "http://127.0.0.1:15721/v1",
            PROXY_MANAGED_TOKEN,
        );

        let (synced, status) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert!(status.provider.local_route);
        assert!(synced.profiles[0].cc_switch_provider_id.is_none());
    }

    #[test]
    fn old_cc_switch_schema_does_not_block_local_provider_sync() {
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
                    settings_config TEXT NOT NULL,
                    is_current BOOLEAN NOT NULL DEFAULT 0
                );",
            )
            .unwrap();
        write_live_route(&home, "manual", "https://manual.example/v1", "sk-manual");

        let (synced, _) = sync_current_provider(&CodeyConfig::default(), &home).unwrap();

        assert_eq!(synced.profiles[0].base_url, "https://manual.example/v1");
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

        assert!(status.provider.official);
        assert!(synced.profiles[0].api_key.is_empty());
    }

    #[test]
    fn status_from_config_keeps_the_saved_provider_identity() {
        let config = CodeyConfig {
            active_profile_id: "route-a".into(),
            profiles: vec![saved_route_profile("route-a")],
            ..CodeyConfig::default()
        };
        let status = status_from_config(&config);

        assert_eq!(status.provider.id, "route-a");
    }

    #[test]
    fn status_from_config_marks_a_saved_code_switch_r_endpoint_as_a_local_route() {
        let mut profile = saved_route_profile("code-switch-r");
        profile.base_url = "http://127.0.0.1:18100".into();
        profile.api_key = CODE_SWITCH_R_TOKEN.into();
        let config = CodeyConfig {
            active_profile_id: profile.id.clone(),
            profiles: vec![profile],
            ..CodeyConfig::default()
        };

        let status = status_from_config(&config);

        assert!(status.provider.local_route);
        assert_eq!(
            serde_json::to_value(status).unwrap()["provider"]["localRoute"],
            true
        );
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

        assert!(!status.provider.official);
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
    fn model_fetch_uses_the_cc_switch_source_api_during_live_route_takeover() {
        let (_directory, path, home) = fixture();
        install_proxy_schema(&path);
        insert_provider_with_api_format(
            &path,
            "source-route",
            "源线路",
            "https://source.example/v1",
            true,
            Some("openai_responses"),
        );
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
        let live_profile = ProviderProfile {
            id: "relay".into(),
            name: "CC Switch 路由".into(),
            base_url: "http://127.0.0.1:15721/v1".into(),
            api_key: PROXY_MANAGED_TOKEN.into(),
            model_request_headers: BTreeMap::new(),
            cc_switch_provider_id: None,
            cc_switch_read_only: false,
            supports_remote_compaction: false,
        };

        let fetch_profile =
            provider_model_fetch_profile_from_paths(&live_profile, &home, &path).unwrap();

        assert_eq!(fetch_profile.id, live_profile.id);
        assert_eq!(fetch_profile.name, live_profile.name);
        assert_eq!(fetch_profile.base_url, "https://source.example/v1");
        assert_eq!(fetch_profile.api_key, "source-route-secret");
        assert_eq!(live_profile.api_key, PROXY_MANAGED_TOKEN);
    }

    #[test]
    fn model_fetch_keeps_the_codex_profile_when_route_takeover_is_not_live() {
        let (_directory, path, home) = fixture();
        let profile = saved_route_profile("direct");

        let fetch_profile =
            provider_model_fetch_profile_from_paths(&profile, &home, &path).unwrap();

        assert_eq!(fetch_profile, profile);
    }

    #[test]
    fn model_fetch_reads_codex_provider_tokens_and_request_headers() {
        let (_directory, path, home) = fixture();
        fs::write(
            home.join("config.toml"),
            r#"
model_provider = "direct"

[model_providers.direct]
base_url = "https://direct.example/v1"
experimental_bearer_token = "fresh-secret"

[model_providers.direct.http_headers]
X-Route = "manual"
"#,
        )
        .unwrap();
        let profile = saved_route_profile("direct");

        let fetch_profile =
            provider_model_fetch_profile_from_paths(&profile, &home, &path).unwrap();

        assert_eq!(fetch_profile.api_key, "fresh-secret");
        assert_eq!(
            fetch_profile
                .model_request_headers
                .get("X-Route")
                .map(String::as_str),
            Some("manual")
        );
    }

    #[test]
    fn provider_request_extensions_resolve_environment_values() {
        let document = r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://custom.example/v1"
env_key = "CUSTOM_API_KEY"

[model_providers.custom.http_headers]
X-Route = "static"
Authorization = " "

[model_providers.custom.env_http_headers]
X-Environment = "CUSTOM_HEADER"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let provider = document["model_providers"]["custom"]
            .as_table_like()
            .unwrap();
        let env_value = |name: &str| match name {
            "CUSTOM_API_KEY" => Some("env-secret".to_string()),
            "CUSTOM_HEADER" => Some("env-header".to_string()),
            _ => None,
        };

        assert_eq!(
            provider_config_api_key_with_env(&document, Some(provider), &env_value).as_deref(),
            Some("env-secret")
        );
        assert_eq!(
            provider_model_request_headers_with_env(provider, &env_value),
            BTreeMap::from([
                ("X-Environment".to_string(), "env-header".to_string()),
                ("X-Route".to_string(), "static".to_string()),
            ])
        );

        let no_env_key = r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://custom.example/v1"
"#
        .parse::<DocumentMut>()
        .unwrap();
        let provider = no_env_key["model_providers"]["custom"]
            .as_table_like()
            .unwrap();
        assert_eq!(
            provider_config_api_key_with_env(&no_env_key, Some(provider), &|_| {
                Some("must-not-leak".to_string())
            }),
            None
        );
    }

    #[test]
    fn cc_switch_source_prefers_provider_token_and_keeps_request_headers() {
        let settings = json!({
            "auth": {"OPENAI_API_KEY": "stale-auth-secret"},
            "config": r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://source.example/v1"
experimental_bearer_token = "provider-secret"

[model_providers.custom.http_headers]
X-Route = "source"
"#
        });

        let source = cc_switch_source_api("source", Some("custom"), &settings.to_string()).unwrap();

        assert_eq!(source.api_key, "provider-secret");
        assert_eq!(
            source
                .model_request_headers
                .get("X-Route")
                .map(String::as_str),
            Some("source")
        );
    }

    #[test]
    fn model_fetch_never_forwards_the_cc_switch_proxy_marker_to_a_source_api() {
        let (_directory, path, home) = fixture();
        install_proxy_schema(&path);
        let settings = json!({
            "auth": {"OPENAI_API_KEY": PROXY_MANAGED_TOKEN},
            "config": r#"
model_provider = "custom"

[model_providers.custom]
base_url = "https://source.example/v1"
wire_api = "responses"
"#
        });
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO providers
                 (id, app_type, name, settings_config, sort_index, is_current)
                 VALUES ('source-route', 'codex', '源线路', ?1, 0, 1)",
                [settings.to_string()],
            )
            .unwrap();
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
        let live_profile = ProviderProfile {
            base_url: "http://127.0.0.1:15721/v1".into(),
            api_key: PROXY_MANAGED_TOKEN.into(),
            ..saved_route_profile("relay")
        };

        let error = provider_model_fetch_profile_from_paths(&live_profile, &home, &path)
            .unwrap_err()
            .to_string();

        assert!(error.contains("路由占位凭据"));
        assert!(!error.contains(PROXY_MANAGED_TOKEN));
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
    fn route_takeover_recognizes_code_switch_r_loopback_without_a_cc_switch_database_marker() {
        for provider_id in CODE_SWITCH_R_PROVIDER_IDS {
            let (_directory, path, home) = fixture();
            write_live_route(&home, provider_id, "http://127.0.0.1:18100", "");

            let state = startup_route_state_from_paths(&path, &home).unwrap();
            let snapshot = state.live_route.unwrap();

            assert_eq!(
                state.takeover,
                RouteTakeoverState {
                    managed: false,
                    live: true,
                }
            );
            assert_eq!(snapshot.provider_id(), *provider_id);
            assert!(snapshot.provider.local_route);
            assert_eq!(snapshot.profile().base_url, "http://127.0.0.1:18100");
        }
    }

    #[test]
    fn code_switch_r_provider_id_requires_a_loopback_endpoint() {
        let (_directory, path, home) = fixture();
        write_live_route(
            &home,
            "code-switch-r",
            "https://relay.example/v1",
            "sk-direct",
        );

        assert_eq!(
            route_takeover_state_from_paths(&path, &home).unwrap(),
            RouteTakeoverState::default()
        );
    }

    #[test]
    fn startup_route_snapshot_rejects_chat_wire_api() {
        let (_directory, path, home) = fixture();
        install_proxy_schema(&path);
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO proxy_config (app_type, enabled) VALUES ('codex', 1)",
                [],
            )
            .unwrap();
        let config = br#"model_provider = "relay"

[model_providers.relay]
name = "Live Relay"
base_url = "https://relay.example/v1"
wire_api = "chat"
experimental_bearer_token = "sk-live"
"#;
        let auth = br#"{"OPENAI_API_KEY":"PROXY_MANAGED"}"#;
        fs::write(home.join("config.toml"), config).unwrap();
        fs::write(home.join("auth.json"), auth).unwrap();

        let error = startup_route_state_from_paths(&path, &home)
            .err()
            .expect("Chat wire API must be rejected");

        assert!(format!("{error:#}").contains("第三方网关"));
    }

    #[test]
    fn startup_route_snapshot_rejects_a_dangling_provider_before_session_repair() {
        let (_directory, path, home) = fixture();
        install_proxy_schema(&path);
        fs::write(
            home.join("config.toml"),
            "model_provider = \"codey_global\"\n",
        )
        .unwrap();
        fs::write(
            home.join("auth.json"),
            br#"{"OPENAI_API_KEY":"PROXY_MANAGED"}"#,
        )
        .unwrap();

        let error = startup_route_state_from_paths(&path, &home)
            .err()
            .unwrap()
            .to_string();

        assert!(error.contains("Provider「codey_global」不存在"));
        assert!(error.contains("避免损坏会话"));
    }

    #[test]
    fn startup_route_snapshot_rejects_a_third_party_reserved_provider_id() {
        let (_directory, path, home) = fixture();
        install_proxy_schema(&path);
        write_live_route(
            &home,
            "openai",
            "https://third-party.example/v1",
            "sk-third-party",
        );
        fs::write(
            home.join("auth.json"),
            br#"{"OPENAI_API_KEY":"PROXY_MANAGED"}"#,
        )
        .unwrap();

        let error = startup_route_state_from_paths(&path, &home)
            .err()
            .unwrap()
            .to_string();

        assert!(error.contains("Codex 保留 Provider ID「openai」"));
        assert!(error.contains("把密钥发往内置服务"));
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
