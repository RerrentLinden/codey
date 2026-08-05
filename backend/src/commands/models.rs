use std::collections::HashSet;
use std::sync::Arc;

use codey_runtime_core::settings::{RelayMode, RelayProfile};
use serde_json::{Value, json};

use super::{
    AppState, STARTUP_PROVIDER_MODEL_SYNC_TIMEOUT, redacted_config, runtime_config_requires_restart,
};
use crate::cc_switch;
use crate::cdp;
use crate::codex_config::codex_home;
use crate::config::{CodeyConfig, ProviderProfile};
use crate::error_log;
use crate::model_catalog;
use crate::provider_models;

#[derive(Default)]
struct ModelHotReloadOutcome {
    reloaded: bool,
    error: Option<String>,
}

impl ModelHotReloadOutcome {
    fn add_to_response(self, mut response: Value) -> Value {
        if let Some(object) = response.as_object_mut() {
            object.insert("modelHotReloaded".into(), Value::Bool(self.reloaded));
            if let Some(error) = self.error {
                object.insert("modelHotReloadError".into(), Value::String(error));
            }
        }
        response
    }
}

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

#[cfg(test)]
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

fn selected_models_not_in_upstream(
    selected_models: &[String],
    upstream_models: &[String],
) -> Vec<String> {
    let official_model_keys = model_catalog::default_official_model_slugs()
        .into_iter()
        .map(|model| model.to_lowercase())
        .collect::<HashSet<_>>();
    let upstream_model_keys = upstream_models
        .iter()
        .map(|model| model.trim().to_lowercase())
        .collect::<HashSet<_>>();
    selected_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| {
            !model.is_empty()
                && !official_model_keys.contains(model.to_lowercase().as_str())
                && !upstream_model_keys.contains(model.to_lowercase().as_str())
        })
        .fold(Vec::<String>::new(), |mut models, model| {
            if !models.iter().any(|existing| existing == model) {
                models.push(model.to_string());
            }
            models
        })
}

fn config_with_current_provider_model_sync(
    config: &CodeyConfig,
    provider_models: Vec<String>,
    synced: bool,
) -> CodeyConfig {
    let Some(provider_id) = config.current_provider_id().map(ToString::to_string) else {
        return config.clone();
    };
    let manual_models = if synced {
        selected_models_not_in_upstream(config.selected_models(), &provider_models)
    } else {
        preserve_selected_third_party_models(Vec::new(), config.manual_third_party_models())
    };
    let supported_models = if synced {
        preserve_selected_third_party_models(provider_models, config.selected_models())
    } else {
        provider_models
    };
    let mut next = config.clone();
    next.upstream_models_by_provider
        .insert(provider_id.clone(), supported_models);
    if manual_models.is_empty() {
        next.manual_third_party_models_by_provider
            .remove(&provider_id);
    } else {
        next.manual_third_party_models_by_provider
            .insert(provider_id, manual_models);
    }
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
    preserve_selected_third_party_models_except(
        &mut upstream_models,
        selected_models,
        &HashSet::new(),
    );
    upstream_models
}

pub(super) fn preserve_selected_third_party_models_except(
    upstream_models: &mut Vec<String>,
    selected_models: &[String],
    deleted_model_keys: &HashSet<String>,
) {
    let official_model_keys = model_catalog::default_official_model_slugs()
        .into_iter()
        .map(|model| model.to_lowercase())
        .collect::<HashSet<_>>();
    for model in selected_models {
        let model = model.trim();
        let key = model.to_lowercase();
        if model.is_empty()
            || official_model_keys.contains(key.as_str())
            || deleted_model_keys.contains(key.as_str())
            || upstream_models.iter().any(|existing| existing == model)
        {
            continue;
        }
        upstream_models.push(model.to_string());
    }
}

async fn fetch_provider_models(
    profile: ProviderProfile,
    http_client: &reqwest::Client,
) -> anyhow::Result<Vec<String>> {
    let home = codex_home();
    let fetch_profile = tokio::task::spawn_blocking(move || {
        cc_switch::provider_model_fetch_profile(&profile, &home)
    })
    .await
    .map_err(|error| anyhow::anyhow!("解析模型源 API 配置任务异常退出：{error}"))??;
    provider_models::fetch(&fetch_profile, http_client).await
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
        fetch_provider_models(profile.clone(), &state.http_client),
    )
    .await
    {
        Ok(Ok(models)) => {
            let fetched_model_count = models.len();
            let (provider_models, synced) =
                startup_model_sync_models_or_fallback(models, config.upstream_models_snapshot());
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
            (provider_models, synced)
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
    let next = config_with_current_provider_model_sync(&latest, models, synced);
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
    let fetched_models = fetch_provider_models(profile, &state.http_client)
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
    let manual_models = selected_models_not_in_upstream(next.selected_models(), &fetched_models);
    next.upstream_models_by_provider
        .insert(provider_id.clone(), models.clone());
    if manual_models.is_empty() {
        next.manual_third_party_models_by_provider
            .remove(&provider_id);
    } else {
        next.manual_third_party_models_by_provider
            .insert(provider_id, manual_models);
    }
    next = next.normalize();
    let model_state = current_model_state(&next)?;
    let model_catalog_fallback = should_refresh_model_catalog(&model_state)
        && refresh_model_catalog_for_provider_sync(&next)?;
    state.store.save(&next).map_err(|error| error.to_string())?;
    *state.config.write().await = next.clone();
    drop(_config_write_guard);
    let hot_reload = hot_reload_runtime_models(state, &next, &model_state).await;
    let restart_required = runtime_config_requires_restart(state, &next).await;
    Ok(hot_reload.add_to_response(json!({
        "status":"ok",
        "models":fetched_models,
        "modelState":model_state,
        "modelCatalogFallback":model_catalog_fallback,
        "restartRequired":restart_required,
    })))
}

pub async fn test_current_provider(state: &Arc<AppState>) -> Result<Value, String> {
    let config = state.config.read().await.clone();
    let profile = config
        .active_profile()
        .ok_or_else(|| "找不到当前线路".to_string())?;
    if profile.cc_switch_read_only {
        return Err("官方线路无需执行第三方 API 对话测试".to_string());
    }
    let model = config
        .default_model()
        .or_else(|| config.selected_models().first().map(String::as_str))
        .or_else(|| config.upstream_models().first().map(String::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| "请先同步模型并设置默认模型".to_string())?
        .to_string();
    let base_url = profile.normalized_base_url();
    let relay = RelayProfile {
        id: profile.id.clone(),
        name: profile.name.clone(),
        model: model.clone(),
        base_url: base_url.clone(),
        upstream_base_url: base_url,
        api_key: profile.api_key.clone(),
        protocol: profile.protocol,
        relay_mode: RelayMode::PureApi,
        ..RelayProfile::default()
    };
    let result = codey_runtime_core::relay_config::test_relay_profile(&relay, &model)
        .await
        .map_err(|error| {
            error_log::record_failure(
                "provider_test_failed",
                "test_current_provider",
                "无法完成第三方模型对话测试",
                json!({
                    "provider": profile.id,
                    "model": model,
                    "protocol": profile.protocol,
                    "errorKind": "request_failed",
                }),
            );
            error.to_string()
        })?;
    if !(200..300).contains(&result.http_status) {
        error_log::record_failure(
            "provider_test_failed",
            "test_current_provider",
            format!("上游返回 HTTP {}", result.http_status),
            json!({
                "provider": profile.id,
                "model": model,
                "protocol": profile.protocol,
                "httpStatus": result.http_status,
                "responseBytes": result.response_preview.len(),
            }),
        );
        return Err(format!(
            "模型「{model}」对话测试失败：HTTP {}{}",
            result.http_status,
            if result.response_preview.trim().is_empty() {
                String::new()
            } else {
                format!("，{}", result.response_preview.trim())
            }
        ));
    }
    Ok(json!({
        "status": "ok",
        "model": model,
        "protocol": profile.protocol,
        "httpStatus": result.http_status,
        "endpoint": result.endpoint,
        "responsePreview": result.response_preview,
    }))
}

pub async fn save_selected_models(
    state: &Arc<AppState>,
    requested_official_models: Vec<String>,
    requested_third_party_models: Vec<String>,
    requested_manual_third_party_models: Vec<String>,
    requested_deleted_third_party_models: Vec<String>,
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
    let deleted_third_party_model_keys = validate_deleted_third_party_models(
        &model_catalog::default_official_model_slugs(),
        &requested_deleted_third_party_models,
    )?;
    let selected = selected
        .into_iter()
        .filter(|model| !deleted_third_party_model_keys.contains(model.to_lowercase().as_str()))
        .collect::<Vec<_>>();
    let provider_id = config
        .current_provider_id()
        .ok_or_else(|| "当前线路缺少标识".to_string())?
        .to_string();
    validate_deleted_models_are_manual(
        config.manual_third_party_models(),
        &deleted_third_party_model_keys,
    )?;
    let manual_third_party_models = validate_manual_third_party_model_sources(
        &model_catalog::default_official_model_slugs(),
        &selected,
        config.upstream_models(),
        config.manual_third_party_models(),
        &requested_manual_third_party_models,
    )?;
    let mut supported_models = supported_official;
    preserve_selected_third_party_models_except(
        &mut supported_models,
        config.upstream_models(),
        &deleted_third_party_model_keys,
    );
    preserve_selected_third_party_models_except(&mut supported_models, &selected, &HashSet::new());
    config
        .upstream_models_by_provider
        .insert(provider_id.clone(), supported_models);
    if selected.is_empty() {
        config.selected_models_by_provider.remove(&provider_id);
        config
            .manual_third_party_models_by_provider
            .remove(&provider_id);
    } else {
        config
            .selected_models_by_provider
            .insert(provider_id.clone(), selected);
        if manual_third_party_models.is_empty() {
            config
                .manual_third_party_models_by_provider
                .remove(&provider_id);
        } else {
            config
                .manual_third_party_models_by_provider
                .insert(provider_id, manual_third_party_models);
        }
    }
    config = config.normalize();
    let model_catalog_fallback = refresh_model_catalog_or_fallback(&config)?;
    state
        .store
        .save(&config)
        .map_err(|error| error.to_string())?;
    *state.config.write().await = config.clone();
    let model_state = current_model_state(&config)?;
    let public_config = redacted_config(&config);
    drop(_config_write_guard);
    let hot_reload = hot_reload_runtime_models(state, &config, &model_state).await;
    let restart_required = runtime_config_requires_restart(state, &config).await;
    Ok(hot_reload.add_to_response(json!({
        "status":"ok",
        "config":public_config,
        "modelState":model_state,
        "modelCatalogFallback":model_catalog_fallback,
        "restartRequired":restart_required,
    })))
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

pub(super) fn validate_deleted_third_party_models(
    official_model_ids: &[String],
    requested_deleted_third_party_models: &[String],
) -> Result<HashSet<String>, String> {
    let official_model_keys = official_model_ids
        .iter()
        .map(|model| model.to_lowercase())
        .collect::<HashSet<_>>();
    requested_deleted_third_party_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .try_fold(HashSet::<String>::new(), |mut models, model| {
            let key = model.to_lowercase();
            if official_model_keys.contains(key.as_str()) {
                return Err(format!("官方模型 {model} 不能作为其他模型删除"));
            }
            models.insert(key);
            Ok(models)
        })
}

fn validate_deleted_models_are_manual(
    manual_third_party_models: &[String],
    deleted_model_keys: &HashSet<String>,
) -> Result<(), String> {
    let manual_model_keys = manual_third_party_models
        .iter()
        .map(|model| model.trim().to_lowercase())
        .collect::<HashSet<_>>();
    if let Some(model) = deleted_model_keys
        .iter()
        .find(|model| !manual_model_keys.contains(model.as_str()))
    {
        return Err(format!("模型 {model} 不是手动添加的其他模型，不能删除"));
    }
    Ok(())
}

fn validate_manual_third_party_model_sources(
    official_model_ids: &[String],
    selected_third_party_models: &[String],
    upstream_models: &[String],
    existing_manual_third_party_models: &[String],
    requested_manual_third_party_models: &[String],
) -> Result<Vec<String>, String> {
    let official_model_keys = official_model_ids
        .iter()
        .map(|model| model.to_lowercase())
        .collect::<HashSet<_>>();
    let selected_model_keys = selected_third_party_models
        .iter()
        .map(|model| model.trim().to_lowercase())
        .collect::<HashSet<_>>();
    let upstream_model_keys = upstream_models
        .iter()
        .map(|model| model.trim().to_lowercase())
        .collect::<HashSet<_>>();
    let existing_manual_model_keys = existing_manual_third_party_models
        .iter()
        .map(|model| model.trim().to_lowercase())
        .collect::<HashSet<_>>();

    requested_manual_third_party_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .try_fold(Vec::<String>::new(), |mut models, model| {
            let key = model.to_lowercase();
            if official_model_keys.contains(key.as_str()) {
                return Err(format!("官方模型 {model} 不能作为手动添加的其他模型"));
            }
            if !selected_model_keys.contains(key.as_str()) {
                return Ok(models);
            }
            if upstream_model_keys.contains(key.as_str())
                && !existing_manual_model_keys.contains(key.as_str())
            {
                return Ok(models);
            }
            if !models.iter().any(|existing| existing == model) {
                models.push(model.to_string());
            }
            Ok(models)
        })
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
    let hot_reload = hot_reload_runtime_models(state, &config, &model_state).await;
    let restart_required = runtime_config_requires_restart(state, &config).await;
    Ok(hot_reload.add_to_response(json!({
        "status":"ok",
        "config":public_config,
        "modelState":model_state,
        "restartRequired":restart_required,
    })))
}

async fn hot_reload_runtime_models(
    state: &Arc<AppState>,
    config: &CodeyConfig,
    model_state: &model_catalog::ModelSelectionState,
) -> ModelHotReloadOutcome {
    let runtime = state.runtime.lock().await.clone();
    let Some(runtime) = runtime else {
        return ModelHotReloadOutcome::default();
    };
    if provider_route_requires_restart(&runtime.applied_config, config) {
        return ModelHotReloadOutcome::default();
    }
    let expected_models = renderer_model_ids(model_state);
    let websocket_url = runtime.renderer_websocket_url().await;
    match cdp::refresh_model_whitelist(&websocket_url, &expected_models, &model_state.default_model)
        .await
    {
        Ok(()) => {
            runtime.mark_model_config_applied(config).await;
            ModelHotReloadOutcome {
                reloaded: true,
                error: None,
            }
        }
        Err(error) => {
            let error = format!("{error:#}");
            error_log::record_failure(
                "patch_verification_failed",
                "refresh_model_whitelist",
                error.clone(),
                json!({
                    "modelCount": expected_models.len(),
                    "websocketUrl": websocket_url,
                }),
            );
            ModelHotReloadOutcome {
                reloaded: false,
                error: Some(error),
            }
        }
    }
}

pub(super) fn current_model_state(
    config: &CodeyConfig,
) -> Result<model_catalog::ModelSelectionState, String> {
    let official = config
        .profiles
        .iter()
        .find(|profile| profile.id == config.active_profile_id)
        .is_none_or(|profile| profile.cc_switch_read_only);
    model_catalog::selection_state_with_manual_models(
        &codex_home(),
        official,
        config.upstream_models_snapshot(),
        config.selected_models(),
        config.manual_third_party_models(),
        config.default_model(),
    )
    .map_err(|error| error.to_string())
}

pub(super) fn current_renderer_model_catalog(config: &CodeyConfig) -> Result<Value, String> {
    let model_state = current_model_state(config)?;
    Ok(renderer_model_catalog_value(config, &model_state))
}

pub(super) fn provider_route_requires_restart(
    applied: &CodeyConfig,
    current: &CodeyConfig,
) -> bool {
    applied.active_profile() != current.active_profile()
}

pub(super) fn renderer_model_catalog_value(
    config: &CodeyConfig,
    model_state: &model_catalog::ModelSelectionState,
) -> Value {
    let models = renderer_model_ids(model_state);
    let model_metadata = model_state
        .official_models
        .iter()
        .filter(|model| model.supported)
        .map(|model| {
            json!({
                "model": model.slug,
                "supported_reasoning_efforts": model.supported_reasoning_efforts,
                "default_reasoning_effort": model.default_reasoning_effort,
            })
        })
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
        "model_metadata": model_metadata,
        "sources": [],
        "responses_api": {
            "status": "unknown",
            "message": ""
        }
    })
}

fn renderer_model_ids(model_state: &model_catalog::ModelSelectionState) -> Vec<String> {
    model_state
        .official_models
        .iter()
        .filter(|model| model.supported)
        .map(|model| model.slug.clone())
        .chain(model_state.third_party_models.iter().cloned())
        .collect()
}

pub(super) fn should_refresh_model_catalog(
    model_state: &model_catalog::ModelSelectionState,
) -> bool {
    !model_state.official_models.is_empty() || !model_state.third_party_models.is_empty()
}

fn refresh_model_catalog_for_provider_sync(config: &CodeyConfig) -> Result<bool, String> {
    refresh_model_catalog_or_fallback(config)
}

fn refresh_model_catalog_or_fallback(config: &CodeyConfig) -> Result<bool, String> {
    model_catalog_fallback(try_refresh_model_catalog(config))
}

fn model_catalog_fallback(result: anyhow::Result<()>) -> Result<bool, String> {
    match result {
        Ok(()) => Ok(false),
        Err(error) if model_catalog::is_runtime_model_cache_unavailable(&error) => Ok(true),
        Err(error) => Err(error.to_string()),
    }
}

fn try_refresh_model_catalog(config: &CodeyConfig) -> anyhow::Result<()> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_changes_accept_only_the_known_builtin_catalog_fallback() {
        let home = tempfile::tempdir().unwrap();
        let missing_cache =
            model_catalog::refresh_for_provider(home.path(), false, Some(&[]), &[]).unwrap_err();

        assert!(model_catalog_fallback(Err(missing_cache)).unwrap());
        assert!(!model_catalog_fallback(Ok(())).unwrap());
        assert_eq!(
            model_catalog_fallback(Err(anyhow::anyhow!("模型目录写入失败"))).unwrap_err(),
            "模型目录写入失败"
        );
    }

    #[test]
    fn synced_models_are_not_marked_as_manual_sources() {
        let official = model_catalog::default_official_model_slugs();

        let manual = validate_manual_third_party_model_sources(
            &official,
            &["provider-synced".into(), "provider-manual".into()],
            &["provider-synced".into()],
            &[],
            &["provider-synced".into(), "provider-manual".into()],
        )
        .unwrap();

        assert_eq!(manual, ["provider-manual"]);
    }

    #[test]
    fn deleted_model_must_be_a_saved_manual_source() {
        let deleted = ["provider-synced".to_string()]
            .into_iter()
            .collect::<HashSet<_>>();

        let error =
            validate_deleted_models_are_manual(&["provider-manual".into()], &deleted).unwrap_err();

        assert!(error.contains("不是手动添加的其他模型"));
    }

    #[test]
    fn provider_sync_reclassifies_old_selected_models_by_the_raw_upstream_list() {
        let mut config = CodeyConfig::default();
        let provider_id = config.current_provider_id().unwrap().to_string();
        config.selected_models_by_provider.insert(
            provider_id.clone(),
            vec!["provider-synced".into(), "provider-manual".into()],
        );

        let synced =
            config_with_current_provider_model_sync(&config, vec!["provider-synced".into()], true);

        assert_eq!(
            synced.manual_third_party_models_by_provider[&provider_id],
            ["provider-manual"]
        );
        assert_eq!(
            synced.upstream_models_by_provider[&provider_id],
            ["provider-synced", "provider-manual"]
        );
    }

    #[test]
    fn provider_route_restart_detection_ignores_model_only_changes() {
        let applied = CodeyConfig::default();
        let mut current = applied.clone();
        let provider_id = current.current_provider_id().unwrap().to_string();
        current
            .default_model_by_provider
            .insert(provider_id, "provider-default".into());

        assert!(!provider_route_requires_restart(&applied, &current));
    }

    #[test]
    fn provider_route_restart_detection_catches_profile_changes() {
        let applied = CodeyConfig::default();
        let mut current = applied.clone();
        current.profiles[0].base_url = "https://api.example.test/v1".into();

        assert!(provider_route_requires_restart(&applied, &current));
    }
}
