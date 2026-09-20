use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use super::{argument, optional_argument, string_argument};
use crate::codey_plugins;

pub(super) async fn invoke(command: &str, args: &Value) -> Result<Value, String> {
    match command {
        "list_codey_plugins" => blocking(codey_plugins::list).await,
        "get_codey_plugin_config_file" => {
            let id = string_argument(args, "pluginId")?;
            blocking(move || codey_plugins::get_config_file(&id)).await
        }
        "select_codey_plugin_package" => select_package().await,
        "inspect_codey_plugin" => {
            let path = PathBuf::from(string_argument(args, "path")?);
            blocking(move || codey_plugins::inspect(&path)).await
        }
        "install_codey_plugin" => {
            let path = PathBuf::from(string_argument(args, "path")?);
            let sha256 = string_argument(args, "sha256")?;
            blocking(move || codey_plugins::install(&path, &sha256)).await
        }
        "set_codey_plugin_enabled" => {
            let id = string_argument(args, "pluginId")?;
            let enabled = argument::<bool>(args, "enabled")?;
            blocking(move || codey_plugins::set_enabled(&id, enabled)).await
        }
        "save_codey_plugin_config_file" => {
            let id = string_argument(args, "pluginId")?;
            let content = string_argument(args, "content")?;
            let expected_sha256 = string_argument(args, "expectedSha256")?;
            blocking(move || codey_plugins::save_config_file(&id, &content, &expected_sha256)).await
        }
        "uninstall_codey_plugin" => {
            let id = string_argument(args, "pluginId")?;
            let remove_data = optional_argument::<bool>(args, "removeData")?.unwrap_or(false);
            blocking(move || codey_plugins::uninstall(&id, remove_data)).await
        }
        "invoke_codey_plugin" => {
            let id = string_argument(args, "pluginId")?;
            let method = string_argument(args, "method")?;
            let params = args.get("params").cloned().unwrap_or(Value::Null);
            blocking(move || codey_plugins::invoke(&id, &method, params)).await
        }
        _ => Err("未知插件管理命令".into()),
    }
}

async fn blocking<T: Serialize + Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<Value, String> {
    tokio::task::spawn_blocking(move || {
        serde_json::to_value(work()?).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("插件管理任务异常退出：{error}"))?
}

#[cfg(any(target_os = "macos", windows))]
async fn select_package() -> Result<Value, String> {
    blocking(|| {
        rfd::FileDialog::new()
            .set_title("导入 Codey 插件")
            .add_filter("Codey 插件", &["codey-plugin"])
            .pick_file()
            .map(|path| codey_plugins::inspect(&path))
            .transpose()
    })
    .await
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn select_package() -> Result<Value, String> {
    Err("当前平台请使用本地文件路径导入插件".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn invalid_plugin_arguments_are_rejected_before_execution() {
        for (command, args) in [
            (
                "set_codey_plugin_enabled",
                json!({"pluginId":"demo","enabled":"true"}),
            ),
            (
                "install_codey_plugin",
                json!({"path":"/tmp/demo.codey-plugin"}),
            ),
            ("save_codey_plugin_config_file", json!({"pluginId":"demo"})),
            ("get_codey_plugin_config_file", json!({"pluginId":42})),
            (
                "uninstall_codey_plugin",
                json!({"pluginId":"demo","removeData":1}),
            ),
        ] {
            assert!(invoke(command, &args).await.is_err(), "{command}");
        }
    }
}
