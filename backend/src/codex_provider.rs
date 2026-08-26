use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use codey_runtime_core::config_manager::ConfigManager;
use serde::Serialize;
use serde_json::Value;
use toml_edit::{DocumentMut, Item, TableLike};

use crate::codex_config::BUILTIN_OPENAI_PROVIDER_ID;
use crate::config::{CodeyConfig, DERIVED_OFFICIAL_PROFILE_ID, ProviderProfile};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CurrentProvider {
    pub id: String,
    pub name: String,
    pub official: bool,
    pub supports_remote_compaction: bool,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub changed: bool,
    pub provider: CurrentProvider,
}

struct ProviderRequestExtensions {
    api_key: Option<String>,
    headers: BTreeMap<String, String>,
}

struct LocalProviderSnapshot {
    provider: CurrentProvider,
    api_key: String,
    upstream_protocol: String,
    official_account_available: bool,
}

pub fn current_official_account_profile(codex_home: &Path) -> Result<Option<ProviderProfile>> {
    let snapshot = local_provider(codex_home)?;
    if !snapshot.official_account_available || !snapshot.provider.official {
        return Ok(None);
    }
    let provider_id = snapshot.provider.id.clone();
    let mut profile = profile_from_provider(
        &snapshot.provider,
        String::new(),
        &snapshot.upstream_protocol,
    );
    profile.id = DERIVED_OFFICIAL_PROFILE_ID.to_string();
    profile.source_provider_id = Some(provider_id);
    profile.normalize();
    Ok(Some(profile))
}

pub fn current_provider(codex_home: &Path) -> Result<CurrentProvider> {
    Ok(local_provider(codex_home)?.provider)
}

pub fn provider_model_fetch_profile(
    profile: &ProviderProfile,
    codex_home: &Path,
) -> Result<ProviderProfile> {
    let mut fetch_profile = profile.clone();
    if let Some(extensions) = local_provider_model_request_extensions(codex_home, profile)? {
        if let Some(api_key) = extensions.api_key {
            fetch_profile.api_key = api_key;
        }
        fetch_profile.model_request_headers = extensions.headers;
    }
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
    let provider_id = active_provider_id(document);
    if provider_id != profile.provider_id() {
        return Ok(None);
    }
    let provider = provider_table(document, provider_id);
    Ok(provider.map(|provider| ProviderRequestExtensions {
        api_key: provider_config_api_key(document, Some(provider)),
        headers: provider_model_request_headers(provider),
    }))
}

#[cfg(test)]
pub fn sync_current_provider(
    config: &CodeyConfig,
    codex_home: &Path,
) -> Result<(CodeyConfig, ProviderStatus)> {
    let snapshot = local_provider(codex_home)?;
    sync_provider_profile(
        config,
        snapshot.provider,
        snapshot.api_key,
        &snapshot.upstream_protocol,
    )
}

pub fn sync_current_third_party_provider(
    config: &CodeyConfig,
    codex_home: &Path,
) -> Result<(CodeyConfig, ProviderStatus)> {
    let snapshot = local_provider(codex_home)?;
    if snapshot.provider.official {
        bail!("当前 Codex 配置是官方账号线路，不自动导入为第三方线路");
    }
    sync_provider_profile(
        config,
        snapshot.provider,
        snapshot.api_key,
        &snapshot.upstream_protocol,
    )
}

fn sync_provider_profile(
    config: &CodeyConfig,
    provider: CurrentProvider,
    api_key: String,
    upstream_protocol: &str,
) -> Result<(CodeyConfig, ProviderStatus)> {
    let profile = profile_from_provider(&provider, api_key, upstream_protocol);
    let mut next = config.clone();
    let imported_id = profile.id.clone();
    let imported_provider_id = profile.provider_id().to_string();
    let mut active_profile_id = imported_id.clone();
    let replace_placeholder =
        next.profiles.len() == 1 && next.profiles[0].is_unconfigured_default();
    if replace_placeholder {
        let placeholder_provider_id = next.profiles[0].provider_id().to_string();
        next.profiles = vec![profile];
        next.selected_models_by_provider
            .remove(&placeholder_provider_id);
        next.manual_third_party_models_by_provider
            .remove(&placeholder_provider_id);
        next.declared_official_models_by_provider
            .remove(&placeholder_provider_id);
        next.upstream_models_by_provider
            .remove(&placeholder_provider_id);
    } else if let Some(existing) = next.profiles.iter_mut().find(|existing| {
        existing.provider_id() == imported_provider_id || existing.id == imported_id
    }) {
        // Keep the Codey UI identity stable when a previously imported route
        // has a different runtime provider id.
        active_profile_id = existing.id.clone();
        let mut replacement = profile;
        replacement.short_name.clone_from(&existing.short_name);
        if replacement.id != active_profile_id {
            replacement.id = active_profile_id.clone();
            replacement.source_provider_id = Some(imported_provider_id);
        }
        *existing = replacement;
    } else {
        next.profiles.push(profile);
    }
    next.active_profile_id = active_profile_id;
    next.initial_route_import_completed = true;
    next = next.normalize();
    let changed = &next != config;
    if changed {
        next.settings_revision = config.settings_revision.saturating_add(1);
    }
    Ok((next, ProviderStatus { changed, provider }))
}

pub fn status_from_config(config: &CodeyConfig) -> ProviderStatus {
    let profile = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id)
        .or_else(|| config.profiles.first());
    let provider = profile
        .map(|profile| CurrentProvider {
            id: profile.id.clone(),
            name: profile.name.clone(),
            official: profile.official_account,
            supports_remote_compaction: profile.supports_remote_compaction,
            base_url: profile.base_url.clone(),
        })
        .unwrap_or_else(|| CurrentProvider {
            id: BUILTIN_OPENAI_PROVIDER_ID.to_string(),
            name: "OpenAI 官方直登".to_string(),
            official: true,
            supports_remote_compaction: true,
            base_url: String::new(),
        });
    ProviderStatus {
        changed: false,
        provider,
    }
}

fn profile_from_provider(
    provider: &CurrentProvider,
    api_key: String,
    upstream_protocol: &str,
) -> ProviderProfile {
    ProviderProfile {
        id: provider.id.clone(),
        name: provider.name.clone(),
        short_name: String::new(),
        base_url: provider.base_url.clone(),
        api_key,
        upstream_protocol: if provider.official {
            crate::config::UPSTREAM_PROTOCOL_OFFICIAL.to_string()
        } else {
            upstream_protocol.to_string()
        },
        auth_mode: if provider.official {
            crate::config::AUTH_MODE_OFFICIAL_ACCOUNT.to_string()
        } else {
            crate::config::AUTH_MODE_API_KEY.to_string()
        },
        api_key_configured: !provider.official,
        clear_api_key: false,
        model_request_headers: BTreeMap::new(),
        source_provider_id: None,
        official_account: provider.official,
        supports_remote_compaction: provider.supports_remote_compaction,
    }
}

fn local_provider(codex_home: &Path) -> Result<LocalProviderSnapshot> {
    let config_path = codex_home.join("config.toml");
    let config = ConfigManager::new(&config_path).load()?;
    let document = config.document();
    let provider_id = active_provider_id(document);
    let table = provider_table(document, provider_id);
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
    let upstream_protocol = upstream_protocol_from_wire_api(wire_api)?;
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
    let official_endpoint = base_url.is_empty() || is_official_base_url(&base_url);
    let official_account_available = official_endpoint
        && config_api_key.is_none()
        && auth.as_ref().is_some_and(auth_has_chatgpt_tokens);
    // A provider-scoped token describes the active route and must win over a
    // long-lived auth.json login retained alongside it.
    let api_key = config_api_key
        .or_else(|| auth_api_key.map(ToString::to_string))
        .unwrap_or_default();
    let official = official_endpoint && (auth_mode == Some("chatgpt") || api_key.is_empty());
    if !official && base_url.is_empty() {
        base_url = "https://api.openai.com/v1".to_string();
    }
    Ok(LocalProviderSnapshot {
        provider: CurrentProvider {
            id: provider_id.to_string(),
            name: if official {
                "OpenAI 官方直登".to_string()
            } else if name == BUILTIN_OPENAI_PROVIDER_ID {
                "OpenAI API".to_string()
            } else {
                name.to_string()
            },
            official,
            supports_remote_compaction: official || name == "OpenAI",
            base_url,
        },
        api_key: if official { String::new() } else { api_key },
        upstream_protocol: if official {
            crate::config::UPSTREAM_PROTOCOL_OFFICIAL.to_string()
        } else {
            upstream_protocol.to_string()
        },
        official_account_available,
    })
}

fn active_provider_id(document: &DocumentMut) -> &str {
    document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(BUILTIN_OPENAI_PROVIDER_ID)
}

fn provider_table<'a>(document: &'a DocumentMut, provider_id: &str) -> Option<&'a dyn TableLike> {
    document
        .get("model_providers")
        .and_then(Item::as_table_like)
        .and_then(|providers| providers.get(provider_id))
        .and_then(Item::as_table_like)
}

fn auth_has_chatgpt_tokens(auth: &Value) -> bool {
    auth.get("auth_mode").and_then(Value::as_str) == Some("chatgpt")
        && auth
            .get("tokens")
            .and_then(Value::as_object)
            .is_some_and(|tokens| {
                ["access_token", "id_token"].iter().any(|name| {
                    tokens
                        .get(*name)
                        .and_then(Value::as_str)
                        .is_some_and(|token| !token.trim().is_empty())
                })
            })
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

fn upstream_protocol_from_wire_api(value: &str) -> Result<&'static str> {
    let value = value.trim().to_ascii_lowercase();
    if value.contains("anthropic") || value == "messages" || value.ends_with("/messages") {
        return Ok(crate::config::UPSTREAM_PROTOCOL_ANTHROPIC_MESSAGES);
    }
    if value.contains("chat") {
        return Ok(crate::config::UPSTREAM_PROTOCOL_OPENAI_CHAT_COMPLETIONS);
    }
    if value.is_empty() || value.contains("response") {
        return Ok(crate::config::UPSTREAM_PROTOCOL_OPENAI_RESPONSES);
    }
    bail!("Codex Provider 使用了 Codey 不支持的 wire_api：{value}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use tempfile::TempDir;
    use toml_edit::DocumentMut;

    fn write_config(home: &Path, contents: &str) {
        fs::write(home.join("config.toml"), contents).unwrap();
    }

    fn write_auth(home: &Path, value: Value) {
        fs::write(
            home.join("auth.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    fn third_party_config(wire_api: &str) -> String {
        format!(
            r#"model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "{wire_api}"
experimental_bearer_token = "sk-relay"
"#,
        )
    }

    #[test]
    fn rejects_malformed_codex_files() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), "not = [valid");
        assert!(current_provider(home.path()).is_err());

        write_config(home.path(), "");
        fs::write(home.path().join("auth.json"), b"{").unwrap();
        assert!(current_provider(home.path()).is_err());
    }

    #[test]
    fn imports_supported_wire_protocols() {
        for (wire_api, expected) in [
            (
                "chat_completions",
                crate::config::UPSTREAM_PROTOCOL_OPENAI_CHAT_COMPLETIONS,
            ),
            (
                "anthropic/messages",
                crate::config::UPSTREAM_PROTOCOL_ANTHROPIC_MESSAGES,
            ),
            (
                "responses",
                crate::config::UPSTREAM_PROTOCOL_OPENAI_RESPONSES,
            ),
        ] {
            let home = TempDir::new().unwrap();
            write_config(home.path(), &third_party_config(wire_api));
            let (config, status) =
                sync_current_third_party_provider(&CodeyConfig::default(), home.path()).unwrap();
            assert_eq!(config.profiles[0].upstream_protocol, expected);
            assert_eq!(status.provider.id, "relay");
        }
    }

    #[test]
    fn official_capability_requires_tokens_on_the_active_official_endpoint() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), "");
        write_auth(
            home.path(),
            serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": { "access_token": "token" }
            }),
        );
        let profile = current_official_account_profile(home.path())
            .unwrap()
            .unwrap();
        assert!(profile.official_account);
        assert_eq!(profile.provider_id(), "openai");
        assert!(profile.api_key.is_empty());

        write_config(home.path(), &third_party_config("responses"));
        assert!(
            current_official_account_profile(home.path())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn provider_token_wins_over_retained_auth_key() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), &third_party_config("responses"));
        write_auth(
            home.path(),
            serde_json::json!({
                "auth_mode": "chatgpt",
                "OPENAI_API_KEY": "stale-auth-key",
                "tokens": { "access_token": "retained-token" }
            }),
        );
        let (config, _) = sync_current_provider(&CodeyConfig::default(), home.path()).unwrap();
        assert_eq!(config.profiles[0].api_key, "sk-relay");
        assert!(!config.profiles[0].official_account);
    }

    #[test]
    fn synchronization_upserts_without_removing_saved_routes() {
        let home = TempDir::new().unwrap();
        write_config(home.path(), &third_party_config("responses"));
        let mut saved = ProviderProfile::new("Saved");
        saved.id = "saved".into();
        saved.base_url = "https://saved.example/v1".into();
        saved.api_key = "sk-saved".into();
        saved.normalize();
        let config = CodeyConfig {
            profiles: vec![saved],
            active_profile_id: "saved".into(),
            initial_route_import_completed: true,
            ..CodeyConfig::default()
        };
        let (synced, _) = sync_current_third_party_provider(&config, home.path()).unwrap();
        assert_eq!(synced.profiles.len(), 2);
        assert!(synced.profiles.iter().any(|profile| profile.id == "saved"));
        assert!(synced.profiles.iter().any(|profile| profile.id == "relay"));
    }

    #[test]
    fn synchronization_preserves_an_existing_route_short_name() {
        let mut saved = ProviderProfile::new("Relay");
        saved.id = "relay".into();
        saved.short_name = "中".into();
        saved.base_url = "https://old.example/v1".into();
        saved.api_key = "old-key".into();
        saved.normalize();
        let config = CodeyConfig {
            profiles: vec![saved],
            active_profile_id: "relay".into(),
            initial_route_import_completed: true,
            ..CodeyConfig::default()
        };

        let (synced, _) = sync_provider_profile(
            &config,
            CurrentProvider {
                id: "relay".into(),
                name: "Relay Updated".into(),
                official: false,
                supports_remote_compaction: false,
                base_url: "https://new.example/v1".into(),
            },
            "new-key".into(),
            crate::config::UPSTREAM_PROTOCOL_OPENAI_RESPONSES,
        )
        .unwrap();

        assert_eq!(synced.profiles[0].short_name, "中");
        assert_eq!(synced.profiles[0].name, "Relay Updated");
    }

    #[test]
    fn model_fetch_uses_active_provider_key_and_headers() {
        let home = TempDir::new().unwrap();
        write_config(
            home.path(),
            r#"model_provider = "relay"

[model_providers.relay]
name = "Relay"
base_url = "https://relay.example/v1"
wire_api = "responses"
experimental_bearer_token = "fresh-key"
http_headers = { X-Static = "static-value" }
env_http_headers = { X-Dynamic = "DYNAMIC_HEADER" }
"#,
        );
        let mut profile = ProviderProfile::new("Relay");
        profile.id = "relay".into();
        profile.base_url = "https://relay.example/v1".into();
        profile.api_key = "old-key".into();

        let document = DocumentMut::from_str(
            r#"http_headers = { X-Static = "static-value" }
env_http_headers = { X-Dynamic = "DYNAMIC_HEADER" }
"#,
        )
        .unwrap();
        let table = document.as_table();
        let headers = provider_model_request_headers_with_env(table, &|name| {
            (name == "DYNAMIC_HEADER").then(|| "dynamic-value".to_string())
        });
        assert_eq!(headers["X-Static"], "static-value");
        assert_eq!(headers["X-Dynamic"], "dynamic-value");

        let fetch = provider_model_fetch_profile(&profile, home.path()).unwrap();
        assert_eq!(fetch.api_key, "fresh-key");
        assert_eq!(fetch.model_request_headers["X-Static"], "static-value");
    }

    #[test]
    fn environment_provider_keys_are_supported() {
        let document = DocumentMut::from_str(
            r#"[model_providers.relay]
env_key = "RELAY_TOKEN"
"#,
        )
        .unwrap();
        let provider = provider_table(&document, "relay").unwrap();
        let key = provider_config_api_key_with_env(&document, Some(provider), &|name| {
            (name == "RELAY_TOKEN").then(|| "env-secret".to_string())
        });
        assert_eq!(key.as_deref(), Some("env-secret"));
    }
}
