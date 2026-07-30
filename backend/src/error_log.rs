use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use anyhow::Context;
use chrono::{DateTime, Local, NaiveDate, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ERROR_LOG_FILE: &str = "codey-errors.log";
const ERROR_LOG_HELPER_ARGUMENT: &str = "--codey-record-error";
const MAX_HELPER_INPUT_BYTES: u64 = 1024 * 1024;

static ERROR_LOG_WRITER: OnceLock<Mutex<ErrorLogWriter>> = OnceLock::new();

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorRecord {
    timestamp: String,
    timestamp_ms: i64,
    pid: u32,
    platform: String,
    event: String,
    operation: String,
    error: String,
    context: Value,
}

#[derive(Default)]
struct ErrorLogWriter;

impl ErrorLogWriter {
    fn clear_if_stale(&mut self, path: &Path, today: NaiveDate) -> std::io::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        with_log_file_lock(path, || {
            if !path.exists() || !file_is_from_different_day(path, today)? {
                return Ok(());
            }
            OpenOptions::new().write(true).truncate(true).open(path)?;
            Ok(())
        })
    }

    fn append(&mut self, path: &Path, today: NaiveDate, line: &str) -> std::io::Result<()> {
        with_log_file_lock(path, || {
            let truncate = file_is_from_different_day(path, today)?;
            if !truncate {
                repair_incomplete_tail(path)?;
            }
            let mut options = OpenOptions::new();
            options.create(true);
            #[cfg(unix)]
            options.mode(0o600);
            if truncate {
                options.write(true).truncate(true);
            } else {
                options.append(true);
            }
            let mut file = options.open(path)?;
            let mut complete_line = String::with_capacity(line.len().saturating_add(1));
            complete_line.push_str(line);
            complete_line.push('\n');
            file.write_all(complete_line.as_bytes())?;
            file.flush()
        })
    }
}

fn with_log_file_lock<T>(
    log_path: &Path,
    action: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = log_path.with_extension("lock");
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let lock_file = options.open(lock_path)?;
    fs2::FileExt::lock_exclusive(&lock_file)?;
    let result = action();
    let unlock_result = fs2::FileExt::unlock(&lock_file);
    match (result, unlock_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn repair_incomplete_tail(path: &Path) -> std::io::Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if bytes.is_empty() || bytes.last() == Some(&b'\n') {
        return Ok(());
    }

    let line_start = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index.saturating_add(1));
    let mut file = OpenOptions::new().append(true).open(path)?;
    if serde_json::from_slice::<Value>(&bytes[line_start..]).is_ok() {
        file.write_all(b"\n")?;
    } else {
        file.set_len(u64::try_from(line_start).unwrap_or(0))?;
    }
    file.flush()
}

pub fn initialize() {
    let path = error_log_path();
    let today = Local::now().date_naive();
    let writer = ERROR_LOG_WRITER.get_or_init(|| Mutex::new(ErrorLogWriter));
    let result = writer
        .lock()
        .map_err(|_| std::io::Error::other("Codey error log lock is poisoned"))
        .and_then(|mut writer| writer.clear_if_stale(&path, today));
    if let Err(error) = result {
        eprintln!("清理过期 Codey 错误日志失败：{error}");
    }
}

pub fn record_failure(
    event: impl Into<String>,
    operation: impl Into<String>,
    error: impl Into<String>,
    context: impl Serialize,
) {
    let now = Local::now();
    let context = serde_json::to_value(context).unwrap_or_else(|serialization_error| {
        serde_json::json!({
            "contextSerializationError": serialization_error.to_string(),
        })
    });
    let record = ErrorRecord {
        timestamp: now.to_rfc3339_opts(SecondsFormat::Millis, false),
        timestamp_ms: now.timestamp_millis(),
        pid: std::process::id(),
        platform: std::env::consts::OS.to_string(),
        event: event.into(),
        operation: operation.into(),
        error: error.into(),
        context,
    };
    if let Err(error) = append_record(&record, now.date_naive()) {
        eprintln!("写入 Codey 错误日志失败：{error}");
    }
}

pub fn run_helper_if_requested() -> anyhow::Result<bool> {
    if std::env::args_os()
        .nth(1)
        .is_none_or(|argument| argument != ERROR_LOG_HELPER_ARGUMENT)
    {
        return Ok(false);
    }

    let mut input = String::new();
    std::io::stdin()
        .take(MAX_HELPER_INPUT_BYTES.saturating_add(1))
        .read_to_string(&mut input)
        .context("读取 Codey 错误日志 helper 输入失败")?;
    anyhow::ensure!(
        u64::try_from(input.len()).unwrap_or(u64::MAX) <= MAX_HELPER_INPUT_BYTES,
        "Codey 错误日志 helper 输入过大"
    );
    let record: ErrorRecord =
        serde_json::from_str(&input).context("解析 Codey 错误日志 helper 输入失败")?;
    anyhow::ensure!(
        !record.event.trim().is_empty()
            && !record.operation.trim().is_empty()
            && !record.error.trim().is_empty(),
        "Codey 错误日志 helper 缺少失败信息"
    );
    append_record(&record, Local::now().date_naive()).context("Codey 错误日志 helper 写入失败")?;
    Ok(true)
}

fn append_record(record: &ErrorRecord, today: NaiveDate) -> anyhow::Result<()> {
    let line = serde_json::to_string(record).context("序列化 Codey 错误日志失败")?;
    let path = error_log_path();
    let writer = ERROR_LOG_WRITER.get_or_init(|| Mutex::new(ErrorLogWriter));
    writer
        .lock()
        .map_err(|_| anyhow::anyhow!("Codey error log lock is poisoned"))?
        .append(&path, today, &line)
        .map_err(anyhow::Error::from)
}

pub fn error_log_path() -> PathBuf {
    codey_runtime_core::paths::default_app_state_dir().join(ERROR_LOG_FILE)
}

fn file_is_from_different_day(path: &Path, today: NaiveDate) -> std::io::Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let modified = metadata.modified()?;
    Ok(DateTime::<Local>::from(modified).date_naive() != today)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_uses_codey_state_directory() {
        assert!(
            error_log_path().ends_with(".codex-session-delete/codey-errors.log"),
            "{}",
            error_log_path().display()
        );
    }

    #[test]
    fn record_contains_diagnostic_fields() {
        let now = Local::now();
        let record = ErrorRecord {
            timestamp: now.to_rfc3339_opts(SecondsFormat::Millis, false),
            timestamp_ms: now.timestamp_millis(),
            pid: 42,
            platform: "windows".to_string(),
            event: "injection_failed".to_string(),
            operation: "inject_cdp_bridge".to_string(),
            error: "renderer unavailable".to_string(),
            context: serde_json::json!({"attempts": 30}),
        };

        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["event"], "injection_failed");
        assert_eq!(value["operation"], "inject_cdp_bridge");
        assert_eq!(value["error"], "renderer unavailable");
        assert_eq!(value["platform"], "windows");
        assert_eq!(value["context"]["attempts"], 30);
        assert!(value["timestampMs"].is_i64());
    }

    #[test]
    fn same_day_failures_are_appended() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(ERROR_LOG_FILE);
        let today = Local::now().date_naive();
        let mut writer = ErrorLogWriter;

        writer.append(&path, today, r#"{"error":"first"}"#).unwrap();
        writer
            .append(&path, today, r#"{"error":"second"}"#)
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{\"error\":\"first\"}\n{\"error\":\"second\"}\n"
        );
    }

    #[test]
    fn crossing_into_a_new_day_clears_old_failures() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(ERROR_LOG_FILE);
        let first_day = Local::now().date_naive();
        let next_day = first_day.succ_opt().unwrap();
        let mut writer = ErrorLogWriter;

        writer
            .append(&path, first_day, r#"{"error":"old"}"#)
            .unwrap();
        writer
            .append(&path, next_day, r#"{"error":"new"}"#)
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{\"error\":\"new\"}\n"
        );
    }

    #[test]
    fn incomplete_tail_is_repaired_before_the_next_failure() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(ERROR_LOG_FILE);
        std::fs::write(&path, b"{\"error\":\"complete\"}\n{\"error\":").unwrap();
        let today = Local::now().date_naive();
        let mut writer = ErrorLogWriter;

        writer.append(&path, today, r#"{"error":"next"}"#).unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{\"error\":\"complete\"}\n{\"error\":\"next\"}\n"
        );
    }

    #[test]
    fn concurrent_writers_keep_each_json_line_intact() {
        let temp = tempfile::tempdir().unwrap();
        let path = std::sync::Arc::new(temp.path().join(ERROR_LOG_FILE));
        let today = Local::now().date_naive();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads = (0..8)
            .map(|thread_id| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut writer = ErrorLogWriter;
                    barrier.wait();
                    for entry_id in 0..20 {
                        writer
                            .append(
                                &path,
                                today,
                                &serde_json::json!({
                                    "thread": thread_id,
                                    "entry": entry_id,
                                })
                                .to_string(),
                            )
                            .unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }

        let contents = std::fs::read_to_string(path.as_ref()).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 160);
        for line in lines {
            serde_json::from_str::<Value>(line).unwrap();
        }
    }
}
