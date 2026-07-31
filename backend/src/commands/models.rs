use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    AppState, STARTUP_PROVIDER_MODEL_SYNC_TIMEOUT, redacted_config, runtime_config_requires_restart,
};
use crate::cc_switch;
use crate::cdp;
use crate::codex_config::codex_home;
use crate::config::CodeyConfig;
use crate::error_log;
use crate::model_catalog;
use crate::provider_models;

pub async fn sync_current_provider_command(state: &Arc<AppState>) -> Result<Value, String> {
    let cc_switch = sync_cc_switch_state(state).await;
    let config = state.config.read().await.clone();
    let restart_required = runtime_config_requires_restart(state, &config).await;
    let model_state = current_model_state(&config)?;
    let public_config = redacted_config(&config);
    Ok(json!({
        "status":"ok",
        "config":public_config,
        "ccSwitch":cc_switch,
        "modelState":model_state,
        "restartRequired":restart_required,
    }))
}

pub async fn sync_official_experimental_features(state: &Arc<AppState>) -> Result<Value, String> {
    let runtime = state.runtime.lock().await.clone();
    let Some(runtime) = runtime else {
        return Err("Codex 当前未运行，无法同步官方试验性功能配置".to_string());
    };
    let websocket_url = runtime.renderer_websocket_url().await;
    let experimental_features = match cdp::read_official_experimental_features(&websocket_url).await
    {
        Ok(features) => features,
        Err(error) => {
            error_log::record_failure(
                "patch_verification_failed",
                "read_official_experimental_features",
                format!("{error:#}"),
                json!({
                    "websocketUrl": websocket_url,
                }),
            );
            return Err(error.to_string());
        }
    };
    Ok(json!({
        "status": "ok",
        "experimentalFeatures": experimental_features,
    }))
}

pub async fn sync_cc_switch_state(state: &Arc<AppState>) -> cc_switch::CcSwitchStatus {
    let home = codex_home();
    sync_cc_switch_state_with(state, move |config| {
        cc_switch::sync_current_provider(&config, &home).map_err(|error| error.to_string())
    })
    .await
}

pub(super) async fn sync_cc_switch_state_with<F>(
    state: &Arc<AppState>,
    sync: F,
) -> cc_switch::CcSwitchStatus
where
    F: FnOnce(CodeyConfig) -> Result<(CodeyConfig, cc_switch::CcSwitchStatus), String>
        + Send
        + 'static,
{
    let previous = state.config.read().await.clone();
    let sync_input = previous.clone();
    let sync_result = tokio::task::spawn_blocking(move || sync(sync_input)).await;
    match sync_result {
        Ok(Ok((config, status))) => {
            if !status.changed {
                return status;
            }

            let _config_write_guard = state.config_write_lock.lock().await;
            let latest = state.config.read().await.clone();
            if latest != previous {
                let mut current = cc_switch::status_from_config(&latest);
                current.message =
                    Some("Codey 设置在同步线路期间已更新，已忽略过期的同步结果".to_string());
                return current;
            }
            if let Err(error) = state.store.save(&config) {
                let mut current = cc_switch::status_from_config(&latest);
                current.message = Some(format!("保存当前线路同步结果失败：{error}"));
                return current;
            }
            *state.config.write().await = config;
            status
        }
        Ok(Err(error)) => {
            let current = state.config.read().await.clone();
            let mut status = cc_switch::status_from_config(&current);
            status.message = Some(error);
            status
        }
        Err(error) => {
            let current = state.config.read().await.clone();
            let mut status = cc_switch::status_from_config(&current);
            status.message = Some(format!("同步当前线路任务异常退出：{error}"));
            status
        }
    }
}

pub(super) fn config_with_current_provider_models(
    config: &CodeyConfig,
    models: Vec<String>,
) -> CodeyConfig {
    let Some(provider_id) = config.current_provider_id().map(ToString::to_string) else {
        return config.clone();
    };
    let mut next = config.clone();
    next.upstream_models_by_provider.insert(provider_id, models);
    next.normalize()
}

pub(super) fn startup_model_sync_models_or_fallback(
    models: Vec<String>,
    saved_models: Option<&[String]>,
) -> (Vec<String>, bool) {
    if models.is_empty() {
        (
            saved_models
                .map(<[String]>::to_vec)
                .unwrap_or_else(model_catalog::default_official_model_slugs),
            false,
        )
    } else {
        (models, true)
    }
}

pub(super) fn preserve_selected_third_party_models(
    mut upstream_models: Vec<String>,
    selected_models: &[String],
) -> Vec<String> {
    let official_model_keys = model_catalog::default_official_model_slugs()
        .into_iter()
        .map(|model| model.to_lowercase())
        .collect::<HashSet<_>>();
    for model in selected_models {
        let model = model.trim();
        if model.is_empty()
            || official_model_keys.contains(model.to_lowercase().as_str())
            || upstream_models.iter().any(|existing| existing == model)
        {
            continue;
        }
        upstream_models.push(model.to_string());
    }
    upstream_models
}

pub(super) async fn sync_provider_models_for_launch(state: &Arc<AppState>) -> CodeyConfig {
    let config = state.config.read().await.clone();
    let Some(profile) = config.active_profile() else {
        return config;
    };
    if profile.cc_switch_read_only {
        return config;
    }
    let Some(provider_id) = config.current_provider_id().map(ToString::to_string) else {
        return config;
    };

    let (models, synced) = match tokio::time::timeout(
        STARTUP_PROVIDER_MODEL_SYNC_TIMEOUT,
        provider_models::fetch(&profile, &state.http_client),
    )
    .await
    {
        Ok(Ok(models)) => {
            let fetched_model_count = models.len();
            let (models, synced) =
                startup_model_sync_models_or_fallback(models, config.upstream_models_snapshot());
            let models = if synced {
                preserve_selected_third_party_models(models, config.selected_models())
            } else {
                models
            };
            if synced {
                eprintln!(
                    "启动时已从「{}」同步 {} 个上游模型",
                    profile.name, fetched_model_count
                );
            } else if config.upstream_models_snapshot().is_some() {
                eprintln!(
                    "启动时「{}」返回空模型列表，沿用已保存的模型支持配置",
                    profile.name
                );
            } else {
                eprintln!(
                    "启动时「{}」返回空模型列表，使用默认 7 个模型",
                    profile.name
                );
            }
            (models, synced)
        }
        Ok(Err(error)) => {
            let (models, synced) = startup_model_sync_models_or_fallback(
                Vec::new(),
                config.upstream_models_snapshot(),
            );
            if config.upstream_models_snapshot().is_some() {
                eprintln!(
                    "启动时同步「{}」上游模型失败，沿用已保存的模型支持配置：{error:#}",
                    profile.name
                );
            } else {
                eprintln!(
                    "启动时同步「{}」上游模型失败，使用默认 7 个模型：{error:#}",
                    profile.name
                );
            }
            (models, synced)
        }
        Err(_) => {
            let (models, synced) = startup_model_sync_models_or_fallback(
                Vec::new(),
                config.upstream_models_snapshot(),
            );
            if config.upstream_models_snapshot().is_some() {
                eprintln!(
                    "启动时同步「{}」上游模型超时，沿用已保存的模型支持配置",
                    profile.name
                );
            } else {
                eprintln!(
                    "启动时同步「{}」上游模型超时，使用默认 7 个模型",
                    profile.name
                );
            }
            (models, synced)
        }
    };
    let _config_write_guard = state.config_write_lock.lock().await;
    let latest = state.config.read().await.clone();
    if latest.current_provider_id() != Some(provider_id.as_str()) {
        eprintln!("启动时同步模型期间当前线路已变化，忽略旧线路的同步结果");
        return latest;
    }
    let next = config_with_current_provider_models(&latest, models);
    if synced && let Err(error) = state.store.save(&next) {
        eprintln!("保存启动时模型同步结果失败，本次启动仍使用最新模型：{error:#}");
    }
    *state.config.write().await = next.clone();
    next
}

pub async fn fetch_current_provider_models(state: &Arc<AppState>) -> Result<Value, String> {
    let config = state.config.read().await.clone();
    let profile = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id)
        .cloned()
        .ok_or_else(|| "找不到当前线路".to_string())?;
    if profile.cc_switch_read_only {
        return Err("官方线路使用官方模型目录，无需同步第三方模型".to_string());
    }
    let provider_id = config
        .current_provider_id()
        .ok_or_else(|| "当前线路缺少标识".to_string())?
        .to_string();
    let fetched_models = provider_models::fetch(&profile, &state.http_client)
        .await
        .map_err(|error| error.to_string())?;
    let _config_write_guard = state.config_write_lock.lock().await;
    let latest = state.config.read().await.clone();
    if latest.current_provider_id() != Some(provider_id.as_str()) {
        return Err("同步模型期间当前线路已变化，请重试".to_string());
    }
    let models =
        preserve_selected_third_party_models(fetched_models.clone(), latest.selected_models());
    let mut next = latest;
    next.upstream_models_by_provider
        .insert(provider_id, models.clone());
    next = next.normalize();
    let model_state = current_model_state(&next)?;
    if should_refresh_model_catalog(&model_state) {
        refresh_model_catalog(&next)?;
    }
    state.store.save(&next).map_err(|error| error.to_string())?;
    *state.config.write().await = next.clone();
    drop(_config_write_guard);
    let restart_required = runtime_config_requires_restart(state, &next).await;
    Ok(json!({
        "status":"ok",
        "models":fetched_models,
        "modelState":model_state,
        "restartRequired":restart_required,
    }))
}

pub async fn save_selected_models(
    state: &Arc<AppState>,
    requested_official_models: Vec<String>,
    requested_third_party_models: Vec<String>,
) -> Result<Value, String> {
    let _config_write_guard = state.config_write_lock.lock().await;
    let mut config = state.config.read().await.clone();
    let profile = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id)
        .ok_or_else(|| "找不到当前线路".to_string())?;
    if profile.cc_switch_read_only {
        return Err("官方线路不支持添加第三方模型".to_string());
    }
    let (supported_official, selected) = validate_manual_model_selection(
        &model_catalog::default_official_model_slugs(),
        &requested_official_models,
        &requested_third_party_models,
    )?;
    let provider_id = config
        .current_provider_id()
        .ok_or_else(|| "当前线路缺少标识".to_string())?
        .to_string();
    let supported_models =
        preserve_selected_third_party_models(supported_official, config.upstream_models());
    let supported_models = preserve_selected_third_party_models(supported_models, &selected);
    config
        .upstream_models_by_provider
        .insert(provider_id.clone(), supported_models);
    if selected.is_empty() {
        config.selected_models_by_provider.remove(&provider_id);
    } else {
        config
            .selected_models_by_provider
            .insert(provider_id, selected);
    }
    config = config.normalize();
    refresh_model_catalog(&config)?;
    state
        .store
        .save(&config)
        .map_err(|error| error.to_string())?;
    *state.config.write().await = config.clone();
    let model_state = current_model_state(&config)?;
    let public_config = redacted_config(&config);
    drop(_config_write_guard);
    let restart_required = runtime_config_requires_restart(state, &config).await;
    Ok(json!({
        "status":"ok",
        "config":public_config,
        "modelState":model_state,
        "restartRequired":restart_required,
    }))
}

pub(super) fn validate_manual_model_selection(
    official_model_ids: &[String],
    requested_official_models: &[String],
    requested_third_party_models: &[String],
) -> Result<(Vec<String>, Vec<String>), String> {
    let official_by_key = official_model_ids
        .iter()
        .map(|model| (model.to_lowercase(), model.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let requested_official = requested_official_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .map(|model| model.to_lowercase())
        .collect::<HashSet<_>>();
    if let Some(model) = requested_official
        .iter()
        .find(|model| !official_by_key.contains_key(model.as_str()))
    {
        return Err(format!("模型 {model} 不在官方模型列表中"));
    }
    let supported_official = official_model_ids
        .iter()
        .filter(|model| requested_official.contains(model.to_lowercase().as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let selected_third_party = requested_third_party_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .try_fold(Vec::<String>::new(), |mut models, model| {
            if official_by_key.contains_key(model.to_lowercase().as_str()) {
                return Err(format!(
                    "模型 {model} 已在官方模型列表中，请直接勾选，不可作为其他模型手动添加"
                ));
            }
            if !models.iter().any(|existing| existing == model) {
                models.push(model.to_string());
            }
            Ok(models)
        })?;
    Ok((supported_official, selected_third_party))
}

pub async fn save_default_model(
    state: &Arc<AppState>,
    requested_model: String,
) -> Result<Value, String> {
    let _config_write_guard = state.config_write_lock.lock().await;
    let mut config = state.config.read().await.clone();
    let requested_model = requested_model.trim();
    if requested_model.is_empty() {
        return Err("默认模型不能为空".to_string());
    }
    let model_state = current_model_state(&config)?;
    let supported = model_state
        .official_models
        .iter()
        .any(|model| model.supported && model.slug == requested_model)
        || model_state
            .third_party_models
            .iter()
            .any(|model| model == requested_model);
    if !supported {
        return Err(format!("模型 {requested_model} 当前不可用，无法设为默认"));
    }
    let provider_id = config
        .current_provider_id()
        .ok_or_else(|| "当前线路缺少标识".to_string())?
        .to_string();
    config
        .default_model_by_provider
        .insert(provider_id, requested_model.to_string());
    config = config.normalize();
    state
        .store
        .save(&config)
        .map_err(|error| error.to_string())?;
    *state.config.write().await = config.clone();
    let model_state = current_model_state(&config)?;
    let public_config = redacted_config(&config);
    drop(_config_write_guard);
    let restart_required = runtime_config_requires_restart(state, &config).await;
    Ok(json!({
        "status":"ok",
        "config":public_config,
        "modelState":model_state,
        "restartRequired":restart_required,
    }))
}

pub(super) fn current_model_state(
    config: &CodeyConfig,
) -> Result<model_catalog::ModelSelectionState, String> {
    let official = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id)
        .is_none_or(|profile| profile.cc_switch_read_only);
    model_catalog::selection_state(
        &codex_home(),
        official,
        config.upstream_models_snapshot(),
        config.selected_models(),
        config.default_model(),
    )
    .map_err(|error| error.to_string())
}

pub(super) fn current_renderer_model_catalog(config: &CodeyConfig) -> Result<Value, String> {
    let model_state = current_model_state(config)?;
    Ok(renderer_model_catalog_value(config, &model_state))
}

pub(super) fn renderer_model_catalog_value(
    config: &CodeyConfig,
    model_state: &model_catalog::ModelSelectionState,
) -> Value {
    let models = model_state
        .official_models
        .iter()
        .filter(|model| model.supported)
        .map(|model| model.slug.clone())
        .chain(model_state.third_party_models.iter().cloned())
        .collect::<Vec<_>>();
    let active_profile = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id);
    let provider_id = config.current_provider_id().unwrap_or_default().trim();
    let provider_name = active_profile
        .map(|profile| profile.name.trim())
        .filter(|name| !name.is_empty())
        .unwrap_or(provider_id);
    json!({
        "status": if models.is_empty() { "not_configured" } else { "ok" },
        "model": model_state.default_model,
        "default_model": model_state.default_model,
        "model_provider": provider_id,
        "provider_name": provider_name,
        "models": models,
        "sources": [],
        "responses_api": {
            "status": "unknown",
            "message": ""
        }
    })
}

pub(super) fn should_refresh_model_catalog(
    model_state: &model_catalog::ModelSelectionState,
) -> bool {
    !model_state.official_models.is_empty() || !model_state.third_party_models.is_empty()
}

pub(super) fn refresh_model_catalog(config: &CodeyConfig) -> Result<(), String> {
    let official = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id)
        .is_none_or(|profile| profile.cc_switch_read_only);
    model_catalog::refresh_for_provider(
        &codex_home(),
        official,
        config.upstream_models_snapshot(),
        config.selected_models(),
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}
