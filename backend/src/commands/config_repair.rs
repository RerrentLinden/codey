use super::{AppState, Arc, Ordering};
use crate::codex_config::{self, ConfigRepairFailure};
use crate::error_log;
use serde_json::{Value, json};

fn record_failure(failure: ConfigRepairFailure) -> String {
    record_failure_with(failure, |message, context| {
        error_log::record_failure(
            "codex_config_repair_failed",
            "repair_codex_config",
            message,
            context,
        );
    })
}

fn record_failure_with(failure: ConfigRepairFailure, log: impl FnOnce(&str, Value)) -> String {
    let message = failure.description();
    log(
        &message,
        json!({"stage": failure.stage, "targetPath": failure.path}),
    );
    message
}

pub(super) async fn repair_codex_config(state: &Arc<AppState>) -> Result<Value, String> {
    let path = codex_config::codex_home().join("config.toml");
    // Keep lock ownership inside the spawned task: closing the requesting page
    // must not release serialization while blocking filesystem work continues.
    let operation = Arc::clone(state);
    tokio::spawn(async move {
        let _guard = operation.runtime_operation.try_lock().map_err(|_| {
            record_failure(ConfigRepairFailure::new(
                "检查运行状态",
                &path,
                "Codex 正在启动、停止或修复，请稍后重试",
            ))
        })?;
        if operation.restart_in_progress.load(Ordering::Acquire)
            || operation.shutting_down.load(Ordering::Acquire)
        {
            return Err(record_failure(ConfigRepairFailure::new(
                "检查运行状态",
                &path,
                "Codex 正在重启或退出，请稍后重试",
            )));
        }
        let active = operation.runtime.lock().await.is_some();
        let report = tokio::task::spawn_blocking(move || codex_config::repair_codex_config(active))
            .await
            .map_err(|_| {
                record_failure(ConfigRepairFailure::new(
                    "后台修复任务",
                    &path,
                    "配置修复任务异常退出，请检查错误日志并重试",
                ))
            })?
            .map_err(record_failure)?;
        serde_json::to_value(report).map_err(|_| {
            record_failure(ConfigRepairFailure::new(
                "返回修复结果",
                &path,
                "无法生成配置修复结果",
            ))
        })
    })
    .await
    .map_err(|_| {
        record_failure(ConfigRepairFailure::new(
            "后台调度",
            &codex_config::codex_home().join("config.toml"),
            "配置修复调度任务异常退出，请重试",
        ))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_repair_failure_is_forwarded_with_stage_and_path() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.toml");
        let log_file = temp.path().join("errors.log");
        let failure = ConfigRepairFailure::new("解析配置", &target, "TOML 语法无效");
        let returned = record_failure_with(failure, |message, context| {
            std::fs::write(
                &log_file,
                serde_json::to_vec(&json!({"error":message, "context":context})).unwrap(),
            )
            .unwrap();
        });
        let record: Value = serde_json::from_slice(&std::fs::read(log_file).unwrap()).unwrap();
        assert_eq!(record["context"]["stage"], "解析配置");
        assert_eq!(
            record["context"]["targetPath"],
            target.to_string_lossy().as_ref()
        );
        assert_eq!(record["error"], returned);
    }
}
