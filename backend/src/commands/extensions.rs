use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};

use super::{AppState, argument};

/// 仅在这里取得宿主路径和生命周期状态；扩展模块不依赖 AppState。
pub(super) async fn invoke(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let request = argument::<Value>(args, "request")?;
    let reload_mcp = matches!(
        request["action"].as_str(),
        Some("save_mcp" | "set_mcp_enabled" | "set_mcps_enabled" | "remove_mcp")
    );
    if state.is_shutting_down() || state.restart_in_progress.load(Ordering::Acquire) {
        return Err("Codey 正在退出或重启，请稍后重新操作".into());
    }
    let user_home = directories::BaseDirs::new()
        .ok_or("无法确定当前用户目录")?
        .home_dir()
        .to_path_buf();
    let data_dir = state
        .store
        .path()
        .parent()
        .ok_or("Codey 配置目录无效")?
        .join("codex-extensions");
    let mut result = crate::codex_extensions::dispatch(
        crate::codex_config::codex_home().to_path_buf(),
        data_dir,
        user_home,
        request,
    )
    .await
    .map_err(|error| error.to_string())?;
    if reload_mcp
        && (result["applyStatus"] == "reload-required" || result["applyStatus"] == "unchanged")
    {
        // 配置事务已完成；刷新失败也必须返回已保存的清单，避免重复导入。
        let runtime = state.runtime.lock().await.clone();
        if let Some(runtime) = runtime {
            let websocket_url = runtime.renderer_websocket_url().await;
            let reloaded = tokio::time::timeout(
                std::time::Duration::from_secs(12),
                crate::cdp::reload_mcp_servers(&websocket_url),
            )
            .await;
            if matches!(reloaded, Ok(Ok(()))) {
                result["applyStatus"] = json!("applied");
                result["message"] = json!("MCP 配置已保存并通知 Codex 重新加载，无需重启。");
            } else {
                result["applyStatus"] = json!("reload-failed");
                result["message"] = json!(
                    "MCP 配置已保存，但未能确认 Codex 自动刷新。请确认 Codex 已连接后重新保存；旧版 Codex 可能需要重启。"
                );
            }
        } else {
            result["applyStatus"] = json!("pending-runtime");
            result["message"] = json!("MCP 配置已保存；Codex 当前未运行，下次启动时自动加载。");
        }
    }
    Ok(result)
}
