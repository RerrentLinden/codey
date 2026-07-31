use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::oneshot;

#[cfg(windows)]
use super::ensure_windows_codex_app_path;
use super::webhooks::{
    RecentSessionScanTask, await_recent_session_scan, start_recent_session_scan,
    start_waiting_webhook_watcher, stop_waiting_webhook_watcher, webhook_watcher_should_run,
};
use super::{
    AppState, RestartInProgressGuard, ScheduledRestart, current_update_platform,
    make_bridge_handler, runtime_config_requires_restart, sync_provider_models_for_launch,
};
use crate::codex_config::codex_home;
use crate::error_log;
use crate::launcher::{CodeyRuntime, restore_previous_runtime_state, restore_runtime_config};

pub async fn runtime_status(state: &Arc<AppState>) -> Result<Value, String> {
    let runtime = state.runtime.lock().await.clone();
    let config = state.config.read().await;
    let profile = config.active_profile();
    let restart_required = runtime_config_requires_restart(state, &config).await;
    let mut status = json!({
        "running": runtime.is_some(),
        "appVersion": env!("CARGO_PKG_VERSION"),
        "clientPlatform": current_update_platform(),
        "activeProfileId": profile.as_ref().map(|profile| profile.id.as_str()).unwrap_or_default(),
        "activeProfileName": profile.as_ref().map(|profile| profile.name.as_str()).unwrap_or_default(),
        "restartRequired": restart_required,
        "restartInProgress": state.restart_in_progress.load(Ordering::Acquire),
    });
    drop(config);
    if let Some(error) = state.startup_error.read().await.clone()
        && let Some(object) = status.as_object_mut()
    {
        object.insert("startupError".into(), Value::String(error));
    };
    if let Some(runtime) = runtime.as_ref()
        && let Some(object) = status.as_object_mut()
    {
        object.insert(
            "codexAppPath".into(),
            Value::String(runtime.codex_app_path.to_string_lossy().to_string()),
        );
        object.insert(
            "maintenance".into(),
            serde_json::to_value(&runtime.maintenance).unwrap_or_else(|_| json!({})),
        );
        let injection_statuses = runtime.injection_statuses.read().await.clone();
        object.insert(
            "injectionScripts".into(),
            serde_json::to_value(injection_statuses.as_ref()).unwrap_or_else(|_| json!([])),
        );
        let experimental_feature_runtime =
            runtime.experimental_feature_runtime.read().await.clone();
        object.insert(
            "experimentalFeatureRuntime".into(),
            serde_json::to_value(&experimental_feature_runtime).unwrap_or_else(|_| json!({})),
        );
    }
    if let Some(object) = status.as_object_mut() {
        object.insert(
            "traceLogStats".into(),
            serde_json::to_value(&state.trace_log_stats).unwrap_or_else(|_| json!({})),
        );
    }
    Ok(status)
}

pub(super) async fn refresh_injection_status(state: &Arc<AppState>) -> Result<Value, String> {
    let runtime = state.runtime.lock().await.clone();
    let Some(runtime) = runtime else {
        return Ok(json!([]));
    };
    let statuses = runtime.refresh_injection_statuses().await;
    serde_json::to_value(statuses.as_ref()).map_err(|error| error.to_string())
}

fn ensure_runtime_can_start(state: &AppState) -> Result<(), String> {
    if state.is_shutting_down() {
        Err("Codey 正在退出，无法启动 Codex".to_string())
    } else {
        Ok(())
    }
}

async fn reclaim_initial_session_scan(
    state: &Arc<AppState>,
    initial_scan_task: Option<RecentSessionScanTask>,
) {
    if let Some(initial_scan_task) = initial_scan_task {
        let (initial_event_cache, _) = await_recent_session_scan(initial_scan_task).await;
        *state.recent_session_event_cache.lock().await = Some(initial_event_cache);
    }
}

async fn launch_codey_inner_locked(state: &Arc<AppState>) -> Result<Value, String> {
    ensure_runtime_can_start(state)?;
    if state.runtime.lock().await.is_some() {
        return Ok(json!({"status":"already_running"}));
    }
    #[cfg(windows)]
    ensure_windows_codex_app_path(state).await?;
    stop_waiting_webhook_watcher(state).await;
    restore_previous_runtime_state(&codex_home())
        .map_err(|error| format!("恢复上次 Codey 临时 Codex 配置失败：{error}"))?;
    let config = sync_provider_models_for_launch(state).await;
    let initial_scan_task = if webhook_watcher_should_run(&config) {
        let initial_event_cache = state
            .recent_session_event_cache
            .lock()
            .await
            .take()
            .unwrap_or_default();
        Some(start_recent_session_scan(initial_event_cache))
    } else {
        None
    };
    if let Err(error) = ensure_runtime_can_start(state) {
        reclaim_initial_session_scan(state, initial_scan_task).await;
        return Err(error);
    }
    let handler = make_bridge_handler(state);
    let (runtime, codex_exit) = match CodeyRuntime::start(&config, handler).await {
        Ok(started) => started,
        Err(error) => {
            reclaim_initial_session_scan(state, initial_scan_task).await;
            return Err(error.to_string());
        }
    };
    if state.is_shutting_down() {
        let stop_error = runtime.stop().await.err();
        reclaim_initial_session_scan(state, initial_scan_task).await;
        return Err(stop_error.map_or_else(
            || "Codey 已进入退出流程，已取消本次 Codex 启动".to_string(),
            |error| format!("Codey 已进入退出流程，停止刚启动的 Codex 失败：{error}"),
        ));
    }
    *state.runtime.lock().await = Some(Arc::new(runtime));
    let runtime_generation = state.runtime_generation.fetch_add(1, Ordering::AcqRel) + 1;
    if let Some(initial_scan_task) = initial_scan_task {
        start_waiting_webhook_watcher(state, initial_scan_task).await;
    }
    let exit_state = Arc::clone(state);
    tokio::spawn(async move {
        if codex_exit.await.is_ok() {
            while exit_state.restart_in_progress.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            if exit_state.runtime_generation.load(Ordering::Acquire) == runtime_generation {
                exit_state.request_shutdown();
            }
        }
    });
    Ok(json!({"status":"running"}))
}

pub(super) async fn launch_codey_inner(state: &Arc<AppState>) -> Result<Value, String> {
    ensure_runtime_can_start(state)?;
    let _operation = state.runtime_operation.lock().await;
    ensure_runtime_can_start(state)?;
    launch_codey_inner_locked(state).await
}

pub async fn launch_codey_runtime(state: &Arc<AppState>) -> Result<Value, String> {
    let result = launch_codey_inner(state).await;
    *state.startup_error.write().await = result.as_ref().err().cloned();
    if let Err(error) = &result {
        error_log::record_failure(
            "runtime_start_failed",
            "launch_codey_runtime",
            error.clone(),
            json!({
                "restart": false,
            }),
        );
    }
    result
}

pub async fn schedule_restart_codey_runtime(state: &Arc<AppState>) -> Result<Value, String> {
    let mut restart_task = state.restart_task.lock().await;
    ensure_runtime_can_start(state)?;
    if state.restart_in_progress.swap(true, Ordering::AcqRel) {
        return Ok(json!({"status":"already_restarting"}));
    }

    let (cancel, cancel_rx) = oneshot::channel();
    let restart_state = Arc::clone(state);
    let task = tokio::spawn(async move {
        let _restart_guard = RestartInProgressGuard {
            state: Arc::clone(&restart_state),
        };
        run_scheduled_restart(restart_state, cancel_rx).await;
    });
    *restart_task = Some(ScheduledRestart { cancel, task });

    Ok(json!({"status":"restarting"}))
}

async fn run_scheduled_restart(restart_state: Arc<AppState>, mut cancel: oneshot::Receiver<()>) {
    tokio::select! {
        // The request originates inside the Codex renderer. Let the bridge
        // deliver its response before stopping the renderer that owns it.
        _ = tokio::time::sleep(Duration::from_millis(250)) => {}
        _ = &mut cancel => return,
    }
    if restart_state.is_shutting_down() {
        return;
    }

    let _operation = tokio::select! {
        operation = restart_state.runtime_operation.lock() => operation,
        _ = &mut cancel => return,
    };
    if restart_state.is_shutting_down() {
        return;
    }

    if let Err(error) = stop_codey_runtime_locked(&restart_state).await {
        error_log::record_failure(
            "runtime_restart_failed",
            "stop_runtime_for_restart",
            error.clone(),
            json!({}),
        );
        *restart_state.startup_error.write().await = Some(error);
        return;
    }
    restart_state
        .runtime_generation
        .fetch_add(1, Ordering::AcqRel);
    if restart_state.is_shutting_down() {
        return;
    }

    let launch = launch_codey_inner_locked(&restart_state).await;
    *restart_state.startup_error.write().await = launch.as_ref().err().cloned();
    if let Err(error) = launch {
        error_log::record_failure(
            "runtime_restart_failed",
            "launch_runtime_after_restart",
            error.clone(),
            json!({}),
        );
        eprintln!("Codey 自动重启 Codex 失败：{error}");
        restart_state.request_shutdown();
    }
}

async fn stop_codey_runtime_locked(state: &Arc<AppState>) -> Result<Value, String> {
    stop_waiting_webhook_watcher(state).await;
    let runtime = state.runtime.lock().await.take();
    if let Some(runtime) = runtime {
        if let Err(error) = runtime.stop().await {
            *state.runtime.lock().await = Some(runtime);
            return Err(error.to_string());
        }
    } else {
        restore_runtime_config(&codex_home()).map_err(|error| error.to_string())?;
    }
    *state.startup_error.write().await = None;
    Ok(json!({"status":"stopped"}))
}

pub async fn stop_codey_runtime(state: &Arc<AppState>) -> Result<Value, String> {
    begin_shutdown(state).await;
    let _operation = state.runtime_operation.lock().await;
    stop_codey_runtime_locked(state).await
}

pub async fn begin_shutdown(state: &Arc<AppState>) {
    state.shutting_down.store(true, Ordering::Release);
    let restart = state.restart_task.lock().await.take();
    if let Some(ScheduledRestart { cancel, task }) = restart {
        let _ = cancel.send(());
        if let Err(error) = task.await {
            error_log::record_failure(
                "runtime_restart_failed",
                "cancel_runtime_restart_during_shutdown",
                error.to_string(),
                json!({}),
            );
        }
    }
    state.restart_in_progress.store(false, Ordering::Release);
}
