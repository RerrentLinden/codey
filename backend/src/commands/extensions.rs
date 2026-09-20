use serde_json::Value;
use std::sync::{Arc, atomic::Ordering};

use super::{AppState, argument};

/// 仅在这里取得宿主路径和生命周期状态；扩展模块不依赖 AppState。
pub(super) async fn invoke(state: &Arc<AppState>, args: &Value) -> Result<Value, String> {
    let request = argument::<Value>(args, "request")?;
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
    crate::codex_extensions::dispatch(
        crate::codex_config::codex_home().to_path_buf(),
        data_dir,
        user_home,
        request,
    )
    .await
    .map_err(|error| error.to_string())
}
