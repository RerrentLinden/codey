use std::sync::{Arc, OnceLock};
use std::{fs, path::Path};

use reqwest::Client;
use serde_json::{Value, json};
use uuid::Uuid;

use super::AppState;
use crate::cc_switch;
use crate::codex_config::codex_home;
use crate::config::{CodeyConfig, PromptOptimizationConfig, ProviderProfile};
use crate::error_log;
use crate::prompt_optimization;

static OPTIMIZER_CLIENT: OnceLock<Client> = OnceLock::new();
const CODEX_INSTALLATION_ID_HEADER: &str = "x-codex-installation-id";
const CODEX_INSTALLATION_ID_FILE: &str = "installation_id";

fn optimizer_client() -> Result<&'static Client, String> {
    if let Some(client) = OPTIMIZER_CLIENT.get() {
        return Ok(client);
    }
    let client = prompt_optimization::optimizer_http_client()?;
    // Concurrent callers may build a duplicate client; the first successful
    // one wins and the rest reuse it.
    Ok(OPTIMIZER_CLIENT.get_or_init(|| client))
}

async fn current_provider_request_profile(config: &CodeyConfig) -> Result<ProviderProfile, String> {
    let profile = config
        .active_profile()
        .ok_or_else(|| "找不到当前 GPT API 线路".to_string())?;
    if profile.cc_switch_read_only {
        return Err("当前为 ChatGPT 官方登录线路，仅第三方线路可以同步 API 配置".to_string());
    }
    let home = codex_home();
    tokio::task::spawn_blocking(move || {
        cc_switch::provider_model_fetch_profile(&profile, home)
            .map_err(|error| format!("读取当前 GPT API 配置失败：{error:#}"))
    })
    .await
    .map_err(|error| format!("读取当前 GPT API 配置任务异常退出：{error}"))?
}

fn resolve_request_config(
    optimization: &PromptOptimizationConfig,
) -> Result<prompt_optimization::ResolvedPromptOptimizationConfig, String> {
    resolve_request_config_at(optimization, codex_home())
}

fn resolve_request_config_at(
    optimization: &PromptOptimizationConfig,
    codex_home: &Path,
) -> Result<prompt_optimization::ResolvedPromptOptimizationConfig, String> {
    if optimization.api_key.trim().is_empty() {
        return Err("请先配置 API Key".to_string());
    }
    let mut resolved =
        prompt_optimization::ResolvedPromptOptimizationConfig::from_custom(optimization);
    if let Some(installation_id) = read_codex_installation_id(codex_home) {
        resolved
            .request_headers
            .insert(CODEX_INSTALLATION_ID_HEADER.to_string(), installation_id);
    }
    Ok(resolved)
}

fn read_codex_installation_id(codex_home: &Path) -> Option<String> {
    let value = fs::read_to_string(codex_home.join(CODEX_INSTALLATION_ID_FILE)).ok()?;
    Uuid::parse_str(value.trim())
        .ok()
        .map(|installation_id| installation_id.to_string())
}

fn apply_current_provider_to_prompt_optimization(
    mut config: CodeyConfig,
    profile: ProviderProfile,
) -> Result<CodeyConfig, String> {
    let base_url = profile.normalized_base_url();
    if base_url.is_empty() {
        return Err("当前第三方线路没有可同步的 API 地址".to_string());
    }
    let api_key = profile.api_key.trim();
    if api_key.is_empty() {
        return Err("当前第三方线路没有可同步的 API Key，请手动填写".to_string());
    }
    let default_model = config
        .default_model()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string);
    config.prompt_optimization.base_url = base_url;
    config.prompt_optimization.api_key = api_key.to_string();
    config.prompt_optimization.api_key_configured = true;
    config.prompt_optimization.clear_api_key = false;
    if let Some(default_model) = default_model {
        config.prompt_optimization.model = default_model;
    }
    Ok(config.normalize())
}

fn apply_prompt_optimization_draft(
    config: &mut CodeyConfig,
    draft: Option<PromptOptimizationConfig>,
) {
    let Some(mut draft) = draft else {
        return;
    };
    draft.merge_redacted_secrets(&config.prompt_optimization);
    config.prompt_optimization.enabled = draft.enabled;
    config.prompt_optimization.instruction = draft.instruction;
}

pub async fn sync_prompt_optimization_current_provider_command(
    state: &Arc<AppState>,
    draft: Option<PromptOptimizationConfig>,
) -> Result<Value, String> {
    let _config_write_guard = state.config_write_lock.lock().await;
    let mut previous = state.config.read().await.clone();
    apply_prompt_optimization_draft(&mut previous, draft);
    let profile = current_provider_request_profile(&previous).await?;
    let mut config = apply_current_provider_to_prompt_optimization(previous.clone(), profile)?;
    config.prompt_optimization.validate()?;
    config.settings_revision = previous.settings_revision.saturating_add(1);
    super::save_config_to_store(state, &config).await?;
    *state.config.write().await = config.clone();
    Ok(json!({ "config": super::redacted_config(&config) }))
}

pub async fn optimize_prompt_command(state: &Arc<AppState>, text: String) -> Result<Value, String> {
    let config = state.config.read().await.clone();
    let optimization = config.prompt_optimization.clone();
    if !optimization.enabled {
        return Err("提示词优化尚未启用，请先在 Codey 控制台开启".to_string());
    }
    let request_config = resolve_request_config(&optimization)?;
    let client = optimizer_client()?;
    match prompt_optimization::optimize_prompt_resolved(client, &request_config, &text).await {
        Ok(optimized) => Ok(json!({"optimized": optimized})),
        Err(error) => {
            error_log::record_failure(
                "prompt_optimization_failed",
                "optimize_prompt",
                error.clone(),
                json!({
                    "model": optimization.model.trim(),
                    "apiSource": "configured",
                }),
            );
            Err(error)
        }
    }
}

/// Fetches the model list advertised by the configured service for the
/// console picker. Accepts an unsaved draft like the connectivity test, with
/// the saved key restored for the request.
pub async fn fetch_prompt_optimization_models_command(
    state: &Arc<AppState>,
    draft: Option<PromptOptimizationConfig>,
) -> Result<Value, String> {
    let config = state.config.read().await.clone();
    let mut optimization = draft.unwrap_or_else(|| config.prompt_optimization.clone());
    optimization.merge_redacted_secrets(&config.prompt_optimization);
    optimization.validate()?;
    let request_config = resolve_request_config(&optimization)?;
    let client = optimizer_client()?;
    let models = prompt_optimization::fetch_models_resolved(client, &request_config).await;
    match models {
        Ok(models) => Ok(json!({"models": models})),
        Err(error) => {
            error_log::record_failure(
                "prompt_optimization_models_failed",
                "fetch_prompt_optimization_models",
                error.clone(),
                json!({ "apiSource": "configured" }),
            );
            Err(error)
        }
    }
}

/// Tests connectivity against the saved configuration, or against an
/// unsaved draft passed by the console. Draft API keys arrive redacted, so
/// the saved key is restored through the same merge used by the save path
/// before the request is sent.
pub async fn test_prompt_optimization_command(
    state: &Arc<AppState>,
    draft: Option<PromptOptimizationConfig>,
) -> Result<Value, String> {
    let config = state.config.read().await.clone();
    let mut optimization = draft.unwrap_or_else(|| config.prompt_optimization.clone());
    optimization.merge_redacted_secrets(&config.prompt_optimization);
    optimization.validate()?;
    let request_config = resolve_request_config(&optimization)?;
    let client = optimizer_client()?;
    match prompt_optimization::test_configuration_resolved(client, &request_config).await {
        Ok(result) => Ok(json!({"status": "ok", "result": result})),
        Err(error) => {
            error_log::record_failure(
                "prompt_optimization_test_failed",
                "test_prompt_optimization",
                error.clone(),
                json!({
                    "model": optimization.model.trim(),
                    "apiSource": "configured",
                }),
            );
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn resolved_prompt_optimization_uses_codex_installation_id_header() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CODEX_INSTALLATION_ID_FILE),
            " 49A95816-9EAD-4F14-B008-1D0CBAA3C328\n",
        )
        .unwrap();
        let optimization = PromptOptimizationConfig {
            api_key: "sk-test".to_string(),
            ..PromptOptimizationConfig::default()
        };

        let resolved = resolve_request_config_at(&optimization, directory.path()).unwrap();

        assert_eq!(
            resolved
                .request_headers
                .get(CODEX_INSTALLATION_ID_HEADER)
                .map(String::as_str),
            Some("49a95816-9ead-4f14-b008-1d0cbaa3c328")
        );
    }

    #[test]
    fn invalid_codex_installation_id_is_not_forwarded() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(CODEX_INSTALLATION_ID_FILE),
            "not-an-installation-id",
        )
        .unwrap();
        let optimization = PromptOptimizationConfig {
            api_key: "sk-test".to_string(),
            ..PromptOptimizationConfig::default()
        };

        let resolved = resolve_request_config_at(&optimization, directory.path()).unwrap();

        assert!(
            !resolved
                .request_headers
                .contains_key(CODEX_INSTALLATION_ID_HEADER)
        );
    }

    #[test]
    fn current_provider_sync_copies_endpoint_key_and_default_model() {
        let profile = ProviderProfile {
            id: "provider".to_string(),
            name: "Provider".to_string(),
            base_url: " https://provider.example/v1/ ".to_string(),
            api_key: "api-secret".to_string(),
            upstream_protocol: crate::config::UPSTREAM_PROTOCOL_OPENAI_RESPONSES.to_string(),
            auth_mode: crate::config::AUTH_MODE_API_KEY.to_string(),
            api_key_configured: true,
            clear_api_key: false,
            model_request_headers: BTreeMap::new(),
            cc_switch_provider_id: None,
            cc_switch_read_only: false,
            supports_remote_compaction: false,
        };
        let config = CodeyConfig {
            active_profile_id: "provider".to_string(),
            profiles: vec![profile.clone()],
            default_model_by_provider: BTreeMap::from([(
                "provider".to_string(),
                "gpt-provider".to_string(),
            )]),
            prompt_optimization: PromptOptimizationConfig {
                enabled: true,
                instruction: "保持简洁".to_string(),
                ..PromptOptimizationConfig::default()
            },
            ..CodeyConfig::default()
        };

        let synced = apply_current_provider_to_prompt_optimization(config, profile).unwrap();

        assert_eq!(
            synced.prompt_optimization.base_url,
            "https://provider.example/v1"
        );
        assert_eq!(synced.prompt_optimization.api_key, "api-secret");
        assert!(synced.prompt_optimization.api_key_configured);
        assert_eq!(synced.prompt_optimization.model, "gpt-provider");
        assert_eq!(synced.prompt_optimization.instruction, "保持简洁");
    }

    #[test]
    fn current_provider_sync_preserves_unsaved_toggle_and_instruction() {
        let profile = ProviderProfile {
            id: "provider".to_string(),
            name: "Provider".to_string(),
            base_url: "https://provider.example/v1".to_string(),
            api_key: "api-secret".to_string(),
            upstream_protocol: crate::config::UPSTREAM_PROTOCOL_OPENAI_RESPONSES.to_string(),
            auth_mode: crate::config::AUTH_MODE_API_KEY.to_string(),
            api_key_configured: true,
            clear_api_key: false,
            model_request_headers: BTreeMap::new(),
            cc_switch_provider_id: None,
            cc_switch_read_only: false,
            supports_remote_compaction: false,
        };
        let mut config = CodeyConfig {
            active_profile_id: "provider".to_string(),
            profiles: vec![profile.clone()],
            default_model_by_provider: BTreeMap::from([(
                "provider".to_string(),
                "gpt-provider".to_string(),
            )]),
            ..CodeyConfig::default()
        };
        let draft = PromptOptimizationConfig {
            enabled: true,
            instruction: "保留草稿指令".to_string(),
            base_url: "https://draft.example/v1".to_string(),
            model: "draft-model".to_string(),
            ..PromptOptimizationConfig::default()
        };

        apply_prompt_optimization_draft(&mut config, Some(draft));
        let synced = apply_current_provider_to_prompt_optimization(config, profile).unwrap();

        assert!(synced.prompt_optimization.enabled);
        assert_eq!(synced.prompt_optimization.instruction, "保留草稿指令");
        assert_eq!(
            synced.prompt_optimization.base_url,
            "https://provider.example/v1"
        );
        assert_eq!(synced.prompt_optimization.model, "gpt-provider");
    }
}
