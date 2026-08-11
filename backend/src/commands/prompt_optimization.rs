use std::sync::{Arc, OnceLock};

use reqwest::Client;
use serde_json::{Value, json};

use super::AppState;
use crate::config::PromptOptimizationConfig;
use crate::error_log;
use crate::prompt_optimization;

static OPTIMIZER_CLIENT: OnceLock<Client> = OnceLock::new();

fn optimizer_client() -> Result<&'static Client, String> {
    if let Some(client) = OPTIMIZER_CLIENT.get() {
        return Ok(client);
    }
    let client = prompt_optimization::optimizer_http_client()?;
    // Concurrent callers may build a duplicate client; the first successful
    // one wins and the rest reuse it.
    Ok(OPTIMIZER_CLIENT.get_or_init(|| client))
}

pub async fn optimize_prompt_command(state: &Arc<AppState>, text: String) -> Result<Value, String> {
    let config = state.config.read().await.clone();
    let optimization = config.prompt_optimization;
    if !optimization.enabled {
        return Err("提示词优化尚未启用，请先在 Codey 控制台开启".to_string());
    }
    let client = optimizer_client()?;
    match prompt_optimization::optimize_prompt(client, &optimization, &text).await {
        Ok(optimized) => Ok(json!({"optimized": optimized})),
        Err(error) => {
            error_log::record_failure(
                "prompt_optimization_failed",
                "optimize_prompt",
                error.clone(),
                json!({
                    "model": optimization.model.trim(),
                    "endpoint": optimization.base_url.trim(),
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
    let client = optimizer_client()?;
    match prompt_optimization::fetch_models(client, &optimization).await {
        Ok(models) => Ok(json!({"models": models})),
        Err(error) => {
            error_log::record_failure(
                "prompt_optimization_models_failed",
                "fetch_prompt_optimization_models",
                error.clone(),
                json!({
                    "endpoint": optimization.base_url.trim(),
                }),
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
    let client = optimizer_client()?;
    match prompt_optimization::test_configuration(client, &optimization).await {
        Ok(result) => Ok(json!({"status": "ok", "result": result})),
        Err(error) => {
            error_log::record_failure(
                "prompt_optimization_test_failed",
                "test_prompt_optimization",
                error.clone(),
                json!({
                    "model": optimization.model.trim(),
                    "endpoint": optimization.base_url.trim(),
                }),
            );
            Err(error)
        }
    }
}
