use std::sync::Arc;

use serde_json::{Value, json};

use super::AppState;
use crate::codex_config::codex_home;
use crate::crashpad_pending_guard::{self, CrashpadPendingStatsSnapshot};
use crate::error_log;
use crate::trace_log_guard;
use crate::trace_log_stats::{self, TraceLogStatsSnapshot};

pub(super) async fn clear_codex_trace_logs(state: &Arc<AppState>) -> Result<Value, String> {
    let _operation = state.diagnostic_storage_operation.lock().await;
    let home = codex_home();
    let disable_writes = state.config.read().await.disable_trace_log_writes;
    let result = tokio::task::spawn_blocking(move || {
        trace_log_guard::configure(&home, disable_writes)?;
        trace_log_guard::clear(&home)
    })
    .await
    .map_err(|error| format!("Trace 日志库清理任务异常退出：{error}"))
    .and_then(|result| result.map_err(|error| error.to_string()));
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            error_log::record_failure(
                "patch_failed",
                "clear_codex_trace_logs",
                error.clone(),
                json!({
                    "protectionEnabled": disable_writes,
                }),
            );
            return Err(error);
        }
    };
    Ok(json!({
        "status":"ok",
        "cleanup":report,
        "protectionEnabled":disable_writes,
    }))
}

pub(super) async fn clear_diagnostic_storage(state: &Arc<AppState>) -> Result<Value, String> {
    let _operation = state.diagnostic_storage_operation.lock().await;
    let config = state.config.read().await;
    let disable_trace_writes = config.disable_trace_log_writes;
    let protect_crashpad_pending = config.protect_crashpad_pending;
    drop(config);

    let trace_home = codex_home();
    let trace_task = tokio::task::spawn_blocking(move || {
        let cleanup = trace_log_guard::configure(&trace_home, disable_trace_writes)
            .and_then(|_| trace_log_guard::clear(&trace_home));
        let snapshot = trace_log_stats::snapshot(&trace_home);
        (cleanup, snapshot)
    });
    let crashpad_task = tokio::task::spawn_blocking(move || {
        crashpad_pending_guard::clear_system(protect_crashpad_pending)
    });
    let (trace_result, crashpad_result) = tokio::join!(trace_task, crashpad_task);

    let mut errors = Vec::new();
    let (trace_cleanup, trace_snapshot) = match trace_result {
        Ok((Ok(cleanup), snapshot)) => (Some(cleanup), snapshot),
        Ok((Err(error), snapshot)) => {
            let error = format!("{error:#}");
            error_log::record_failure(
                "cleanup_failed",
                "clear_diagnostic_trace_logs",
                error.clone(),
                json!({
                    "protectionEnabled": disable_trace_writes,
                }),
            );
            errors.push(error);
            (None, snapshot)
        }
        Err(error) => {
            let error = format!("Trace 日志库清理任务异常退出：{error}");
            error_log::record_failure(
                "cleanup_failed",
                "clear_diagnostic_trace_logs",
                error.clone(),
                json!({
                    "protectionEnabled": disable_trace_writes,
                    "taskJoinFailed": true,
                }),
            );
            errors.push(error.clone());
            let mut snapshot = TraceLogStatsSnapshot::idle();
            snapshot.errors.push(error);
            (None, snapshot)
        }
    };
    state.trace_log_stats.replace(trace_snapshot);

    let (crashpad_cleanup, crashpad_snapshot) = match crashpad_result {
        Ok(run) => {
            if !run.cleanup.errors.is_empty() {
                let error = format!(
                    "{} 个 Crashpad 待处理文件未能完成清理",
                    run.cleanup.errors.len()
                );
                error_log::record_failure(
                    "cleanup_failed",
                    "clear_crashpad_pending",
                    error,
                    json!({
                        "protectionEnabled": protect_crashpad_pending,
                        "errorCount": run.cleanup.errors.len(),
                    }),
                );
            }
            errors.extend(run.cleanup.errors.iter().cloned());
            (run.cleanup, run.snapshot)
        }
        Err(error) => {
            let error = format!("Crashpad 待处理报告清理任务异常退出：{error}");
            error_log::record_failure(
                "cleanup_failed",
                "clear_crashpad_pending",
                error.clone(),
                json!({
                    "protectionEnabled": protect_crashpad_pending,
                    "taskJoinFailed": true,
                }),
            );
            errors.push(error.clone());
            let mut cleanup = crashpad_pending_guard::CrashpadCleanupReport::default();
            cleanup.errors.push(error.clone());
            let mut snapshot = CrashpadPendingStatsSnapshot::idle(protect_crashpad_pending);
            snapshot.errors.push(error);
            (cleanup, snapshot)
        }
    };
    state.crashpad_pending_stats.replace(crashpad_snapshot);

    Ok(json!({
        "status": if errors.is_empty() { "ok" } else { "partial" },
        "traceCleanup": trace_cleanup,
        "crashpadCleanup": crashpad_cleanup,
        "traceProtectionEnabled": disable_trace_writes,
        "crashpadProtectionEnabled": protect_crashpad_pending,
        "errors": errors,
        "traceLogStats": &state.trace_log_stats,
        "crashpadPendingStats": &state.crashpad_pending_stats,
    }))
}

pub(super) async fn refresh_diagnostic_storage_stats(
    state: &Arc<AppState>,
) -> Result<Value, String> {
    let _operation = state.diagnostic_storage_operation.lock().await;
    let protect_crashpad_pending = state.config.read().await.protect_crashpad_pending;
    if !state.trace_log_stats.begin_refresh() {
        return Ok(json!({
            "status": "pending",
            "traceLogStats": &state.trace_log_stats,
            "crashpadPendingStats": &state.crashpad_pending_stats,
        }));
    }
    let _ = state
        .crashpad_pending_stats
        .begin_refresh(protect_crashpad_pending);

    let trace_home = codex_home();
    let trace_task = tokio::task::spawn_blocking(move || trace_log_stats::snapshot(&trace_home));
    let crashpad_task = tokio::task::spawn_blocking(move || {
        crashpad_pending_guard::snapshot_system(protect_crashpad_pending)
    });
    let (trace_result, crashpad_result) = tokio::join!(trace_task, crashpad_task);

    let trace_snapshot = match trace_result {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let mut snapshot = TraceLogStatsSnapshot::idle();
            snapshot
                .errors
                .push(format!("Trace 日志统计任务异常退出：{error}"));
            snapshot
        }
    };
    state.trace_log_stats.replace(trace_snapshot);

    let crashpad_snapshot = match crashpad_result {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let mut snapshot = CrashpadPendingStatsSnapshot::idle(protect_crashpad_pending);
            snapshot
                .errors
                .push(format!("Crashpad 待处理报告统计任务异常退出：{error}"));
            snapshot
        }
    };
    state.crashpad_pending_stats.replace(crashpad_snapshot);

    Ok(json!({
        "status": "ok",
        "traceLogStats": &state.trace_log_stats,
        "crashpadPendingStats": &state.crashpad_pending_stats,
    }))
}

pub(super) async fn refresh_trace_log_stats(state: &Arc<AppState>) -> Result<Value, String> {
    let _operation = state.diagnostic_storage_operation.lock().await;
    if !state.trace_log_stats.begin_refresh() {
        return Ok(json!({
            "status": "pending",
            "traceLogStats": &state.trace_log_stats,
        }));
    }

    let home = codex_home();
    let snapshot = match tokio::task::spawn_blocking(move || trace_log_stats::snapshot(&home)).await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let mut snapshot = TraceLogStatsSnapshot::idle();
            snapshot
                .errors
                .push(format!("Trace 日志统计任务异常退出：{error}"));
            snapshot
        }
    };
    state.trace_log_stats.replace(snapshot);

    Ok(json!({
        "status": "ok",
        "traceLogStats": &state.trace_log_stats,
    }))
}
