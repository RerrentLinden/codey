use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    AppState, STARTUP_PROVIDER_MODEL_SYNC_TIMEOUT, SubagentHotReloadOutcome,
    hot_reload_runtime_subagent_config, redacted_config, runtime_config_requires_restart,
    save_config_to_store,
};
use crate::cc_switch;
use crate::cdp;
use crate::codex_config::codex_home;
use crate::config::{CodeyConfig, ProviderProfile};
use crate::error_log;
use crate::model_catalog;
use crate::model_id;
use crate::provider_models;
use crate::subagent_policy;

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

fn add_subagent_hot_reload_to_response(
    mut response: Value,
    outcome: SubagentHotReloadOutcome,
) -> Value {
    if let Some(object) = response.as_object_mut() {
        object.insert(
            "subagentConfigHotReloaded".into(),
            Value::Bool(outcome.reloaded()),
        );
        object.insert(
            "subagentConfigRepaired".into(),
            Value::Bool(outcome.repaired()),
        );
        object.insert(
            "subagentConfigHealth".into(),
            Value::String(outcome.health().to_string()),
        );
        object.insert(
            "subagentConfigRepairReasons".into(),
            Value::Array(
                outcome
                    .repair_reasons()
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
        if outcome.requires_restart() {
            object.insert("restartRequired".into(), Value::Bool(true));
        }
        if let Some(error) = outcome.error() {
            object.insert(
                "subagentConfigHotReloadError".into(),
                Value::String(error.to_string()),
            );
        }
    }
    response
}

pub async fn sync_current_provider_command(state: &Arc<AppState>) -> Result<Value, String> {
    let cc_switch = sync_cc_switch_state(state).await?;
    let config = state.config.read().await.clone();
    let restart_required = runtime_config_requires_restart(state, &config).await;
    let model_state = current_model_state_async(&config).await?;
    let public_config = redacted_config(&config);
    Ok(json!({
        "status":"ok",
        "config":public_config,
        "ccSwitch":cc_switch,
        "modelState":model_state,
        "restartRequired":restart_required,
    }))
}

pub async fn sync_cc_switch_state(
    state: &Arc<AppState>,
) -> Result<cc_switch::CcSwitchStatus, String> {
    let home = codex_home();
    let mut status = sync_cc_switch_state_with(state, move |config| {
        let mut source = config.clone();
        source.remember_current_subagent_config();
        let (mut next, mut status) =
            cc_switch::sync_current_provider(&source, home).map_err(|error| error.to_string())?;
        next.restore_current_subagent_config();
        subagent_policy::reconcile_for_current_provider(&mut next, home, status.provider.official);
        next = next.normalize();
        status.changed = next != config;
        Ok((next, status))
    })
    .await?;
    let (_, subagent_changed) = reconcile_current_subagent_defaults(state, None).await?;
    status.changed |= subagent_changed;
    Ok(status)
}

pub(super) async fn sync_cc_switch_state_with<F>(
    state: &Arc<AppState>,
    sync: F,
) -> Result<cc_switch::CcSwitchStatus, String>
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
                return Ok(status);
            }

            let _config_write_guard = state.config_write_lock.lock().await;
            let latest = state.config.read().await.clone();
            if latest != previous {
                return Err("Codey 设置在同步线路期间已更新，已忽略过期的同步结果".to_string());
            }
            save_config_to_store(state, &config)
                .await
                .map_err(|error| format!("保存当前线路同步结果失败：{error}"))?;
            *state.config.write().await = config;
            Ok(status)
        }
        Ok(Err(error)) => Err(error),
        Err(error) => Err(format!("同步当前线路任务异常退出：{error}")),
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
        .map(|model| model_id::key(&model))
        .collect::<HashSet<_>>();
    let upstream_model_keys = upstream_models
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    selected_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| {
            !model.is_empty()
                && !official_model_keys.contains(&model_id::key(model))
                && !upstream_model_keys.contains(&model_id::key(model))
                && seen.insert(model_id::key(model))
        })
        .map(ToString::to_string)
        .collect()
}

fn config_with_current_provider_model_sync(
    config: &CodeyConfig,
    provider_models: Vec<String>,
    synced: bool,
    codex_home: &std::path::Path,
) -> CodeyConfig {
    let Some(provider_id) = config.current_provider_id().map(ToString::to_string) else {
        return config.clone();
    };
    let manual_models = if synced {
        selected_models_not_in_upstream(config.selected_models(), &provider_models)
    } else {
        preserve_selected_third_party_models(Vec::new(), config.manual_third_party_models())
    };
    let mut supported_models = if synced {
        preserve_selected_third_party_models(provider_models, config.selected_models())
    } else {
        provider_models
    };
    preserve_declared_official_models(&mut supported_models, config.declared_official_models());
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
    next = next.normalize();
    subagent_policy::reconcile_for_current_provider(&mut next, codex_home, false);
    next
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
        .map(|model| model_id::key(&model))
        .collect::<HashSet<_>>();
    for model in selected_models {
        let model = model.trim();
        let key = model_id::key(model);
        if model.is_empty()
            || official_model_keys.contains(key.as_str())
            || deleted_model_keys.contains(key.as_str())
            || upstream_models
                .iter()
                .any(|existing| model_id::equal(existing, model))
        {
            continue;
        }
        upstream_models.push(model.to_string());
    }
}

fn preserve_declared_official_models(
    upstream_models: &mut Vec<String>,
    declared_official_models: &[String],
) {
    let official_models_by_key = model_catalog::default_official_model_slugs()
        .into_iter()
        .map(|model| (model_id::key(&model), model))
        .collect::<std::collections::HashMap<_, _>>();
    for declared_model in declared_official_models {
        let key = model_id::key(declared_model);
        let Some(official_model) = official_models_by_key.get(&key) else {
            continue;
        };
        if upstream_models
            .iter()
            .any(|existing| model_id::equal(existing, official_model))
        {
            continue;
        }
        upstream_models.push(official_model.clone());
    }
}

async fn fetch_provider_models(
    profile: ProviderProfile,
    http_client: &reqwest::Client,
) -> anyhow::Result<Vec<String>> {
    let home = codex_home();
    let fetch_profile = tokio::task::spawn_blocking(move || {
        cc_switch::provider_model_fetch_profile(&profile, home)
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
        return reconcile_current_subagent_defaults(state, None)
            .await
            .map(|(config, _)| config)
            .unwrap_or_else(|error| {
                eprintln!("启动时刷新官方线路模型目录失败，沿用当前设置：{error}");
                config
            });
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
    let persistence_base = (!synced).then(|| latest.clone());
    let next = config_with_current_provider_model_sync(&latest, models, synced, codex_home());
    let committed = commit_startup_model_sync(state, latest, next, synced).await;
    drop(_config_write_guard);
    reconcile_current_subagent_defaults(state, persistence_base.as_ref())
        .await
        .map(|(config, _)| config)
        .unwrap_or_else(|error| {
            eprintln!("启动时刷新第三方线路模型目录失败，沿用当前设置：{error}");
            committed
        })
}

async fn commit_startup_model_sync(
    state: &Arc<AppState>,
    latest: CodeyConfig,
    next: CodeyConfig,
    synced: bool,
) -> CodeyConfig {
    if synced && let Err(error) = save_config_to_store(state, &next).await {
        eprintln!("保存启动时模型同步结果失败，本次启动沿用已持久化模型：{error:#}");
        return latest;
    }
    *state.config.write().await = next.clone();
    next
}

pub async fn fetch_current_provider_models(state: &Arc<AppState>) -> Result<Value, String> {
    let _provider_model_sync_guard = state.provider_model_sync_lock.lock().await;
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
    let mut next = config_with_current_provider_model_sync(
        &latest,
        fetched_models.clone(),
        true,
        codex_home(),
    );
    let (catalog_refresh, model_state) = refreshed_model_state_async(&next, true).await?;
    subagent_policy::reconcile_with_model_state(&mut next, Some(&model_state));
    next = next.normalize();
    if let Err(error) = save_config_to_store(state, &next).await {
        return Err(rollback_model_catalog_after_config_save_async(catalog_refresh, error).await);
    }
    let model_catalog_fallback = catalog_refresh
        .as_ref()
        .is_some_and(|refresh| refresh.fallback);
    *state.config.write().await = next.clone();
    drop(_config_write_guard);
    let hot_reload = hot_reload_runtime_models(state, &next, &model_state).await;
    let subagent_hot_reload = hot_reload_runtime_subagent_config(state, &next).await;
    let restart_required = runtime_config_requires_restart(state, &next).await;
    Ok(add_subagent_hot_reload_to_response(
        hot_reload.add_to_response(json!({
            "status":"ok",
            "models":fetched_models,
            "modelState":model_state,
            "modelCatalogFallback":model_catalog_fallback,
            "restartRequired":restart_required,
        })),
        subagent_hot_reload,
    ))
}

pub async fn save_selected_models(
    state: &Arc<AppState>,
    requested_official_models: Vec<String>,
    requested_third_party_models: Vec<String>,
    requested_manual_third_party_models: Vec<String>,
    requested_deleted_third_party_models: Vec<String>,
) -> Result<Value, String> {
    validate_requested_model_list_bounds("官方模型", &requested_official_models)?;
    validate_requested_model_list_bounds("其他模型", &requested_third_party_models)?;
    validate_requested_model_list_bounds(
        "手动添加的其他模型",
        &requested_manual_third_party_models,
    )?;
    validate_requested_model_list_bounds(
        "待删除的其他模型",
        &requested_deleted_third_party_models,
    )?;
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
        .filter(|model| !deleted_third_party_model_keys.contains(&model_id::key(model)))
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
    let declared_official_models = supported_official.clone();
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
    if declared_official_models.is_empty() {
        config
            .declared_official_models_by_provider
            .remove(&provider_id);
    } else {
        config
            .declared_official_models_by_provider
            .insert(provider_id.clone(), declared_official_models);
    }
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
    let (catalog_refresh, model_state) = refreshed_model_state_async(&config, false).await?;
    subagent_policy::reconcile_with_model_state(&mut config, Some(&model_state));
    config = config.normalize();
    if let Err(error) = save_config_to_store(state, &config).await {
        return Err(rollback_model_catalog_after_config_save_async(catalog_refresh, error).await);
    }
    let model_catalog_fallback = catalog_refresh
        .as_ref()
        .is_some_and(|refresh| refresh.fallback);
    *state.config.write().await = config.clone();
    let public_config = redacted_config(&config);
    drop(_config_write_guard);
    let hot_reload = hot_reload_runtime_models(state, &config, &model_state).await;
    let subagent_hot_reload = hot_reload_runtime_subagent_config(state, &config).await;
    let restart_required = runtime_config_requires_restart(state, &config).await;
    Ok(add_subagent_hot_reload_to_response(
        hot_reload.add_to_response(json!({
            "status":"ok",
            "config":public_config,
            "modelState":model_state,
            "modelCatalogFallback":model_catalog_fallback,
            "restartRequired":restart_required,
        })),
        subagent_hot_reload,
    ))
}

fn validate_requested_model_list_bounds(label: &str, models: &[String]) -> Result<(), String> {
    if models.len() > provider_models::MAX_PROVIDER_MODELS {
        return Err(format!(
            "{label}数量超过安全上限 {}",
            provider_models::MAX_PROVIDER_MODELS
        ));
    }
    if models
        .iter()
        .map(|model| model.trim())
        .any(|model| model.len() > provider_models::MAX_PROVIDER_MODEL_ID_BYTES)
    {
        return Err(format!(
            "{label} ID 超过安全上限 {} 字节",
            provider_models::MAX_PROVIDER_MODEL_ID_BYTES
        ));
    }
    Ok(())
}

pub(super) fn validate_manual_model_selection(
    official_model_ids: &[String],
    requested_official_models: &[String],
    requested_third_party_models: &[String],
) -> Result<(Vec<String>, Vec<String>), String> {
    let official_by_key = official_model_ids
        .iter()
        .map(|model| (model_id::key(model), model.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let requested_official = requested_official_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .map(model_id::key)
        .collect::<HashSet<_>>();
    if let Some(model) = requested_official
        .iter()
        .find(|model| !official_by_key.contains_key(model.as_str()))
    {
        return Err(format!("模型 {model} 不在官方模型列表中"));
    }
    let supported_official = official_model_ids
        .iter()
        .filter(|model| requested_official.contains(&model_id::key(model)))
        .cloned()
        .collect::<Vec<_>>();
    let mut selected_third_party = Vec::with_capacity(requested_third_party_models.len());
    let mut seen_third_party = HashSet::<String>::with_capacity(requested_third_party_models.len());
    for model in requested_third_party_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
    {
        let key = model_id::key(model);
        if official_by_key.contains_key(&key) {
            return Err(format!(
                "模型 {model} 已在官方模型列表中，请直接勾选，不可作为其他模型手动添加"
            ));
        }
        if seen_third_party.insert(key) {
            selected_third_party.push(model.to_string());
        }
    }
    Ok((supported_official, selected_third_party))
}

pub(super) fn validate_deleted_third_party_models(
    official_model_ids: &[String],
    requested_deleted_third_party_models: &[String],
) -> Result<HashSet<String>, String> {
    let official_model_keys = official_model_ids
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    requested_deleted_third_party_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
        .try_fold(HashSet::<String>::new(), |mut models, model| {
            let key = model_id::key(model);
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
        .map(|model| model_id::key(model))
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
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let selected_model_keys = selected_third_party_models
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let upstream_model_keys = upstream_models
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();
    let existing_manual_model_keys = existing_manual_third_party_models
        .iter()
        .map(|model| model_id::key(model))
        .collect::<HashSet<_>>();

    let mut models = Vec::with_capacity(requested_manual_third_party_models.len());
    let mut seen = HashSet::<String>::with_capacity(requested_manual_third_party_models.len());
    for model in requested_manual_third_party_models
        .iter()
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
    {
        let key = model_id::key(model);
        if official_model_keys.contains(key.as_str()) {
            return Err(format!("官方模型 {model} 不能作为手动添加的其他模型"));
        }
        if !selected_model_keys.contains(key.as_str()) {
            continue;
        }
        if upstream_model_keys.contains(key.as_str())
            && !existing_manual_model_keys.contains(key.as_str())
        {
            continue;
        }
        if seen.insert(key) {
            models.push(model.to_string());
        }
    }
    Ok(models)
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
    let mut model_state = current_model_state_async(&config).await?;
    let canonical_model = model_state
        .official_models
        .iter()
        .find(|model| model.supported && model_id::equal(&model.slug, requested_model))
        .map(|model| model.slug.as_str())
        .or_else(|| {
            model_state
                .third_party_models
                .iter()
                .find(|model| model_id::equal(model, requested_model))
                .map(String::as_str)
        })
        .ok_or_else(|| format!("模型 {requested_model} 当前不可用，无法设为默认"))?
        .to_string();
    let provider_id = config
        .current_provider_id()
        .ok_or_else(|| "当前线路缺少标识".to_string())?
        .to_string();
    config
        .default_model_by_provider
        .insert(provider_id, canonical_model.clone());
    config = config.normalize();
    save_config_to_store(state, &config).await?;
    *state.config.write().await = config.clone();
    // The requested model was canonicalized against this exact selection state;
    // changing only the provider default cannot alter the available model lists.
    model_state.default_model = canonical_model;
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
        codex_home(),
        official,
        config.upstream_models_snapshot(),
        config.selected_models(),
        config.manual_third_party_models(),
        config.default_model(),
    )
    .map_err(|error| error.to_string())
}

pub(super) async fn current_model_state_async(
    config: &CodeyConfig,
) -> Result<model_catalog::ModelSelectionState, String> {
    let config = config.clone();
    tokio::task::spawn_blocking(move || current_model_state(&config))
        .await
        .map_err(|error| format!("读取 Codey 模型目录的任务异常退出：{error}"))?
}

fn current_renderer_model_catalog(config: &CodeyConfig) -> Result<Value, String> {
    let model_state = current_model_state(config)?;
    Ok(renderer_model_catalog_value(config, &model_state))
}

pub(super) async fn current_renderer_model_catalog_async(
    config: CodeyConfig,
) -> Result<Value, String> {
    tokio::task::spawn_blocking(move || current_renderer_model_catalog(&config))
        .await
        .map_err(|error| format!("读取渲染进程模型目录的任务异常退出：{error}"))?
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
        .chain(model_state.third_party_models.iter().map(|model| {
            json!({
                "model": model,
                "supported_reasoning_efforts": model_catalog::THIRD_PARTY_REASONING_EFFORTS,
                "default_reasoning_effort":
                    model_catalog::THIRD_PARTY_DEFAULT_REASONING_EFFORT,
            })
        }))
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

struct ModelCatalogRefresh {
    fallback: bool,
    snapshot: model_catalog::CatalogSnapshot,
}

fn refresh_model_catalog_or_fallback(config: &CodeyConfig) -> Result<ModelCatalogRefresh, String> {
    let home = codex_home();
    let snapshot = model_catalog::snapshot(home).map_err(|error| error.to_string())?;
    let result = model_catalog_fallback(try_refresh_model_catalog(config), home);
    match result {
        Ok(fallback) => Ok(ModelCatalogRefresh { fallback, snapshot }),
        Err(error) => Err(rollback_model_catalog_snapshot(snapshot, error)),
    }
}

async fn refreshed_model_state_async(
    config: &CodeyConfig,
    refresh_only_when_populated: bool,
) -> Result<
    (
        Option<ModelCatalogRefresh>,
        model_catalog::ModelSelectionState,
    ),
    String,
> {
    let config = config.clone();
    tokio::task::spawn_blocking(move || {
        let should_refresh = if refresh_only_when_populated {
            should_refresh_model_catalog(&current_model_state(&config)?)
        } else {
            true
        };
        let refresh = should_refresh
            .then(|| refresh_model_catalog_or_fallback(&config))
            .transpose()?;
        match current_model_state(&config) {
            Ok(model_state) => Ok((refresh, model_state)),
            Err(error) => Err(rollback_model_catalog_after_config_save(refresh, error)),
        }
    })
    .await
    .map_err(|error| format!("刷新 Codey 模型目录的任务异常退出：{error}"))?
}

async fn reconcile_current_subagent_defaults(
    state: &Arc<AppState>,
    persistence_base: Option<&CodeyConfig>,
) -> Result<(CodeyConfig, bool), String> {
    let _config_write_guard = state.config_write_lock.lock().await;
    let current = state.config.read().await.clone();
    let (catalog_refresh, model_state) = refreshed_model_state_async(&current, false).await?;
    let mut next = current.clone();
    subagent_policy::reconcile_with_model_state(&mut next, Some(&model_state));
    next = next.normalize();
    if next == current {
        return Ok((current, false));
    }
    let persisted = persistence_base.map_or_else(
        || next.clone(),
        |base| config_with_reconciled_subagent_defaults(base, &next),
    );
    if let Err(error) = save_config_to_store(state, &persisted).await {
        return Err(rollback_model_catalog_after_config_save_async(catalog_refresh, error).await);
    }
    *state.config.write().await = next.clone();
    Ok((next, true))
}

fn config_with_reconciled_subagent_defaults(
    persistence_base: &CodeyConfig,
    reconciled: &CodeyConfig,
) -> CodeyConfig {
    let mut persisted = persistence_base.clone();
    persisted.subagent_optimization = reconciled.subagent_optimization;
    persisted
        .subagent_model
        .clone_from(&reconciled.subagent_model);
    persisted
        .subagent_reasoning_effort
        .clone_from(&reconciled.subagent_reasoning_effort);
    persisted
        .subagent_roles
        .clone_from(&reconciled.subagent_roles);
    if let Some(provider_id) = reconciled.current_provider_id()
        && let Some(selection) = reconciled.subagent_config_by_provider.get(provider_id)
    {
        persisted
            .subagent_config_by_provider
            .insert(provider_id.to_string(), selection.clone());
    }
    persisted.normalize()
}

fn rollback_model_catalog_after_config_save(
    refresh: Option<ModelCatalogRefresh>,
    error: String,
) -> String {
    match refresh {
        Some(refresh) => rollback_model_catalog_snapshot(refresh.snapshot, error),
        None => error,
    }
}

async fn rollback_model_catalog_after_config_save_async(
    refresh: Option<ModelCatalogRefresh>,
    error: String,
) -> String {
    let primary_error = error.clone();
    tokio::task::spawn_blocking(move || rollback_model_catalog_after_config_save(refresh, error))
        .await
        .unwrap_or_else(|join_error| {
            format!("{primary_error}；回滚 Codey 模型目录的任务异常退出：{join_error}")
        })
}

fn rollback_model_catalog_snapshot(
    snapshot: model_catalog::CatalogSnapshot,
    error: String,
) -> String {
    match model_catalog::restore_snapshot(snapshot) {
        Ok(()) => error,
        Err(rollback_error) => {
            format!("{error}；回滚 Codey 模型目录也失败：{rollback_error:#}")
        }
    }
}

fn model_catalog_fallback(
    result: anyhow::Result<()>,
    home: &std::path::Path,
) -> Result<bool, String> {
    match result {
        Ok(()) => Ok(false),
        Err(error) if model_catalog::is_runtime_model_cache_unavailable(&error) => {
            Ok(!model_catalog::is_available(home))
        }
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
        codex_home(),
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
    fn requested_model_lists_enforce_count_and_id_limits() {
        let too_many = (0..=provider_models::MAX_PROVIDER_MODELS)
            .map(|index| format!("model-{index}"))
            .collect::<Vec<_>>();
        assert!(
            validate_requested_model_list_bounds("其他模型", &too_many)
                .unwrap_err()
                .contains("数量超过安全上限")
        );

        let too_long = vec!["m".repeat(provider_models::MAX_PROVIDER_MODEL_ID_BYTES + 1)];
        assert!(
            validate_requested_model_list_bounds("其他模型", &too_long)
                .unwrap_err()
                .contains("ID 超过安全上限")
        );
    }

    #[test]
    fn manual_model_selection_keeps_first_case_insensitive_duplicate() {
        let official = model_catalog::default_official_model_slugs();
        let (_, selected) = validate_manual_model_selection(
            &official,
            &[],
            &[
                "provider-a".into(),
                "provider-a".into(),
                "Provider-A".into(),
                "provider-b".into(),
            ],
        )
        .unwrap();

        assert_eq!(selected, ["provider-a", "provider-b"]);
    }

    #[test]
    fn model_changes_accept_only_the_known_builtin_catalog_fallback() {
        let home = tempfile::tempdir().unwrap();
        let missing_cache =
            model_catalog::refresh_for_provider(home.path(), false, Some(&[]), &[]).unwrap_err();

        assert!(model_catalog_fallback(Err(missing_cache), home.path()).unwrap());
        assert!(!model_catalog_fallback(Ok(()), home.path()).unwrap());
        assert_eq!(
            model_catalog_fallback(Err(anyhow::anyhow!("模型目录写入失败")), home.path())
                .unwrap_err(),
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
        let home = tempfile::tempdir().unwrap();
        let mut config = CodeyConfig::default();
        let provider_id = config.current_provider_id().unwrap().to_string();
        config.selected_models_by_provider.insert(
            provider_id.clone(),
            vec!["provider-synced".into(), "provider-manual".into()],
        );

        let synced = config_with_current_provider_model_sync(
            &config,
            vec!["provider-synced".into()],
            true,
            home.path(),
        );

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
    fn provider_sync_preserves_only_user_declared_official_models() {
        let home = tempfile::tempdir().unwrap();
        let mut config = CodeyConfig::default();
        let provider_id = config.current_provider_id().unwrap().to_string();
        config.upstream_models_by_provider.insert(
            provider_id.clone(),
            vec!["gpt-5.6-sol".into(), "gpt-5.6-luna".into()],
        );
        config
            .declared_official_models_by_provider
            .insert(provider_id.clone(), vec![" GPT-5.6-SOL ".into()]);

        let synced = config_with_current_provider_model_sync(
            &config,
            vec!["provider-custom-model".into()],
            true,
            home.path(),
        );

        assert_eq!(
            synced.upstream_models_by_provider[&provider_id],
            ["provider-custom-model", "gpt-5.6-sol"]
        );
    }

    #[test]
    fn provider_model_refresh_preserves_subagent_settings_when_no_replacement_is_selected() {
        let home = tempfile::tempdir().unwrap();
        let mut config = CodeyConfig {
            subagent_optimization: true,
            subagent_model: "gpt-5.6-sol".into(),
            subagent_reasoning_effort: "high".into(),
            subagent_roles: crate::config::uniform_subagent_roles("gpt-5.6-sol", "high"),
            ..CodeyConfig::default()
        };
        let provider_id = config.current_provider_id().unwrap().to_string();
        config
            .upstream_models_by_provider
            .insert(provider_id, vec!["gpt-5.6-sol".into()]);

        let synced = config_with_current_provider_model_sync(
            &config,
            vec!["provider-custom-model".into()],
            true,
            home.path(),
        );

        assert!(synced.subagent_optimization);
        assert_eq!(synced.subagent_model, "gpt-5.6-sol");
        assert_eq!(synced.subagent_reasoning_effort, "high");
    }

    #[tokio::test]
    async fn startup_model_sync_does_not_publish_memory_when_persistence_fails() {
        let directory = tempfile::tempdir().unwrap();
        let latest = CodeyConfig::default();
        let mut next = latest.clone();
        let provider_id = next.current_provider_id().unwrap().to_string();
        next.upstream_models_by_provider
            .insert(provider_id, vec!["provider-new".into()]);
        let state = Arc::new(AppState {
            store: crate::config::ConfigStore::new(directory.path()),
            config: tokio::sync::RwLock::new(latest.clone()),
            ..AppState::default()
        });

        let committed = commit_startup_model_sync(&state, latest.clone(), next, true).await;

        assert_eq!(committed, latest);
        assert_eq!(*state.config.read().await, latest);
    }

    #[test]
    fn startup_fallback_persists_only_reconciled_subagent_defaults() {
        let mut persisted = CodeyConfig {
            subagent_optimization: true,
            subagent_model: "gpt-5.6-luna".into(),
            subagent_reasoning_effort: "high".into(),
            subagent_roles: crate::config::uniform_subagent_roles("gpt-5.6-luna", "high"),
            ..CodeyConfig::default()
        };
        let provider_id = persisted.current_provider_id().unwrap().to_string();
        persisted
            .upstream_models_by_provider
            .insert(provider_id.clone(), vec!["saved-model".into()]);

        let mut runtime_fallback = persisted.clone();
        runtime_fallback
            .upstream_models_by_provider
            .insert(provider_id.clone(), vec!["fallback-model".into()]);
        runtime_fallback.subagent_model = crate::config::DEFAULT_SUBAGENT_MODEL.into();
        runtime_fallback.subagent_roles.insert(
            crate::config::SUBAGENT_ROLE_DEFAULT.into(),
            crate::config::SubagentRoleConfig::new(crate::config::DEFAULT_SUBAGENT_MODEL, "high"),
        );

        let next = config_with_reconciled_subagent_defaults(&persisted, &runtime_fallback);

        assert_eq!(
            next.upstream_models_by_provider.get(&provider_id),
            Some(&vec!["saved-model".into()])
        );
        assert_eq!(next.subagent_model, crate::config::DEFAULT_SUBAGENT_MODEL);
        assert_eq!(next.subagent_reasoning_effort, "high");
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
