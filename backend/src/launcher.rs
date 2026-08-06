use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
#[cfg(all(test, unix))]
use std::{collections::HashSet, path::Path};

use anyhow::{Context, Result};
use codey_runtime_core::app_paths::resolve_codex_app_dir_with_saved;
use codey_runtime_core::launcher::{
    ProtocolProxyHandle, build_codex_command, start_protocol_proxy,
};
use codey_runtime_core::settings::{BackendSettings, RelayMode, RelayProfile, RelayProtocol};
use codey_runtime_data::{ProviderSyncResult, ProviderSyncStatus};
use serde::Serialize;
use tokio::process::Child;
#[cfg(not(windows))]
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock, oneshot};

use crate::cc_switch::{self, RouteTakeoverState};
use crate::cdp;
use crate::codex_config::{
    RuntimeProviderConfigOptions, apply_runtime_provider_config, codex_home,
    current_model_provider, restore_runtime_provider_config,
};
use crate::config::{CodeyConfig, GpuLaunchMode, ProviderProfile};
use crate::crashpad_pending_guard::{self, CrashpadPendingStatsHandle};
use crate::error_log;
use crate::maintenance_lock;
use crate::model_catalog;
use crate::pet_slim_patch;
use crate::plugin_marketplace;
use crate::provider_lease;
use crate::session_index_cleanup::{self, SessionIndexCleanupReport};
use crate::startup_maintenance::{self, ProviderSyncPlan};
use crate::trace_log_guard;

mod platform;

use platform::*;

const CDP_WATCHDOG_INTERVAL: Duration = Duration::from_secs(30);
const CDP_WATCHDOG_FAILURE_THRESHOLD: u8 = 2;
const ROUTE_OVERLAY_WATCH_INTERVAL: Duration = Duration::from_secs(1);
pub const CODEX_APP_NOT_FOUND_ERROR: &str = "找不到 Codex App，请在 Codey 配置中填写路径";
pub const CODEX_APP_PATH_INVALID_ERROR: &str = "配置的 Codex App 路径无效或指向了 Codex CLI；请选择 Codex 桌面 App，不要选择 codex.exe 命令行程序";
const DISABLE_GPU_ARGUMENT: &str = "--disable-gpu";
const DISABLE_GPU_RASTERIZATION_ARGUMENT: &str = "--disable-gpu-rasterization";
const DEFAULT_CHINESE_LOCALE_ARGUMENT: &str = "--lang=zh-CN";

#[cfg(windows)]
pub fn needs_codex_app_path_selection(startup_error: Option<&str>) -> bool {
    startup_error.is_some_and(|error| {
        error.contains(CODEX_APP_NOT_FOUND_ERROR) || error.contains(CODEX_APP_PATH_INVALID_ERROR)
    })
}

#[cfg(all(test, windows))]
mod app_path_selection_tests {
    use super::*;

    #[test]
    fn path_selection_is_requested_only_for_app_path_failures() {
        assert!(needs_codex_app_path_selection(Some(
            CODEX_APP_NOT_FOUND_ERROR
        )));
        assert!(needs_codex_app_path_selection(Some(
            CODEX_APP_PATH_INVALID_ERROR
        )));
        assert!(!needs_codex_app_path_selection(Some("网络不可用")));
        assert!(!needs_codex_app_path_selection(None));
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceStatus {
    pub session_status: String,
    pub session_detail: String,
    pub session_files_fixed: usize,
    pub sqlite_rows_updated: usize,
    pub ghost_tasks_pruned: usize,
    pub plugin_status: String,
    pub plugin_detail: String,
    pub performance_status: String,
    pub performance_detail: String,
}

struct SessionMaintenanceSummary {
    status: String,
    detail: String,
    files_fixed: usize,
    sqlite_rows_updated: usize,
    ghost_tasks_pruned: usize,
}

fn mark_pet_slim_startup_failure(
    statuses: Arc<[cdp::InjectionScriptStatus]>,
    detail: &str,
) -> Arc<[cdp::InjectionScriptStatus]> {
    let mut statuses = statuses.as_ref().to_vec();
    let Some(pet_status) = statuses
        .iter_mut()
        .find(|status| status.id == "pet-control-shield")
    else {
        return Arc::from(statuses);
    };
    pet_status.status = "failed".to_string();
    pet_status.detail = None;
    pet_status.error = Some(detail.to_string());
    Arc::from(statuses)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeModelConfig {
    selected_models: Vec<String>,
    upstream_models: Vec<String>,
    default_model: Option<String>,
}

impl RuntimeModelConfig {
    pub fn from_config(config: &CodeyConfig) -> Self {
        Self {
            selected_models: config.selected_models().to_vec(),
            upstream_models: config.upstream_models().to_vec(),
            default_model: config.default_model().map(ToString::to_string),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeSubagentConfig {
    model: String,
    reasoning_effort: String,
}

impl RuntimeSubagentConfig {
    pub fn from_config(config: &CodeyConfig) -> Self {
        Self {
            model: config.subagent_model.clone(),
            reasoning_effort: config.subagent_reasoning_effort.clone(),
        }
    }
}

pub struct CodeyRuntime {
    pub codex_app_path: PathBuf,
    pub maintenance: MaintenanceStatus,
    pub applied_config: CodeyConfig,
    applied_model_config: RwLock<RuntimeModelConfig>,
    applied_subagent_config: RwLock<RuntimeSubagentConfig>,
    pub injection_statuses: Arc<RwLock<Arc<[cdp::InjectionScriptStatus]>>>,
    injection_scripts: cdp::PreparedInjectionScripts,
    injection_websocket_url: Arc<RwLock<Arc<str>>>,
    child: Arc<Mutex<Option<Child>>>,
    process_id: Option<u32>,
    #[cfg(unix)]
    process_group_id: Option<u32>,
    #[cfg(target_os = "macos")]
    inspector_argument: Option<String>,
    watchdog_shutdown: Mutex<Option<oneshot::Sender<()>>>,
    watchdog_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    route_overlay_shutdown: Mutex<Option<oneshot::Sender<()>>>,
    route_overlay_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    exit_watchdog_shutdown: Mutex<Option<oneshot::Sender<()>>>,
    exit_watchdog_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    crashpad_guard_enabled: Arc<AtomicBool>,
    crashpad_guard_shutdown: Mutex<Option<oneshot::Sender<()>>>,
    crashpad_guard_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    protocol_proxy: Mutex<Option<ProtocolProxyHandle>>,
}

fn protocol_proxy_settings(
    profile: &ProviderProfile,
    default_model: Option<&str>,
) -> Option<BackendSettings> {
    if profile.cc_switch_read_only || profile.protocol != RelayProtocol::ChatCompletions {
        return None;
    }
    let base_url = profile.normalized_base_url();
    let relay = RelayProfile {
        id: profile.id.clone(),
        name: profile.name.clone(),
        model: default_model.unwrap_or_default().to_string(),
        base_url: base_url.clone(),
        upstream_base_url: base_url,
        api_key: profile.api_key.clone(),
        protocol: RelayProtocol::ChatCompletions,
        relay_mode: RelayMode::PureApi,
        ..RelayProfile::default()
    };
    Some(BackendSettings {
        active_relay_id: relay.id.clone(),
        relay_profiles: vec![relay],
        enhancements_enabled: false,
        ..BackendSettings::default()
    })
}

async fn start_runtime_protocol_proxy(
    profile: &ProviderProfile,
    default_model: Option<&str>,
) -> Result<Option<ProtocolProxyHandle>> {
    let Some(settings) = protocol_proxy_settings(profile, default_model) else {
        return Ok(None);
    };
    start_protocol_proxy(settings)
        .await
        .map(Some)
        .context("启动 Chat Completions 本地协议代理失败")
}

async fn resolve_startup_provider(home: &std::path::Path) -> Result<String> {
    let provider_home = home.to_path_buf();
    tokio::task::spawn_blocking(move || current_model_provider(&provider_home))
        .await
        .map_err(|error| {
            let error = anyhow::Error::new(error).context("读取当前模型 Provider 任务异常退出");
            error_log::record_failure(
                "patch_failed",
                "read_current_model_provider",
                format!("{error:#}"),
                serde_json::json!({
                    "codexHome": home,
                    "taskJoinFailed": true,
                }),
            );
            error
        })?
        .map_err(|error| {
            error_log::record_failure(
                "patch_failed",
                "read_current_model_provider",
                format!("{error:#}"),
                serde_json::json!({
                    "codexHome": home,
                }),
            );
            error
        })
}

async fn run_startup_session_maintenance(
    home: &std::path::Path,
    provider: &str,
) -> Result<SessionMaintenanceSummary> {
    let maintenance_home = home.to_path_buf();
    let maintenance_provider = provider.to_string();
    let maintenance_result = tokio::task::spawn_blocking(move || {
        let stale_lock_recovery = maintenance_lock::recover_stale_locks(&maintenance_home);
        let provider_sync =
            match startup_maintenance::provider_sync_plan(&maintenance_home, &maintenance_provider)
            {
                Ok(ProviderSyncPlan::Cached) => {
                    startup_maintenance::cached_provider_sync_result(&maintenance_provider)
                }
                Ok(ProviderSyncPlan::Full) | Err(_) => {
                    let result = codey_runtime_data::run_provider_sync_with_target(
                        Some(&maintenance_home),
                        Some(&maintenance_provider),
                    );
                    if result.status == ProviderSyncStatus::Synced
                        && result.skipped_locked_rollout_files.is_empty()
                        && let Err(error) = startup_maintenance::record_provider_sync_success(
                            &maintenance_home,
                            &maintenance_provider,
                        )
                    {
                        error_log::record_failure(
                            "patch_failed",
                            "record_provider_sync_success",
                            format!("{error:#}"),
                            serde_json::json!({
                                "provider": maintenance_provider,
                            }),
                        );
                        eprintln!("保存 Provider 同步状态失败：{error:#}");
                    }
                    result
                }
            };
        // `session_index.jsonl` is also cleaned before spawn, while its
        // source snapshot is stable. The original file is backed up.
        let index_cleanup = session_index_cleanup::cleanup(&maintenance_home);
        (stale_lock_recovery, provider_sync, index_cleanup)
    })
    .await;
    let (stale_lock_recovery, provider_sync, index_cleanup) = match maintenance_result {
        Ok(result) => result,
        Err(error) => {
            let error = anyhow::Error::new(error).context("启动前会话修复任务异常退出");
            error_log::record_failure(
                "patch_failed",
                "run_startup_session_repairs",
                format!("{error:#}"),
                serde_json::json!({
                    "codexHome": home,
                }),
            );
            return Err(error);
        }
    };
    match stale_lock_recovery {
        Ok(recovered) => {
            for path in recovered {
                eprintln!("已清理陈旧维护锁：{}", path.display());
            }
        }
        Err(error) => {
            error_log::record_failure(
                "patch_failed",
                "recover_stale_maintenance_locks",
                format!("{error:#}"),
                serde_json::json!({
                    "codexHome": home,
                }),
            );
            eprintln!("清理陈旧维护锁失败：{error:#}");
        }
    }
    if provider_sync.status != ProviderSyncStatus::Synced {
        error_log::record_failure(
            "patch_failed",
            "sync_session_providers",
            provider_sync.message.clone(),
            serde_json::json!({
                "status": format!("{:?}", provider_sync.status),
                "targetProvider": provider_sync.target_provider,
                "skippedLockedFiles": provider_sync.skipped_locked_rollout_files.len(),
            }),
        );
    } else if !provider_sync.skipped_locked_rollout_files.is_empty() {
        error_log::record_failure(
            "patch_failed",
            "sync_session_providers",
            format!(
                "跳过 {} 个被占用的会话文件",
                provider_sync.skipped_locked_rollout_files.len()
            ),
            serde_json::json!({
                "targetProvider": provider_sync.target_provider,
                "skippedLockedFiles": provider_sync.skipped_locked_rollout_files,
            }),
        );
    }
    if let Err(error) = &index_cleanup {
        error_log::record_failure(
            "patch_failed",
            "cleanup_session_index",
            format!("{error:#}"),
            serde_json::json!({
                "codexHome": home,
            }),
        );
    }
    Ok(session_maintenance_summary(&provider_sync, &index_cleanup))
}

async fn resolve_configured_codex_app_dir(config: &CodeyConfig) -> Result<PathBuf> {
    let configured_app_path = config.codex_app_path.trim();
    let configured_app_path_is_empty = configured_app_path.is_empty();
    let configured_app_path =
        (!configured_app_path_is_empty).then(|| PathBuf::from(configured_app_path));
    tokio::task::spawn_blocking(move || {
        resolve_codex_app_dir_with_saved(configured_app_path.as_deref(), None)
    })
    .await
    .map_err(|error| anyhow::Error::new(error).context("定位 Codex App 任务异常退出"))?
    .ok_or_else(|| {
        if configured_app_path_is_empty {
            anyhow::anyhow!(CODEX_APP_NOT_FOUND_ERROR)
        } else {
            anyhow::anyhow!(CODEX_APP_PATH_INVALID_ERROR)
        }
    })
}

struct CodexStartupStateOptions<'a> {
    original_provider: &'a str,
    preserve_provider_route: bool,
    protocol_proxy_base_url: Option<&'a str>,
    expected_config: Option<&'a [u8]>,
}

async fn prepare_startup_model_catalog(
    config: &CodeyConfig,
    current_profile: &ProviderProfile,
    app_dir: &std::path::Path,
    home: &std::path::Path,
    preserve_provider_route: bool,
) -> Result<(bool, String)> {
    if preserve_provider_route {
        return Ok((false, String::new()));
    }

    let catalog_home = home.to_path_buf();
    let official_provider = current_profile.cc_switch_read_only;
    let upstream_models = config.upstream_models_snapshot().map(<[String]>::to_vec);
    let selected_models = config.selected_models().to_vec();
    let manual_models = config.manual_third_party_models().to_vec();
    let requested_default_model = config.default_model().map(str::to_owned);
    let app_dir = app_dir.to_path_buf();
    let configured_app_path = config.codex_app_path.clone();
    let (refresh_result, catalog_available, selection_result) =
        tokio::task::spawn_blocking(move || {
            let codex_runtime_version =
                model_catalog::desktop_runtime_version(Some(&app_dir), &configured_app_path);
            let refresh = model_catalog::refresh_for_provider(
                &catalog_home,
                official_provider,
                upstream_models.as_deref(),
                &selected_models,
                codex_runtime_version.as_deref(),
            );
            let catalog_available = refresh.is_err() && model_catalog::is_available(&catalog_home);
            let selection = model_catalog::selection_state_with_manual_models(
                &catalog_home,
                official_provider,
                upstream_models.as_deref(),
                &selected_models,
                &manual_models,
                requested_default_model.as_deref(),
            );
            (refresh, catalog_available, selection)
        })
        .await
        .map_err(|error| {
            let error = anyhow::Error::new(error).context("准备模型目录任务异常退出");
            error_log::record_failure(
                "patch_failed",
                "prepare_model_catalog",
                format!("{error:#}"),
                serde_json::json!({
                    "officialProvider": official_provider,
                    "taskJoinFailed": true,
                }),
            );
            error
        })?;

    let use_official_catalog = match refresh_result {
        Ok(_) => true,
        Err(error) if model_catalog::is_runtime_model_cache_unavailable(&error) => {
            if catalog_available {
                eprintln!("本机官方模型缓存暂不含自定义目录必需字段，沿用上一份合法镜像");
            } else {
                eprintln!("本机官方模型缓存暂不含自定义目录必需字段，使用 Codex 内置模型目录");
            }
            catalog_available
        }
        Err(error) if catalog_available => {
            error_log::record_failure(
                "patch_failed",
                "refresh_model_catalog",
                format!("{error:#}"),
                serde_json::json!({
                    "fallback": "last_valid_catalog",
                    "officialProvider": official_provider,
                }),
            );
            eprintln!("刷新官方账号模型目录失败，沿用上一份合法镜像：{error:#}");
            true
        }
        Err(error) => {
            error_log::record_failure(
                "patch_failed",
                "refresh_model_catalog",
                format!("{error:#}"),
                serde_json::json!({
                    "fallback": "codex_builtin_catalog",
                    "officialProvider": official_provider,
                }),
            );
            eprintln!("刷新官方账号模型目录失败，临时使用 Codex 内置目录：{error:#}");
            false
        }
    };
    let default_model = match selection_result {
        Ok(state) => state.default_model,
        Err(error) => {
            error_log::record_failure(
                "patch_failed",
                "read_model_catalog_selection",
                format!("{error:#}"),
                serde_json::json!({
                    "fallback": "empty_default_model",
                    "officialProvider": official_provider,
                }),
            );
            String::new()
        }
    };
    Ok((use_official_catalog, default_model))
}

async fn prepare_codex_startup_state(
    config: &CodeyConfig,
    current_profile: &ProviderProfile,
    app_dir: &std::path::Path,
    home: &std::path::Path,
    options: CodexStartupStateOptions<'_>,
) -> Result<Vec<u8>> {
    let CodexStartupStateOptions {
        original_provider,
        preserve_provider_route,
        protocol_proxy_base_url,
        expected_config,
    } = options;
    let (use_official_catalog, default_model) = prepare_startup_model_catalog(
        config,
        current_profile,
        app_dir,
        home,
        preserve_provider_route,
    )
    .await?;
    let runtime_config_home = home.to_path_buf();
    let runtime_config_profile = current_profile.clone();
    let runtime_config_provider = original_provider.to_string();
    let runtime_default_model = (!default_model.is_empty()).then_some(default_model);
    let fast_context_tools = config.fast_context_tools;
    let subagent_optimization = config.subagent_optimization;
    let subagent_model = config.subagent_model.clone();
    let subagent_reasoning_effort = config.subagent_reasoning_effort.clone();
    let protocol_proxy_base_url = protocol_proxy_base_url.map(str::to_string);
    let protocol_proxy_enabled = protocol_proxy_base_url.is_some();
    let expected_config = expected_config.map(<[u8]>::to_vec);
    let runtime_config = tokio::task::spawn_blocking(move || {
        apply_runtime_provider_config(
            &runtime_config_home,
            &runtime_config_profile,
            &runtime_config_provider,
            RuntimeProviderConfigOptions {
                use_official_catalog,
                default_model: runtime_default_model.as_deref(),
                fast_context_tools,
                subagent_optimization,
                subagent_model: &subagent_model,
                subagent_reasoning_effort: &subagent_reasoning_effort,
                preserve_provider_route,
                protocol_proxy_base_url: protocol_proxy_base_url.as_deref(),
                expected_config: expected_config.as_deref(),
            },
        )
    })
    .await
    .map_err(|error| {
        let error = anyhow::Error::new(error).context("应用运行时 Provider 配置任务异常退出");
        error_log::record_failure(
            "patch_failed",
            "apply_runtime_provider_config",
            format!("{error:#}"),
            serde_json::json!({
                "profile": current_profile.name,
                "provider": original_provider,
                "fastContextTools": config.fast_context_tools,
                "subagentOptimization": config.subagent_optimization,
                "preserveProviderRoute": preserve_provider_route,
                "protocolProxyEnabled": protocol_proxy_enabled,
                "taskJoinFailed": true,
            }),
        );
        error
    })?;
    let applied = runtime_config.map_err(|error| {
        error_log::record_failure(
            "patch_failed",
            "apply_runtime_provider_config",
            format!("{error:#}"),
            serde_json::json!({
                "profile": current_profile.name,
                "provider": original_provider,
                "fastContextTools": config.fast_context_tools,
                "subagentOptimization": config.subagent_optimization,
                "preserveProviderRoute": preserve_provider_route,
                "protocolProxyEnabled": protocol_proxy_enabled,
            }),
        );
        error
    })?;
    Ok(applied.config_contents)
}

async fn await_initial_storage_guards(
    initial_trace_guard: tokio::task::JoinHandle<Result<trace_log_guard::TraceLogGuardReport>>,
    disable_trace_log_writes: bool,
    initial_crashpad_guard: tokio::task::JoinHandle<crashpad_pending_guard::CrashpadGuardRun>,
    protect_crashpad_pending: bool,
    crashpad_pending_stats: &CrashpadPendingStatsHandle,
) -> Result<()> {
    match initial_trace_guard.await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            error_log::record_failure(
                "patch_failed",
                "configure_trace_log_guard",
                format!("{error:#}"),
                serde_json::json!({
                    "disabled": disable_trace_log_writes,
                }),
            );
            return Err(error);
        }
        Err(error) => {
            let error = anyhow::Error::new(error).context("Trace 日志保护切换任务异常退出");
            error_log::record_failure(
                "patch_failed",
                "configure_trace_log_guard",
                format!("{error:#}"),
                serde_json::json!({
                    "disabled": disable_trace_log_writes,
                }),
            );
            return Err(error);
        }
    }

    match initial_crashpad_guard.await {
        Ok(run) => {
            if !run.cleanup.errors.is_empty() || run.cleanup.still_over_limit {
                error_log::record_failure(
                    "cleanup_failed",
                    "enforce_crashpad_pending_limit_at_startup",
                    if run.cleanup.still_over_limit {
                        "Crashpad pending 仍超过安全上限".to_string()
                    } else {
                        format!(
                            "{} 个 Crashpad 待处理文件未能完成收敛",
                            run.cleanup.errors.len()
                        )
                    },
                    serde_json::json!({
                        "errorCount": run.cleanup.errors.len(),
                        "stillOverLimit": run.cleanup.still_over_limit,
                        "bytesReclaimed": run.cleanup.bytes_reclaimed,
                    }),
                );
            }
            crashpad_pending_stats.replace(run.snapshot);
        }
        Err(error) => {
            let error = format!("Crashpad 磁盘保护任务异常退出：{error}");
            error_log::record_failure(
                "cleanup_failed",
                "enforce_crashpad_pending_limit_at_startup",
                error.clone(),
                serde_json::json!({
                    "taskJoinFailed": true,
                }),
            );
            let mut snapshot = crashpad_pending_guard::CrashpadPendingStatsSnapshot::idle(
                protect_crashpad_pending,
            );
            snapshot.errors.push(error);
            crashpad_pending_stats.replace(snapshot);
        }
    }
    Ok(())
}

type PetSlimTaskResult =
    std::result::Result<Result<pet_slim_patch::PetSlimReport>, tokio::task::JoinError>;

async fn read_startup_patch_status(
    home: &std::path::Path,
    slim_codex_pet: bool,
) -> (String, String, PetSlimTaskResult) {
    let marketplace_home = home.to_path_buf();
    let marketplace_task = tokio::task::spawn_blocking(move || {
        plugin_marketplace::marketplaces_status(&marketplace_home)
    });
    let pet_home = home.to_path_buf();
    let pet_task =
        tokio::task::spawn_blocking(move || pet_slim_patch::configure(&pet_home, slim_codex_pet));
    let (marketplace_result, pet_result) = tokio::join!(marketplace_task, pet_task);
    let (plugin_status, plugin_detail) = match marketplace_result {
        Ok(status)
            if !status
                .get("needsRepair")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true) =>
        {
            (
                "ready".to_string(),
                "插件市场状态正常；启动时未执行修复".to_string(),
            )
        }
        Ok(_) => (
            "needs_repair".to_string(),
            "插件市场需要修复；可在 Codey 配置页手动处理".to_string(),
        ),
        Err(error) => {
            error_log::record_failure(
                "patch_status_failed",
                "read_plugin_marketplace_status",
                error.to_string(),
                serde_json::json!({}),
            );
            (
                "error".to_string(),
                format!("插件市场状态任务异常退出：{error}"),
            )
        }
    };
    (plugin_status, plugin_detail, pet_result)
}

async fn stop_runtime_watcher(
    shutdown: &Mutex<Option<oneshot::Sender<()>>>,
    task: &Mutex<Option<tokio::task::JoinHandle<()>>>,
    failure_event: &'static str,
    failure_operation: &'static str,
    failure_message: &'static str,
) {
    if let Some(sender) = shutdown.lock().await.take() {
        let _ = sender.send(());
    }
    let task = task.lock().await.take();
    if let Some(task) = task
        && let Err(error) = task.await
    {
        error_log::record_failure(
            failure_event,
            failure_operation,
            error.to_string(),
            serde_json::json!({}),
        );
        eprintln!("{failure_message}：{error}");
    }
}

async fn stop_codex_processes(
    app_dir: &std::path::Path,
    process_id: Option<u32>,
    #[cfg(unix)] process_group_id: Option<u32>,
    #[cfg(target_os = "macos")] inspector_argument: Option<&str>,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if let Some(inspector_argument) = inspector_argument {
            return stop_macos_codex(inspector_argument, app_dir, process_id, process_group_id)
                .await;
        }
        terminate_unix_codex_processes(app_dir, process_id, process_group_id, None)
            .await
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        terminate_unix_codex_processes(app_dir, process_id, process_group_id, None)
            .await
            .map(|_| ())
    }
    #[cfg(windows)]
    {
        terminate_windows_codex_processes(app_dir, process_id).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (app_dir, process_id);
        Ok(())
    }
}

fn injection_failure_cleanup_operation() -> &'static str {
    #[cfg(windows)]
    {
        "cleanup_windows_after_injection_failure"
    }
    #[cfg(target_os = "macos")]
    {
        "cleanup_macos_after_injection_failure"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "cleanup_unix_after_injection_failure"
    }
    #[cfg(not(any(unix, windows)))]
    {
        "cleanup_after_injection_failure"
    }
}

async fn inject_initial_renderer(
    debug_port: u16,
    handler: codey_runtime_core::bridge::BridgeHandler,
    injection_scripts: &cdp::PreparedInjectionScripts,
    app_dir: &std::path::Path,
    home: &std::path::Path,
    spawned: &SpawnedCodex,
    child: &Arc<Mutex<Option<Child>>>,
) -> Result<cdp::InjectedTarget> {
    let failure = match cdp::retry_inject_with_scripts(debug_port, handler, injection_scripts).await
    {
        Ok(target) => return Ok(target),
        Err(failure) => failure,
    };
    let error_message = format!("{failure:#}");
    let failure_metadata = error_log::FailureMetadata {
        stage: Some("startup.renderer_injection".to_string()),
        duration_ms: Some(failure.duration_ms()),
        attempts: Some(failure.attempts()),
        timeout_ms: Some(failure.timeout_ms()),
        recoverable: Some(false),
    };
    let error = failure.into_error();
    error_log::record_failure_with_metadata(
        "injection_failed",
        "inject_cdp_bridge",
        error_message,
        failure_metadata,
        serde_json::json!({
            "appPath": app_dir,
            "debugPort": debug_port,
            "processId": spawned.process_id,
        }),
    );

    if let Err(stop_error) = stop_codex_processes(
        app_dir,
        spawned.process_id,
        #[cfg(unix)]
        spawned.process_group_id,
        #[cfg(target_os = "macos")]
        spawned.inspector_argument.as_deref(),
    )
    .await
    {
        let context = serde_json::json!({
            "appPath": app_dir,
            "processId": spawned.process_id,
        });
        #[cfg(unix)]
        let context = {
            let mut context = context;
            if let Some(context) = context.as_object_mut() {
                context.insert(
                    "processGroupId".to_string(),
                    serde_json::json!(spawned.process_group_id),
                );
            }
            context
        };
        error_log::record_failure(
            "cleanup_failed",
            injection_failure_cleanup_operation(),
            format!("{stop_error:#}"),
            context,
        );
        eprintln!("Codex 注入失败后的进程清理失败：{stop_error:#}");
    }
    if let Some(child) = child.lock().await.take() {
        reap_child_after_cleanup(child, "reap_child_after_injection_failure").await;
    }
    Err(restore_runtime_config_after_error(home, error).await)
}

struct InjectionWatchdog {
    statuses: Arc<RwLock<Arc<[cdp::InjectionScriptStatus]>>>,
    websocket_url: Arc<RwLock<Arc<str>>>,
    shutdown: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

fn spawn_injection_watchdog(
    injected_target: cdp::InjectedTarget,
    debug_port: u16,
    handler: codey_runtime_core::bridge::BridgeHandler,
    injection_scripts: cdp::PreparedInjectionScripts,
) -> InjectionWatchdog {
    let statuses = Arc::new(RwLock::new(injected_target.injection_statuses()));
    let websocket_url = Arc::new(RwLock::new(injected_target.websocket_url_arc()));
    let (shutdown, mut shutdown_rx) = oneshot::channel();
    let watchdog_statuses = statuses.clone();
    let watchdog_websocket_url = websocket_url.clone();
    let watchdog_scripts = injection_scripts.clone();
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(CDP_WATCHDOG_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let mut target = injected_target;
        let mut consecutive_failures = 0u8;
        'watchdog: loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_rx => break,
                _ = interval.tick() => {}
            }
            let healthy = tokio::select! {
                biased;
                _ = &mut shutdown_rx => break 'watchdog,
                result = cdp::is_target_healthy(target.websocket_url()) => {
                    match result {
                        Ok(healthy) => healthy,
                        Err(error) => {
                            error_log::record_failure(
                                "injection_health_check_failed",
                                "check_cdp_bridge_health",
                                format!("{error:#}"),
                                serde_json::json!({
                                    "websocketUrl": target.websocket_url(),
                                }),
                            );
                            false
                        }
                    }
                }
            };
            if !watchdog_should_reinject(&mut consecutive_failures, healthy) {
                continue;
            }
            let reinjection = tokio::select! {
                biased;
                _ = &mut shutdown_rx => break 'watchdog,
                result = cdp::retry_inject_with_scripts(
                    debug_port,
                    handler.clone(),
                    &watchdog_scripts,
                ) => result,
            };
            match reinjection {
                Ok(reinjected) => {
                    let next_statuses = reinjected.injection_statuses();
                    let next_websocket_url = reinjected.websocket_url_arc();
                    let previous = std::mem::replace(&mut target, reinjected);
                    *watchdog_statuses.write().await = next_statuses;
                    *watchdog_websocket_url.write().await = next_websocket_url;
                    previous.close().await;
                    consecutive_failures = 0;
                }
                Err(error) => {
                    let error_message = format!("{error:#}");
                    error_log::record_failure_with_metadata(
                        "injection_failed",
                        "reinject_cdp_bridge",
                        error_message.clone(),
                        error_log::FailureMetadata {
                            stage: Some("runtime.renderer_reinjection".to_string()),
                            duration_ms: Some(error.duration_ms()),
                            attempts: Some(error.attempts()),
                            timeout_ms: Some(error.timeout_ms()),
                            recoverable: Some(true),
                        },
                        serde_json::json!({
                            "debugPort": debug_port,
                        }),
                    );
                    *watchdog_statuses.write().await = watchdog_scripts
                        .statuses_with_error(format!("脚本重新注入失败：{error_message}"));
                    eprintln!("Codey CDP bridge 恢复失败：{error_message}");
                    consecutive_failures = CDP_WATCHDOG_FAILURE_THRESHOLD.saturating_sub(1);
                }
            }
        }
        target.close().await;
    });
    InjectionWatchdog {
        statuses,
        websocket_url,
        shutdown,
        task,
    }
}

struct InitialStorageGuards {
    trace: tokio::task::JoinHandle<Result<trace_log_guard::TraceLogGuardReport>>,
    crashpad: tokio::task::JoinHandle<crashpad_pending_guard::CrashpadGuardRun>,
}

struct StartupRouteContext {
    preserve_provider_route: bool,
    live_route: Option<cc_switch::LiveRouteSnapshot>,
    original_provider: String,
    current_profile: ProviderProfile,
}

struct StartupStorageState {
    app_dir: PathBuf,
    session_maintenance: SessionMaintenanceSummary,
}

struct PreparedProviderState {
    protocol_proxy: Option<ProtocolProxyHandle>,
    applied_route_files: Option<RouteFilesSnapshot>,
}

struct StartupPatchState {
    plugin_status: String,
    plugin_detail: String,
    debug_port: u16,
    route_overlay_shutdown: Option<oneshot::Sender<()>>,
    route_overlay_task: Option<tokio::task::JoinHandle<()>>,
    route_changed: Option<oneshot::Receiver<()>>,
}

struct SpawnedRenderer {
    app_dir: PathBuf,
    spawned: SpawnedCodex,
    child: Arc<Mutex<Option<Child>>>,
    maintenance: MaintenanceStatus,
    injected_target: cdp::InjectedTarget,
}

struct RuntimeWatchers {
    injection_statuses: Arc<RwLock<Arc<[cdp::InjectionScriptStatus]>>>,
    injection_websocket_url: Arc<RwLock<Arc<str>>>,
    watchdog_shutdown: oneshot::Sender<()>,
    watchdog_task: tokio::task::JoinHandle<()>,
    crashpad_guard_enabled: Arc<AtomicBool>,
    crashpad_guard_shutdown: oneshot::Sender<()>,
    crashpad_guard_task: tokio::task::JoinHandle<()>,
    exit_watchdog_shutdown: oneshot::Sender<()>,
    exit_watchdog_task: tokio::task::JoinHandle<()>,
    codex_exit: oneshot::Receiver<()>,
}

struct RuntimeWatcherInputs {
    injected_target: cdp::InjectedTarget,
    debug_port: u16,
    handler: codey_runtime_core::bridge::BridgeHandler,
    injection_scripts: cdp::PreparedInjectionScripts,
    child: Arc<Mutex<Option<Child>>>,
    process_id: Option<u32>,
    protect_crashpad_pending: bool,
    crashpad_pending_stats: CrashpadPendingStatsHandle,
}

fn spawn_initial_storage_guards(
    home: &std::path::Path,
    config: &CodeyConfig,
) -> InitialStorageGuards {
    let trace_guard_home = home.to_path_buf();
    let disable_trace_log_writes = config.disable_trace_log_writes;
    let trace = tokio::task::spawn_blocking(move || {
        trace_log_guard::configure(&trace_guard_home, disable_trace_log_writes)
    });
    let protect_crashpad_pending = config.protect_crashpad_pending;
    let crashpad = tokio::task::spawn_blocking(move || {
        if protect_crashpad_pending {
            crashpad_pending_guard::enforce_system_limit()
        } else {
            crashpad_pending_guard::CrashpadGuardRun {
                cleanup: crashpad_pending_guard::CrashpadCleanupReport::default(),
                snapshot: crashpad_pending_guard::snapshot_system(false),
            }
        }
    });
    InitialStorageGuards { trace, crashpad }
}

async fn resolve_startup_route_context(
    home: &std::path::Path,
    config: &CodeyConfig,
) -> Result<StartupRouteContext> {
    let startup_route_home = home.to_path_buf();
    let startup_route =
        tokio::task::spawn_blocking(move || cc_switch::startup_route_state(&startup_route_home))
            .await
            .map_err(|error| {
                let error =
                    anyhow::Error::new(error).context("检测 CC Switch 路由接管任务异常退出");
                error_log::record_failure(
                    "patch_failed",
                    "detect_cc_switch_route_takeover",
                    format!("{error:#}"),
                    serde_json::json!({
                        "codexHome": home,
                        "taskJoinFailed": true,
                    }),
                );
                error
            })?
            .map_err(|error| {
                error_log::record_failure(
                    "patch_failed",
                    "detect_cc_switch_route_takeover",
                    format!("{error:#}"),
                    serde_json::json!({
                        "codexHome": home,
                    }),
                );
                error
            })?;
    let preserve_provider_route =
        preserve_cc_switch_route(startup_route.takeover).map_err(|error| {
            error_log::record_failure(
                "patch_failed",
                "validate_cc_switch_route_takeover",
                format!("{error:#}"),
                serde_json::json!({
                    "managed": startup_route.takeover.managed,
                    "live": startup_route.takeover.live,
                }),
            );
            error
        })?;
    let live_route = if preserve_provider_route {
        Some(
            startup_route
                .live_route
                .ok_or_else(|| anyhow::anyhow!("CC Switch Live 路由缺少已验证的 Provider 快照"))?,
        )
    } else {
        None
    };
    let original_provider = if let Some(live_route) = live_route.as_ref() {
        live_route.provider_id().to_string()
    } else {
        resolve_startup_provider(home).await?
    };
    let current_profile = if let Some(live_route) = live_route.as_ref() {
        live_route.profile().clone()
    } else {
        config
            .active_profile()
            .ok_or_else(|| anyhow::anyhow!("找不到当前 Codex 线路"))?
    };
    Ok(StartupRouteContext {
        preserve_provider_route,
        live_route,
        original_provider,
        current_profile,
    })
}

async fn prepare_startup_storage(
    home: &std::path::Path,
    config: &CodeyConfig,
    original_provider: &str,
    guards: InitialStorageGuards,
    crashpad_pending_stats: &CrashpadPendingStatsHandle,
) -> Result<StartupStorageState> {
    let app_dir = resolve_configured_codex_app_dir(config).await?;
    // Session repair must never race a live Codex writer. Stopping the old
    // runtime first also gives SQLite and rollout buffers a chance to flush
    // before any permanent maintenance is applied.
    prepare_codex_for_launch(&app_dir).await?;

    // Permanent maintenance runs before Codey creates the temporary
    // direct-provider lease. A lightweight header/SQLite validation normally
    // reuses the last successful provider sync; provider changes still
    // fall back to the complete rollout and SQLite repair.
    let session_maintenance = run_startup_session_maintenance(home, original_provider).await?;
    await_initial_storage_guards(
        guards.trace,
        config.disable_trace_log_writes,
        guards.crashpad,
        config.protect_crashpad_pending,
        crashpad_pending_stats,
    )
    .await?;
    Ok(StartupStorageState {
        app_dir,
        session_maintenance,
    })
}

async fn prepare_runtime_provider_state(
    home: &std::path::Path,
    config: &CodeyConfig,
    route: &StartupRouteContext,
    app_dir: &std::path::Path,
) -> Result<PreparedProviderState> {
    let protocol_proxy =
        start_runtime_protocol_proxy(&route.current_profile, config.default_model())
            .await
            .map_err(|error| {
                error_log::record_failure(
                    "protocol_proxy_start_failed",
                    "start_chat_completions_protocol_proxy",
                    format!("{error:#}"),
                    serde_json::json!({
                        "provider": route.original_provider,
                        "protocol": route.current_profile.protocol,
                    }),
                );
                error
            })?;
    let protocol_proxy_base_url = protocol_proxy
        .as_ref()
        .map(|proxy| proxy.base_url().to_string());
    let applied_config = prepare_codex_startup_state(
        config,
        &route.current_profile,
        app_dir,
        home,
        CodexStartupStateOptions {
            original_provider: &route.original_provider,
            preserve_provider_route: route.preserve_provider_route,
            protocol_proxy_base_url: protocol_proxy_base_url.as_deref(),
            expected_config: route
                .live_route
                .as_ref()
                .map(|route| route.config_contents()),
        },
    )
    .await?;
    let applied_route_files = route
        .live_route
        .as_ref()
        .map(|live_route| RouteFilesSnapshot {
            config: applied_config,
            auth: live_route.auth_contents().map(<[u8]>::to_vec),
        });
    Ok(PreparedProviderState {
        protocol_proxy,
        applied_route_files,
    })
}

async fn prepare_startup_patches_and_overlay(
    home: &std::path::Path,
    config: &CodeyConfig,
    applied_route_files: Option<&RouteFilesSnapshot>,
) -> Result<StartupPatchState> {
    let (route_overlay_shutdown, route_overlay_task, route_changed) =
        if let Some(applied_route_files) = applied_route_files {
            let (shutdown, task, changed) =
                spawn_route_overlay_watcher(home.to_path_buf(), applied_route_files.clone());
            (Some(shutdown), Some(task), Some(changed))
        } else {
            (None, None, None)
        };

    let slim_codex_pet = config.slim_codex_pet;
    let (plugin_status, plugin_detail, pet_result) =
        read_startup_patch_status(home, slim_codex_pet).await;
    let debug_port = codey_runtime_core::ports::select_packaged_codex_debug_port(9229);
    if let Some(applied_route_files) = applied_route_files {
        let current_route_files = read_route_files(home)
            .await
            .with_context(|| "启动 Codex 前复核 CC Switch Live 路由失败")?;
        if current_route_files.as_ref() != Some(applied_route_files) {
            return Err(restore_runtime_config_after_error(
                home,
                anyhow::anyhow!("CC Switch Live 路由在 Codex 启动前发生变化；已取消旧线路启动"),
            )
            .await);
        }
    }
    match pet_result {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            error_log::record_failure(
                "patch_failed",
                "configure_codex_pet_slim",
                format!("{error:#}"),
                serde_json::json!({
                    "enabled": slim_codex_pet,
                }),
            );
            return Err(restore_runtime_config_after_error(
                home,
                error.context("应用 Codex 宠物精简设置失败"),
            )
            .await);
        }
        Err(error) => {
            error_log::record_failure(
                "patch_failed",
                "configure_codex_pet_slim",
                error.to_string(),
                serde_json::json!({
                    "enabled": slim_codex_pet,
                    "taskJoinFailed": true,
                }),
            );
            return Err(restore_runtime_config_after_error(
                home,
                anyhow::Error::new(error).context("Codex 宠物精简设置任务异常退出"),
            )
            .await);
        }
    };
    Ok(StartupPatchState {
        plugin_status,
        plugin_detail,
        debug_port,
        route_overlay_shutdown,
        route_overlay_task,
        route_changed,
    })
}

async fn spawn_and_inject_runtime(
    home: &std::path::Path,
    config: &CodeyConfig,
    handler: &codey_runtime_core::bridge::BridgeHandler,
    injection_scripts: &cdp::PreparedInjectionScripts,
    storage: StartupStorageState,
    patch: &StartupPatchState,
) -> Result<SpawnedRenderer> {
    let mut spawned = match spawn_codex(
        &storage.app_dir,
        patch.debug_port,
        config.slim_codex_pet,
        config.slim_codex_voice,
        config.fast_codex_startup,
        config.gpu_launch_mode,
    )
    .await
    {
        Ok(spawned) => spawned,
        Err(error) => {
            return Err(restore_runtime_config_after_error(home, error).await);
        }
    };
    let maintenance = MaintenanceStatus {
        session_status: storage.session_maintenance.status,
        session_detail: storage.session_maintenance.detail,
        session_files_fixed: storage.session_maintenance.files_fixed,
        sqlite_rows_updated: storage.session_maintenance.sqlite_rows_updated,
        ghost_tasks_pruned: storage.session_maintenance.ghost_tasks_pruned,
        plugin_status: patch.plugin_status.clone(),
        plugin_detail: patch.plugin_detail.clone(),
        performance_status: spawned.performance_status.clone(),
        performance_detail: spawned.performance_detail.clone(),
    };
    let child = Arc::new(Mutex::new(spawned.child.take()));
    let injected_target = inject_initial_renderer(
        patch.debug_port,
        handler.clone(),
        injection_scripts,
        &storage.app_dir,
        home,
        &spawned,
        &child,
    )
    .await?;
    Ok(SpawnedRenderer {
        app_dir: storage.app_dir,
        spawned,
        child,
        maintenance,
        injected_target,
    })
}

fn spawn_runtime_watchers(inputs: RuntimeWatcherInputs) -> RuntimeWatchers {
    let RuntimeWatcherInputs {
        injected_target,
        debug_port,
        handler,
        injection_scripts,
        child,
        process_id,
        protect_crashpad_pending,
        crashpad_pending_stats,
    } = inputs;
    #[cfg(not(windows))]
    let _ = process_id;
    let InjectionWatchdog {
        statuses: injection_statuses,
        websocket_url: injection_websocket_url,
        shutdown: watchdog_shutdown,
        task: watchdog_task,
    } = spawn_injection_watchdog(injected_target, debug_port, handler, injection_scripts);
    let codex_exited = Arc::new(AtomicBool::new(false));
    let crashpad_guard_enabled = Arc::new(AtomicBool::new(protect_crashpad_pending));
    let (crashpad_guard_shutdown, crashpad_guard_task) =
        spawn_crashpad_guard_watcher(crashpad_guard_enabled.clone(), crashpad_pending_stats);
    #[cfg(windows)]
    let (exit_watchdog_shutdown, codex_exit, exit_watchdog_task) =
        spawn_codex_exit_watcher(child, process_id, codex_exited);
    #[cfg(not(windows))]
    let (exit_watchdog_shutdown, codex_exit, exit_watchdog_task) =
        spawn_codex_exit_watcher(child, codex_exited);
    RuntimeWatchers {
        injection_statuses,
        injection_websocket_url,
        watchdog_shutdown,
        watchdog_task,
        crashpad_guard_enabled,
        crashpad_guard_shutdown,
        crashpad_guard_task,
        exit_watchdog_shutdown,
        exit_watchdog_task,
        codex_exit,
    }
}

impl CodeyRuntime {
    pub async fn renderer_websocket_url(&self) -> Arc<str> {
        self.injection_websocket_url.read().await.clone()
    }

    pub async fn applied_model_config(&self) -> RuntimeModelConfig {
        self.applied_model_config.read().await.clone()
    }

    pub async fn mark_model_config_applied(&self, config: &CodeyConfig) {
        *self.applied_model_config.write().await = RuntimeModelConfig::from_config(config);
    }

    pub async fn applied_subagent_config(&self) -> RuntimeSubagentConfig {
        self.applied_subagent_config.read().await.clone()
    }

    pub async fn mark_subagent_config_applied(&self, config: &CodeyConfig) {
        *self.applied_subagent_config.write().await = RuntimeSubagentConfig::from_config(config);
    }

    pub fn set_crashpad_pending_protection(&self, enabled: bool) {
        self.crashpad_guard_enabled
            .store(enabled, Ordering::Release);
    }

    pub fn injection_statuses_for_display(
        &self,
        statuses: Arc<[cdp::InjectionScriptStatus]>,
    ) -> Arc<[cdp::InjectionScriptStatus]> {
        if self.applied_config.slim_codex_pet && self.maintenance.performance_status == "degraded" {
            mark_pet_slim_startup_failure(statuses, &self.maintenance.performance_detail)
        } else {
            statuses
        }
    }

    pub async fn refresh_injection_statuses(&self) -> Arc<[cdp::InjectionScriptStatus]> {
        let websocket_url = self.injection_websocket_url.read().await.clone();
        let statuses = cdp::read_injection_statuses(&websocket_url, &self.injection_scripts)
            .await
            .unwrap_or_else(|error| {
                self.injection_scripts
                    .statuses_with_error(format!("实时生效自检失败：{error:#}"))
            });
        if self.injection_websocket_url.read().await.as_ref() != websocket_url.as_ref() {
            let statuses = self.injection_statuses.read().await.clone();
            return self.injection_statuses_for_display(statuses);
        }
        let statuses = self.injection_statuses_for_display(statuses);
        *self.injection_statuses.write().await = statuses.clone();
        statuses
    }

    pub async fn start(
        config: &CodeyConfig,
        handler: codey_runtime_core::bridge::BridgeHandler,
        crashpad_pending_stats: CrashpadPendingStatsHandle,
    ) -> Result<(Self, oneshot::Receiver<()>, Option<oneshot::Receiver<()>>)> {
        let home = codex_home();
        let injection_scripts = cdp::prepare_injection_scripts(
            config.fast_codex_startup,
            config.slim_codex_pet,
            config.slim_codex_voice,
            config.hide_full_access_warning,
            &config.user_scripts,
        );
        let initial_storage_guards = spawn_initial_storage_guards(&home, config);
        let route = resolve_startup_route_context(&home, config).await?;
        let storage = prepare_startup_storage(
            &home,
            config,
            &route.original_provider,
            initial_storage_guards,
            &crashpad_pending_stats,
        )
        .await?;
        let PreparedProviderState {
            protocol_proxy,
            applied_route_files,
        } = prepare_runtime_provider_state(&home, config, &route, &storage.app_dir).await?;
        let patch =
            prepare_startup_patches_and_overlay(&home, config, applied_route_files.as_ref())
                .await?;
        let SpawnedRenderer {
            app_dir,
            spawned,
            child,
            maintenance,
            injected_target,
        } = spawn_and_inject_runtime(&home, config, &handler, &injection_scripts, storage, &patch)
            .await?;
        #[cfg(target_os = "macos")]
        let inspector_argument = spawned.inspector_argument.clone();
        let process_id = spawned.process_id;
        let RuntimeWatchers {
            injection_statuses,
            injection_websocket_url,
            watchdog_shutdown,
            watchdog_task,
            crashpad_guard_enabled,
            crashpad_guard_shutdown,
            crashpad_guard_task,
            exit_watchdog_shutdown,
            exit_watchdog_task,
            codex_exit,
        } = spawn_runtime_watchers(RuntimeWatcherInputs {
            injected_target,
            debug_port: patch.debug_port,
            handler,
            injection_scripts: injection_scripts.clone(),
            child: child.clone(),
            process_id,
            protect_crashpad_pending: config.protect_crashpad_pending,
            crashpad_pending_stats,
        });
        Ok((
            Self {
                codex_app_path: app_dir,
                maintenance,
                applied_config: config.clone(),
                applied_model_config: RwLock::new(RuntimeModelConfig::from_config(config)),
                applied_subagent_config: RwLock::new(RuntimeSubagentConfig::from_config(config)),
                injection_statuses,
                injection_scripts,
                injection_websocket_url,
                child,
                process_id,
                #[cfg(unix)]
                process_group_id: spawned.process_group_id,
                #[cfg(target_os = "macos")]
                inspector_argument,
                watchdog_shutdown: Mutex::new(Some(watchdog_shutdown)),
                watchdog_task: Mutex::new(Some(watchdog_task)),
                route_overlay_shutdown: Mutex::new(patch.route_overlay_shutdown),
                route_overlay_task: Mutex::new(patch.route_overlay_task),
                exit_watchdog_shutdown: Mutex::new(Some(exit_watchdog_shutdown)),
                exit_watchdog_task: Mutex::new(Some(exit_watchdog_task)),
                crashpad_guard_enabled,
                crashpad_guard_shutdown: Mutex::new(Some(crashpad_guard_shutdown)),
                crashpad_guard_task: Mutex::new(Some(crashpad_guard_task)),
                protocol_proxy: Mutex::new(protocol_proxy),
            },
            codex_exit,
            patch.route_changed,
        ))
    }

    pub async fn stop(&self) -> Result<()> {
        stop_runtime_watcher(
            &self.crashpad_guard_shutdown,
            &self.crashpad_guard_task,
            "cleanup_failed",
            "stop_crashpad_pending_guard",
            "Crashpad 磁盘保护任务关闭失败",
        )
        .await;
        stop_runtime_watcher(
            &self.route_overlay_shutdown,
            &self.route_overlay_task,
            "route_overlay_watch_failed",
            "stop_cc_switch_route_overlay_watcher",
            "CC Switch 路由配置监听器关闭失败",
        )
        .await;
        stop_runtime_watcher(
            &self.watchdog_shutdown,
            &self.watchdog_task,
            "injection_watchdog_failed",
            "stop_cdp_watchdog",
            "Codey CDP watchdog 关闭失败",
        )
        .await;
        stop_runtime_watcher(
            &self.exit_watchdog_shutdown,
            &self.exit_watchdog_task,
            "process_watch_failed",
            "stop_codex_exit_watcher",
            "Codex 退出监听器关闭失败",
        )
        .await;
        let process_stop = stop_codex_processes(
            &self.codex_app_path,
            self.process_id,
            #[cfg(unix)]
            self.process_group_id,
            #[cfg(target_os = "macos")]
            self.inspector_argument.as_deref(),
        )
        .await;

        if let Some(child) = self.child.lock().await.take() {
            reap_child_after_cleanup(child, "reap_child_during_runtime_stop").await;
        }
        let protocol_proxy_stop = if let Some(proxy) = self.protocol_proxy.lock().await.take() {
            proxy.shutdown().await
        } else {
            Ok(())
        };
        let config_restore = restore_runtime_config(&codex_home()).await;
        if let Err(error) = &process_stop {
            error_log::record_failure(
                "cleanup_failed",
                "stop_codex_processes",
                format!("{error:#}"),
                serde_json::json!({
                    "appPath": self.codex_app_path,
                    "processId": self.process_id,
                }),
            );
        }
        if let Err(error) = &protocol_proxy_stop {
            error_log::record_failure(
                "cleanup_failed",
                "stop_chat_completions_protocol_proxy",
                format!("{error:#}"),
                serde_json::json!({}),
            );
        }
        let mut failures = Vec::new();
        if let Err(error) = process_stop {
            failures.push(format!("清理 Codex 遗留进程失败：{error:#}"));
        }
        if let Err(error) = protocol_proxy_stop {
            failures.push(format!("关闭本地协议代理失败：{error:#}"));
        }
        if let Err(error) = config_restore {
            failures.push(format!("恢复 Codex 配置失败：{error:#}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(failures.join("；"))
        }
    }
}

fn preserve_cc_switch_route(state: RouteTakeoverState) -> Result<bool> {
    if state.managed && !state.live {
        anyhow::bail!(
            "检测到 CC Switch 已开启 Codex 路由，但当前 Live 配置未处于接管状态。\
             为避免 Codey 覆盖路由，已停止启动；请在 CC Switch 中关闭并重新开启 Codex 路由后重试"
        );
    }
    Ok(state.live)
}

fn spawn_crashpad_guard_watcher(
    enabled: Arc<AtomicBool>,
    stats: CrashpadPendingStatsHandle,
) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(crashpad_pending_guard::GUARD_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_rx => break,
                _ = interval.tick() => {}
            }
            if !enabled.load(Ordering::Acquire) {
                continue;
            }
            let result =
                tokio::task::spawn_blocking(crashpad_pending_guard::enforce_system_limit).await;
            match result {
                Ok(run) => {
                    if !run.cleanup.errors.is_empty() || run.cleanup.still_over_limit {
                        error_log::record_failure_async(
                            "cleanup_failed",
                            "enforce_crashpad_pending_limit",
                            if run.cleanup.still_over_limit {
                                "Crashpad pending 仍超过安全上限".to_string()
                            } else {
                                format!(
                                    "{} 个 Crashpad 待处理文件未能完成收敛",
                                    run.cleanup.errors.len()
                                )
                            },
                            serde_json::json!({
                                "errorCount": run.cleanup.errors.len(),
                                "stillOverLimit": run.cleanup.still_over_limit,
                                "bytesReclaimed": run.cleanup.bytes_reclaimed,
                            }),
                        )
                        .await;
                    }
                    let _ = stats.replace_if_idle(run.snapshot);
                }
                Err(error) => {
                    error_log::record_failure_async(
                        "cleanup_failed",
                        "enforce_crashpad_pending_limit",
                        error.to_string(),
                        serde_json::json!({
                            "taskJoinFailed": true,
                        }),
                    )
                    .await;
                }
            }
        }
    });
    (shutdown_tx, task)
}

fn spawn_route_overlay_watcher(
    home: PathBuf,
    applied: RouteFilesSnapshot,
) -> (
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
    oneshot::Receiver<()>,
) {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let (route_changed_tx, route_changed_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(ROUTE_OVERLAY_WATCH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let mut pending_external: Option<RouteFilesSnapshot> = None;
        let mut route_changed_tx = Some(route_changed_tx);

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_rx => break,
                _ = interval.tick() => {}
            }
            let current = match read_route_files(&home).await {
                Ok(Some(current)) => current,
                Ok(None) => {
                    pending_external = None;
                    continue;
                }
                Err(error) => {
                    error_log::record_failure_async(
                        "route_overlay_watch_failed",
                        "read_cc_switch_live_route",
                        error.to_string(),
                        serde_json::json!({
                            "codexHome": home,
                        }),
                    )
                    .await;
                    continue;
                }
            };
            if !route_files_changed(&applied, &current) {
                pending_external = None;
                continue;
            }
            if pending_external.as_ref() != Some(&current) {
                pending_external = Some(current);
                continue;
            }

            if let Some(sender) = route_changed_tx.take() {
                let _ = sender.send(());
            }
            break;
        }
    });
    (shutdown_tx, task, route_changed_rx)
}

#[derive(Clone, PartialEq, Eq)]
struct RouteFilesSnapshot {
    config: Vec<u8>,
    auth: Option<Vec<u8>>,
}

async fn read_route_files(home: &std::path::Path) -> std::io::Result<Option<RouteFilesSnapshot>> {
    let config = match tokio::fs::read(home.join("config.toml")).await {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let auth = match tokio::fs::read(home.join("auth.json")).await {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    Ok(Some(RouteFilesSnapshot { config, auth }))
}

fn route_files_changed(applied: &RouteFilesSnapshot, current: &RouteFilesSnapshot) -> bool {
    applied != current
}

fn watchdog_should_reinject(consecutive_failures: &mut u8, healthy: bool) -> bool {
    if healthy {
        *consecutive_failures = 0;
        return false;
    }
    *consecutive_failures = consecutive_failures.saturating_add(1);
    *consecutive_failures >= CDP_WATCHDOG_FAILURE_THRESHOLD
}

fn session_maintenance_summary(
    provider_sync: &ProviderSyncResult,
    index_cleanup: &Result<SessionIndexCleanupReport>,
) -> SessionMaintenanceSummary {
    let mut errors = Vec::new();
    if provider_sync.status != ProviderSyncStatus::Synced {
        errors.push(provider_sync.message.clone());
    }
    if !provider_sync.skipped_locked_rollout_files.is_empty() {
        errors.push(format!(
            "跳过 {} 个被占用的会话文件",
            provider_sync.skipped_locked_rollout_files.len()
        ));
    }
    let pruned_entries = match index_cleanup {
        Ok(report) => report.pruned_entries,
        Err(error) => {
            errors.push(format!("幽灵任务索引清理失败：{error}"));
            0
        }
    };
    let mut detail = format!(
        "已同步到 {}：修复 {} 个会话文件，更新 {} 行数据库索引，清理 {} 条幽灵任务",
        provider_sync.target_provider,
        provider_sync.changed_session_files,
        provider_sync.sqlite_rows_updated,
        pruned_entries,
    );
    if provider_sync.encrypted_content_warning.is_some() {
        detail.push_str("；检测到跨 Provider 加密历史警告");
    }
    if !errors.is_empty() {
        detail.push('；');
        detail.push_str(&errors.join("；"));
    }
    let status = if errors.is_empty() { "ready" } else { "error" };
    SessionMaintenanceSummary {
        status: status.to_string(),
        detail,
        files_fixed: provider_sync.changed_session_files,
        sqlite_rows_updated: provider_sync.sqlite_rows_updated,
        ghost_tasks_pruned: pruned_entries,
    }
}

#[cfg(test)]
mod route_takeover_tests {
    use super::*;

    #[test]
    fn managed_route_with_a_broken_live_config_blocks_startup() {
        let error = preserve_cc_switch_route(RouteTakeoverState {
            managed: true,
            live: false,
        })
        .unwrap_err();

        assert!(format!("{error:#}").contains("关闭并重新开启 Codex 路由"));
    }

    #[test]
    fn live_route_is_preserved_and_normal_config_is_not() {
        assert!(
            preserve_cc_switch_route(RouteTakeoverState {
                managed: true,
                live: true,
            })
            .unwrap()
        );
        assert!(!preserve_cc_switch_route(RouteTakeoverState::default()).unwrap());
    }

    #[test]
    fn route_watcher_restarts_for_config_or_auth_changes() {
        let applied = RouteFilesSnapshot {
            config: b"model_provider = \"route-a\"\n".to_vec(),
            auth: Some(br#"{"OPENAI_API_KEY":"PROXY_MANAGED"}"#.to_vec()),
        };
        let same = RouteFilesSnapshot {
            config: applied.config.clone(),
            auth: applied.auth.clone(),
        };
        let changed_config = RouteFilesSnapshot {
            config: b"model_provider = \"route-b\"\n".to_vec(),
            auth: applied.auth.clone(),
        };
        let changed_auth = RouteFilesSnapshot {
            config: applied.config.clone(),
            auth: Some(br#"{"OPENAI_API_KEY":"new-key"}"#.to_vec()),
        };

        assert!(!route_files_changed(&applied, &same));
        assert!(route_files_changed(&applied, &changed_config));
        assert!(route_files_changed(&applied, &changed_auth));
    }
}

#[cfg(test)]
mod maintenance_status_tests {
    use super::*;

    #[test]
    fn maintenance_status_exposes_structured_session_metrics() {
        let provider_sync = ProviderSyncResult {
            status: ProviderSyncStatus::Synced,
            message: "ok".to_string(),
            target_provider: "openai".to_string(),
            backup_dir: None,
            changed_session_files: 3,
            skipped_locked_rollout_files: Vec::new(),
            sqlite_rows_updated: 7,
            sqlite_provider_rows_updated: 2,
            sqlite_user_event_rows_updated: 3,
            sqlite_cwd_rows_updated: 2,
            updated_workspace_roots: 1,
            encrypted_content_warning: None,
        };
        let cleanup = Ok(SessionIndexCleanupReport {
            scanned_entries: 5,
            live_threads: 3,
            pruned_entries: 2,
            backup_dir: None,
        });

        let summary = session_maintenance_summary(&provider_sync, &cleanup);
        let status = MaintenanceStatus {
            session_status: summary.status,
            session_detail: summary.detail,
            session_files_fixed: summary.files_fixed,
            sqlite_rows_updated: summary.sqlite_rows_updated,
            ghost_tasks_pruned: summary.ghost_tasks_pruned,
            plugin_status: "ready".to_string(),
            plugin_detail: String::new(),
            performance_status: "ready".to_string(),
            performance_detail: String::new(),
        };
        let value = serde_json::to_value(status).unwrap();

        assert_eq!(value["sessionFilesFixed"], 3);
        assert_eq!(value["sqliteRowsUpdated"], 7);
        assert_eq!(value["ghostTasksPruned"], 2);
        assert!(
            value["sessionDetail"]
                .as_str()
                .unwrap()
                .contains("修复 3 个")
        );
    }

    #[test]
    fn pet_slim_startup_failure_overrides_the_renderer_only_probe() {
        let statuses: Arc<[cdp::InjectionScriptStatus]> = Arc::from(vec![
            cdp::InjectionScriptStatus {
                id: "pet-control-shield".to_string(),
                name: "宠物控制精简".to_string(),
                source: "builtin".to_string(),
                status: "effective".to_string(),
                detail: Some("宠物控制精简已启用".to_string()),
                error: None,
            },
            cdp::InjectionScriptStatus {
                id: "bridge-helpers".to_string(),
                name: "桥接辅助".to_string(),
                source: "builtin".to_string(),
                status: "effective".to_string(),
                detail: Some("桥接函数可调用".to_string()),
                error: None,
            },
        ]);

        let statuses =
            mark_pet_slim_startup_failure(statuses, "宠物精简启动补丁未能确认生效，已使用兼容模式");

        assert_eq!(statuses[0].status, "failed");
        assert_eq!(statuses[0].detail, None);
        assert_eq!(
            statuses[0].error.as_deref(),
            Some("宠物精简启动补丁未能确认生效，已使用兼容模式")
        );
        assert_eq!(statuses[1].status, "effective");
        assert_eq!(statuses[1].error, None);
    }
}

pub async fn restore_previous_runtime_state(home: &std::path::Path) -> Result<()> {
    let home = home.to_path_buf();
    tokio::task::spawn_blocking(move || restore_previous_runtime_state_blocking(&home))
        .await
        .context("恢复上次 Codey 运行状态任务异常退出")?
}

fn restore_previous_runtime_state_blocking(home: &std::path::Path) -> Result<()> {
    let provider_result = provider_lease::restore_legacy();
    let config_result = restore_runtime_provider_config(home);
    if let Err(error) = &provider_result {
        error_log::record_failure(
            "restore_failed",
            "restore_legacy_provider_lease",
            format!("{error:#}"),
            serde_json::json!({}),
        );
    }
    if let Err(error) = &config_result {
        error_log::record_failure(
            "restore_failed",
            "restore_runtime_provider_config",
            format!("{error:#}"),
            serde_json::json!({
                "codexHome": home,
            }),
        );
    }
    match (provider_result, config_result) {
        (Ok(_), Ok(_)) => Ok(()),
        (Err(provider), Ok(_)) => Err(provider).context("恢复会话 provider 失败"),
        (Ok(_), Err(config)) => Err(config).context("恢复 Codex 配置失败"),
        (Err(provider), Err(config)) => {
            anyhow::bail!("恢复会话 provider 失败：{provider}；恢复 Codex 配置也失败：{config}")
        }
    }
}

pub async fn restore_runtime_config(home: &std::path::Path) -> Result<()> {
    let home = home.to_path_buf();
    tokio::task::spawn_blocking(move || restore_runtime_config_blocking(&home))
        .await
        .context("恢复 Codey 运行时配置任务异常退出")?
}

fn restore_runtime_config_blocking(home: &std::path::Path) -> Result<()> {
    let result = restore_runtime_provider_config(home)
        .map(|_| ())
        .context("恢复 Codex 配置失败");
    if let Err(error) = &result {
        error_log::record_failure(
            "restore_failed",
            "restore_runtime_provider_config",
            format!("{error:#}"),
            serde_json::json!({
                "codexHome": home,
            }),
        );
    }
    result
}

async fn restore_runtime_config_after_error(
    home: &std::path::Path,
    error: anyhow::Error,
) -> anyhow::Error {
    match restore_runtime_config(home).await {
        Ok(()) => error,
        Err(restore_error) => {
            anyhow::anyhow!("{error:#}；启动失败后恢复临时 Codex 配置也失败：{restore_error:#}")
        }
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildProcessState {
    Running,
    Exited,
    Untracked,
}

#[cfg(windows)]
async fn child_process_state(child: &Arc<Mutex<Option<Child>>>) -> ChildProcessState {
    let mut slot = child.lock().await;
    let state = match slot.as_mut() {
        Some(process) => match process.try_wait() {
            Ok(Some(_)) => ChildProcessState::Exited,
            Ok(None) => ChildProcessState::Running,
            Err(_) => ChildProcessState::Running,
        },
        None => ChildProcessState::Untracked,
    };
    if state == ChildProcessState::Exited {
        slot.take();
    }
    state
}

#[cfg(not(windows))]
fn spawn_codex_exit_watcher(
    child: Arc<Mutex<Option<Child>>>,
    codex_exited: Arc<AtomicBool>,
) -> (
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
    tokio::task::JoinHandle<()>,
) {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let (exit_tx, exit_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let Some(mut process) = child.lock().await.take() else {
            return;
        };
        let wait_result = tokio::select! {
            _ = &mut shutdown_rx => None,
            result = process.wait() => Some(result),
        };
        let natural_exit = match wait_result {
            Some(Ok(_)) => true,
            Some(Err(error)) => {
                error_log::record_failure(
                    "process_watch_failed",
                    "wait_for_codex_exit",
                    error.to_string(),
                    serde_json::json!({
                        "processId": process.id(),
                    }),
                );
                *child.lock().await = Some(process);
                false
            }
            None => {
                *child.lock().await = Some(process);
                false
            }
        };
        if natural_exit {
            codex_exited.store(true, Ordering::Release);
            let _ = exit_tx.send(());
        }
    });
    (shutdown_tx, exit_rx, task)
}

#[cfg(windows)]
fn spawn_codex_exit_watcher(
    child: Arc<Mutex<Option<Child>>>,
    process_id: Option<u32>,
    codex_exited: Arc<AtomicBool>,
) -> (
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
    tokio::task::JoinHandle<()>,
) {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let (exit_tx, exit_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let natural_exit = if let Some(process_id) = process_id {
            tokio::select! {
                _ = &mut shutdown_rx => false,
                result = codey_runtime_core::launcher::wait_for_windows_process_id(process_id) => {
                    match result {
                        Ok(()) => true,
                        Err(error) => {
                            error_log::record_failure(
                                "process_watch_failed",
                                "wait_for_windows_codex_exit",
                                format!("{error:#}"),
                                serde_json::json!({
                                    "processId": process_id,
                                }),
                            );
                            eprintln!("等待 Windows Codex 进程退出失败：{error:#}");
                            !codey_runtime_core::windows_enumerate_processes()
                                .iter()
                                .any(|process| process.process_id == process_id)
                        }
                    }
                }
            }
        } else {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break false,
                    _ = interval.tick() => match child_process_state(&child).await {
                        ChildProcessState::Running => {}
                        ChildProcessState::Exited => break true,
                        ChildProcessState::Untracked => break false,
                    }
                }
            }
        };
        if natural_exit {
            codex_exited.store(true, Ordering::Release);
            let _ = exit_tx.send(());
        }
    });
    (shutdown_tx, exit_rx, task)
}

struct SpawnedCodex {
    child: Option<Child>,
    process_id: Option<u32>,
    #[cfg(unix)]
    process_group_id: Option<u32>,
    #[cfg(target_os = "macos")]
    inspector_argument: Option<String>,
    performance_status: String,
    performance_detail: String,
}

async fn spawn_codex(
    app_dir: &std::path::Path,
    debug_port: u16,
    disable_codex_pet: bool,
    disable_codex_voice: bool,
    fast_codex_startup: bool,
    gpu_launch_mode: GpuLaunchMode,
) -> Result<SpawnedCodex> {
    #[cfg(any(windows, target_os = "macos"))]
    let patch_options = crate::codex_startup_patch::PatchOptions {
        disable_pet: disable_codex_pet,
        disable_voice: disable_codex_voice,
        fast_codex_startup,
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = fast_codex_startup;
    let runtime_arguments = codex_runtime_arguments(gpu_launch_mode, !cfg!(target_os = "macos"));

    #[cfg(windows)]
    {
        let inspector_port =
            crate::codex_startup_patch::reserve_loopback_port().map_err(|error| {
                let error = error.context("为 Codex 启动补丁选择本地调试端口失败");
                error_log::record_failure(
                    "patch_failed",
                    "reserve_startup_patch_port",
                    format!("{error:#}"),
                    serde_json::json!({
                        "platform": "windows",
                    }),
                );
                error
            })?;
        let inspector_arg = crate::codex_startup_patch::inspector_argument(inspector_port);
        let mut launch_arguments = vec![inspector_arg];
        launch_arguments.extend(runtime_arguments.iter().cloned());
        let mut spawned = spawn_windows_codex(app_dir, debug_port, &launch_arguments).await?;
        match crate::codex_startup_patch::install(inspector_port, patch_options).await {
            Ok(()) => {
                spawned.performance_status = "ready".to_string();
                spawned.performance_detail = startup_patch_detail();
                Ok(spawned)
            }
            Err(error) => {
                let patch_error = format!("{error:#}");
                error_log::record_failure(
                    "patch_failed",
                    "install_startup_patch",
                    patch_error.clone(),
                    serde_json::json!({
                        "platform": "windows",
                        "inspectorPort": inspector_port,
                        "processId": spawned.process_id,
                        "disablePet": patch_options.disable_pet,
                        "disableVoice": patch_options.disable_voice,
                        "fastCodexStartup": patch_options.fast_codex_startup,
                    }),
                );
                if let Err(cleanup_error) = stop_windows_spawned_codex(&mut spawned, app_dir).await
                {
                    anyhow::bail!(
                        "Codex 启动补丁未能安装，且无法安全清理暂停的启动进程：{patch_error}；{cleanup_error:#}"
                    );
                }
                match spawn_windows_codex(app_dir, debug_port, &runtime_arguments).await {
                    Ok(mut fallback) => {
                        fallback.performance_status = "degraded".to_string();
                        fallback.performance_detail = if patch_options.disable_pet {
                            "宠物精简启动补丁未能确认生效，已自动以兼容模式启动；本次宠物精简失败，可能存在额外 Renderer，下次启动将自动重试"
                                .to_string()
                        } else {
                            "启动补丁未能安装，已自动以兼容模式启动；启动优化将在下次启动时重试"
                                .to_string()
                        };
                        error_log::record_failure(
                            "patch_degraded",
                            "restart_without_startup_patch",
                            patch_error,
                            serde_json::json!({
                                "platform": "windows",
                                "processId": fallback.process_id,
                                "petSlimRequested": patch_options.disable_pet,
                            }),
                        );
                        Ok(fallback)
                    }
                    Err(fallback_error) => anyhow::bail!(
                        "Codex 启动补丁未能安装，且兼容模式重启失败：{patch_error}；{fallback_error:#}"
                    ),
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let inspector_port =
            crate::codex_startup_patch::reserve_loopback_port().map_err(|error| {
                let error = error.context("为 macOS Codex 启动补丁选择本地调试端口失败");
                error_log::record_failure(
                    "patch_failed",
                    "reserve_startup_patch_port",
                    format!("{error:#}"),
                    serde_json::json!({
                        "platform": "macos",
                    }),
                );
                error
            })?;
        let inspector_arg = crate::codex_startup_patch::inspector_argument(inspector_port);
        let mut launch_arguments = vec![inspector_arg.clone()];
        launch_arguments.extend(runtime_arguments);
        let command = if app_dir.extension().and_then(|value| value.to_str()) == Some("app") {
            build_fresh_macos_open_command(app_dir, debug_port, &launch_arguments)
        } else {
            build_codex_command(app_dir, debug_port, &launch_arguments)
        };
        let mut spawned = spawn_command(command)?;
        spawned.inspector_argument = Some(inspector_arg.clone());
        match crate::codex_startup_patch::install(inspector_port, patch_options).await {
            Ok(()) => {
                spawned.performance_status = "ready".to_string();
                spawned.performance_detail = startup_patch_detail();
                Ok(spawned)
            }
            Err(error) => {
                error_log::record_failure(
                    "patch_failed",
                    "install_startup_patch",
                    format!("{error:#}"),
                    serde_json::json!({
                        "platform": "macos",
                        "inspectorPort": inspector_port,
                        "processId": spawned.process_id,
                        "processGroupId": spawned.process_group_id,
                        "disablePet": patch_options.disable_pet,
                        "disableVoice": patch_options.disable_voice,
                        "fastCodexStartup": patch_options.fast_codex_startup,
                    }),
                );
                if let Err(stop_error) = stop_macos_codex(
                    &inspector_arg,
                    app_dir,
                    spawned.process_id,
                    spawned.process_group_id,
                )
                .await
                {
                    error_log::record_failure(
                        "cleanup_failed",
                        "cleanup_macos_after_startup_patch_failure",
                        format!("{stop_error:#}"),
                        serde_json::json!({
                            "appPath": app_dir,
                            "processId": spawned.process_id,
                            "processGroupId": spawned.process_group_id,
                        }),
                    );
                    eprintln!("Codex 启动补丁失败后的进程清理失败：{stop_error:#}");
                }
                if let Some(child) = spawned.child.take() {
                    reap_child_after_cleanup(child, "reap_child_after_startup_patch_failure").await;
                }
                Err(error).context("Codex 启动硬补丁未能安装；已停止 Codex，未降级为仅隐藏 UI")
            }
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let command = build_codex_command(app_dir, debug_port, &runtime_arguments);
        let mut spawned = spawn_command(command)?;
        spawned.performance_status = "ready".to_string();
        spawned.performance_detail = if disable_codex_pet {
            "当前平台不支持宠物硬屏蔽启动补丁".to_string()
        } else if disable_codex_voice {
            "当前平台不支持语音硬屏蔽启动补丁".to_string()
        } else {
            "当前平台无需 macOS / Windows 启动补丁".to_string()
        };
        Ok(spawned)
    }
}

async fn reap_child_after_cleanup(mut child: Child, operation: &'static str) {
    let process_id = child.id();
    let needs_kill = match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(_)) => false,
        Ok(Err(error)) => {
            error_log::record_failure(
                "cleanup_failed",
                operation,
                error.to_string(),
                serde_json::json!({
                    "processId": process_id,
                    "phase": "wait",
                }),
            );
            true
        }
        Err(_) => true,
    };
    if !needs_kill {
        return;
    }
    if let Err(error) = child.kill().await {
        error_log::record_failure(
            "cleanup_failed",
            operation,
            error.to_string(),
            serde_json::json!({
                "processId": process_id,
                "phase": "kill",
            }),
        );
    }
    if let Err(error) = child.wait().await {
        error_log::record_failure(
            "cleanup_failed",
            operation,
            error.to_string(),
            serde_json::json!({
                "processId": process_id,
                "phase": "wait_after_kill",
            }),
        );
    }
}

fn gpu_launch_arguments(gpu_launch_mode: GpuLaunchMode, enabled_for_platform: bool) -> Vec<String> {
    if !enabled_for_platform {
        return Vec::new();
    }

    match gpu_launch_mode {
        GpuLaunchMode::Off => Vec::new(),
        GpuLaunchMode::DisableGpu => vec![DISABLE_GPU_ARGUMENT.to_string()],
        GpuLaunchMode::DisableGpuRasterization => {
            vec![DISABLE_GPU_RASTERIZATION_ARGUMENT.to_string()]
        }
    }
}

fn codex_runtime_arguments(
    gpu_launch_mode: GpuLaunchMode,
    gpu_arguments_enabled_for_platform: bool,
) -> Vec<String> {
    let mut arguments = vec![DEFAULT_CHINESE_LOCALE_ARGUMENT.to_string()];
    arguments.extend(gpu_launch_arguments(
        gpu_launch_mode,
        gpu_arguments_enabled_for_platform,
    ));
    arguments
}

async fn prepare_codex_for_launch(app_dir: &std::path::Path) -> Result<()> {
    // Startup patches must be applied before the Codex main process starts.
    // If the configured app is already running, stop its process tree and
    // relaunch it under Codey instead of leaving the user to quit it manually.
    #[cfg(windows)]
    {
        let executable = codey_runtime_core::app_paths::build_codex_executable(app_dir);
        let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
        let executable = normalized_windows_path(&executable);
        let already_running = codey_runtime_core::windows_enumerate_processes()
            .into_iter()
            .filter_map(|process| process.executable_path)
            .map(|path| std::fs::canonicalize(&path).unwrap_or(path))
            .any(|path| normalized_windows_path(&path) == executable);
        if already_running {
            terminate_windows_codex_processes(app_dir, None)
                .await
                .context("停止正在运行的 Codex 失败")?;
        }
    }
    #[cfg(not(windows))]
    let _ = app_dir;
    #[cfg(target_os = "macos")]
    if macos_codex_is_running(app_dir).await? {
        terminate_unix_codex_processes(app_dir, None, None, None)
            .await
            .context("停止正在运行的 Codex 失败")?;
    }
    Ok(())
}

#[cfg(any(windows, target_os = "macos"))]
fn startup_patch_detail() -> String {
    #[cfg(windows)]
    {
        "Windows 启动补丁已启用：WMI 周期采样、临时 WebView 残留和执行环境泄漏已修复".to_string()
    }
    #[cfg(not(windows))]
    {
        "启动补丁已启用：临时 WebView 和执行环境会自动回收".to_string()
    }
}

#[cfg(not(windows))]
fn spawn_command(command: Vec<String>) -> Result<SpawnedCodex> {
    let executable = command
        .first()
        .ok_or_else(|| anyhow::anyhow!("Codex 启动命令为空"))?;
    let mut child_command = Command::new(executable);
    child_command.args(&command[1..]);
    #[cfg(unix)]
    child_command.process_group(0);
    let child = child_command
        .spawn()
        .with_context(|| format!("启动 Codex 失败：{executable}"))?;
    let process_id = child.id();
    Ok(SpawnedCodex {
        child: Some(child),
        process_id,
        #[cfg(unix)]
        process_group_id: process_id,
        #[cfg(target_os = "macos")]
        inspector_argument: None,
        performance_status: String::new(),
        performance_detail: String::new(),
    })
}

#[cfg(test)]
mod gpu_launch_argument_tests {
    use super::*;

    #[test]
    fn gpu_launch_arguments_are_mutually_exclusive_and_platform_gated() {
        assert!(gpu_launch_arguments(GpuLaunchMode::Off, true).is_empty());
        assert_eq!(
            gpu_launch_arguments(GpuLaunchMode::DisableGpu, true),
            vec![DISABLE_GPU_ARGUMENT.to_string()]
        );
        assert_eq!(
            gpu_launch_arguments(GpuLaunchMode::DisableGpuRasterization, true),
            vec![DISABLE_GPU_RASTERIZATION_ARGUMENT.to_string()]
        );
        assert!(gpu_launch_arguments(GpuLaunchMode::DisableGpu, false).is_empty());
        assert!(gpu_launch_arguments(GpuLaunchMode::DisableGpuRasterization, false).is_empty());
    }

    #[test]
    fn runtime_arguments_set_chinese_before_the_renderer_starts() {
        assert_eq!(
            codex_runtime_arguments(GpuLaunchMode::Off, true),
            vec![DEFAULT_CHINESE_LOCALE_ARGUMENT.to_string()]
        );
        assert_eq!(
            codex_runtime_arguments(GpuLaunchMode::DisableGpu, true),
            vec![
                DEFAULT_CHINESE_LOCALE_ARGUMENT.to_string(),
                DISABLE_GPU_ARGUMENT.to_string(),
            ]
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn chat_provider_builds_an_explicit_protocol_proxy_snapshot() {
        let mut config = CodeyConfig::default();
        let mut profile = crate::config::ProviderProfile::new("DeepSeek");
        profile.base_url = "https://relay.example/v1".to_string();
        profile.api_key = "sk-test".to_string();
        profile.protocol = RelayProtocol::ChatCompletions;
        config.active_profile_id = profile.id.clone();
        config.profiles = vec![profile.clone()];
        config
            .default_model_by_provider
            .insert(profile.id.clone(), "deepseek-reasoner".to_string());

        let settings = protocol_proxy_settings(&profile, config.default_model()).unwrap();
        let relay = settings.active_relay_profile();
        assert_eq!(settings.active_relay_id, profile.id);
        assert_eq!(relay.base_url, "https://relay.example/v1");
        assert_eq!(relay.api_key, "sk-test");
        assert_eq!(relay.model, "deepseek-reasoner");
        assert_eq!(relay.protocol, RelayProtocol::ChatCompletions);
        assert_eq!(relay.relay_mode, RelayMode::PureApi);
    }

    #[test]
    fn responses_provider_does_not_start_a_protocol_proxy() {
        let mut config = CodeyConfig::default();
        let mut profile = crate::config::ProviderProfile::new("Responses");
        profile.base_url = "https://relay.example/v1".to_string();
        config.active_profile_id = profile.id.clone();
        config.profiles = vec![profile.clone()];

        assert!(protocol_proxy_settings(&profile, config.default_model()).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_launch_forces_a_new_app_instance() {
        let command = build_fresh_macos_open_command(
            std::path::Path::new("/Applications/ChatGPT.app"),
            9229,
            &["--inspect-brk=127.0.0.1:19321".to_string()],
        );
        assert_eq!(command.first().map(String::as_str), Some("open"));
        assert!(command.iter().any(|part| part == "-n"));
        assert!(command.iter().any(|part| part == "-W"));
        assert!(
            command
                .iter()
                .any(|part| part == "--remote-debugging-port=9229")
        );
        assert!(
            command
                .iter()
                .any(|part| part == "--inspect-brk=127.0.0.1:19321")
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_running_check_does_not_match_an_unrelated_app_path() {
        let running = macos_codex_is_running(std::path::Path::new(
            "/Applications/Definitely Not Codex.app",
        ))
        .await
        .unwrap();
        assert!(!running);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_running_check_matches_only_the_app_main_executable() {
        let processes = crate::process_tree::parse_unix_process_snapshot(
            b"100 1 100 Thu Jul 23 19:23:12 2026 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --remote-debugging-port=9229\n\
              101 100 100 Thu Jul 23 19:23:13 2026 /Applications/ChatGPT.app/Contents/Resources/codex app-server\n\
              102 101 102 Thu Jul 23 19:23:14 2026 /Applications/ChatGPT.app/Contents/Frameworks/Chromium Helper\n",
        );
        assert!(macos_main_executable_is_running(
            &processes,
            Path::new("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"),
        ));
        assert!(!macos_main_executable_is_running(
            &processes[1..],
            Path::new("/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"),
        ));
    }

    #[test]
    fn owned_codex_tree_includes_bundle_helpers_and_external_descendants() {
        let processes = crate::process_tree::parse_unix_process_snapshot(
            b"100 1 100 Thu Jul 23 19:23:12 2026 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --inspect\n\
              101 100 100 Thu Jul 23 19:23:13 2026 /Applications/ChatGPT.app/Contents/Resources/codex app-server\n\
              102 101 102 Thu Jul 23 19:23:14 2026 node ./mcp/server.mjs\n\
              103 1 103 Thu Jul 23 19:23:15 2026 /Applications/ChatGPT.app/Contents/Frameworks/browser_crashpad_handler\n\
              200 1 200 Thu Jul 23 19:23:16 2026 unrelated\n",
        );
        assert_eq!(
            owned_unix_codex_process_ids(
                &processes,
                Path::new("/Applications/ChatGPT.app"),
                None,
                None,
                Some("--inspect"),
            ),
            HashSet::from([100, 101, 102, 103])
        );
    }

    #[tokio::test]
    async fn unix_shutdown_terminates_the_spawned_process_group() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        command.process_group(0);
        let mut child = command.spawn().expect("spawn process tree");
        let process_id = child.id().expect("child process id");

        terminate_unix_codex_processes(
            Path::new("/definitely-not-a-real-codex-app"),
            Some(process_id),
            Some(process_id),
            None,
        )
        .await
        .expect("terminate process tree");

        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .expect("root process was left running")
            .expect("wait for root process");
    }

    #[tokio::test]
    async fn exit_watcher_reports_a_naturally_exited_child() {
        let child = Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn short-lived child");
        let child = Arc::new(Mutex::new(Some(child)));
        let exited = Arc::new(AtomicBool::new(false));
        let (_shutdown, exit_rx, task) = spawn_codex_exit_watcher(child, exited.clone());

        tokio::time::timeout(Duration::from_secs(2), exit_rx)
            .await
            .expect("watcher timed out")
            .expect("watcher was cancelled");
        task.await.expect("watcher task failed");
        assert!(exited.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn exit_watcher_returns_the_child_to_stop_on_shutdown() {
        let child = Command::new("sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .expect("spawn long-lived child");
        let child = Arc::new(Mutex::new(Some(child)));
        let exited = Arc::new(AtomicBool::new(false));
        let (shutdown, _exit_rx, task) = spawn_codex_exit_watcher(child.clone(), exited.clone());

        shutdown.send(()).expect("send watcher shutdown");
        task.await.expect("watcher task failed");

        assert!(!exited.load(Ordering::Acquire));
        let mut process = child
            .lock()
            .await
            .take()
            .expect("watcher should return the child");
        process.kill().await.expect("kill child");
        process.wait().await.expect("reap child");
    }

    #[test]
    fn cdp_watchdog_requires_consecutive_failures_before_reinjecting() {
        let mut failures = 0;

        assert!(!watchdog_should_reinject(&mut failures, false));
        assert_eq!(failures, 1);
        assert!(!watchdog_should_reinject(&mut failures, true));
        assert_eq!(failures, 0);
        assert!(!watchdog_should_reinject(&mut failures, false));
        assert!(watchdog_should_reinject(&mut failures, false));
    }
}
