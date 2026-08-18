use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use anyhow::Context;
use chrono::{DateTime, FixedOffset, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ERROR_LOG_FILE: &str = "codey-errors.log";
const ERROR_LOG_HELPER_ARGUMENT: &str = "--codey-record-error";
const MAX_HELPER_INPUT_BYTES: u64 = 1024 * 1024;
const MAX_HELPER_RECORDS: usize = 64;
const BEIJING_OFFSET_SECONDS: i32 = 8 * 60 * 60;
static ERROR_LOG_WRITER: OnceLock<Mutex<ErrorLogWriter>> = OnceLock::new();
static PANIC_LOG_HOOK: OnceLock<()> = OnceLock::new();

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorVersions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codey: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    codex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    electron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chrome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node: Option<String>,
}

impl ErrorVersions {
    fn current() -> Self {
        Self {
            codey: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..Self::default()
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorRecord {
    timestamp: String,
    platform: String,
    #[serde(default)]
    versions: ErrorVersions,
    event: String,
    operation: String,
    error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recoverable: Option<bool>,
    #[serde(default, skip_serializing_if = "is_empty_context")]
    context: Value,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ErrorHelperInput {
    Record(ErrorRecord),
    Batch(Vec<ErrorRecord>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailureMetadata {
    pub stage: Option<String>,
    pub recoverable: Option<bool>,
}

fn beijing_offset() -> FixedOffset {
    FixedOffset::east_opt(BEIJING_OFFSET_SECONDS).expect("valid Beijing UTC offset")
}

fn beijing_now() -> DateTime<FixedOffset> {
    Utc::now().with_timezone(&beijing_offset())
}

fn format_beijing_timestamp(timestamp: DateTime<FixedOffset>) -> String {
    timestamp.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn normalize_beijing_timestamp(timestamp: &str) -> String {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|timestamp| format_beijing_timestamp(timestamp.with_timezone(&beijing_offset())))
        .unwrap_or_else(|_| format_beijing_timestamp(beijing_now()))
}

fn is_empty_context(context: &Value) -> bool {
    match context {
        Value::Null => true,
        Value::Object(values) => values.is_empty(),
        Value::Array(values) => values.is_empty(),
        _ => false,
    }
}

fn take_context_version(context: &mut Value, key: &str) -> Option<String> {
    let values = context.as_object_mut()?;
    let version = values
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|version| !version.is_empty())?
        .to_string();
    values.remove(key);
    Some(version)
}

fn normalize_record(record: &mut ErrorRecord) {
    record.timestamp = normalize_beijing_timestamp(&record.timestamp);
    if record.platform.trim().is_empty() {
        record.platform = std::env::consts::OS.to_string();
    }
    record.versions.codey = Some(env!("CARGO_PKG_VERSION").to_string());
    record.versions.codex = record
        .versions
        .codex
        .take()
        .or_else(|| take_context_version(&mut record.context, "codexVersion"));
    record.versions.electron = record
        .versions
        .electron
        .take()
        .or_else(|| take_context_version(&mut record.context, "electronVersion"));
    record.versions.chrome = record
        .versions
        .chrome
        .take()
        .or_else(|| take_context_version(&mut record.context, "chromeVersion"));
    record.versions.node = record
        .versions
        .node
        .take()
        .or_else(|| take_context_version(&mut record.context, "nodeVersion"));
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
    use std::io::{Read, Seek, SeekFrom};

    // 只探测文件尾部：修复目标只有最后一行，整文件读取会让持续失败的记录
    // 路径退化为 O(日志大小)。窗口内没有换行时按倍扩大，语义与全量读一致。
    const TAIL_PROBE_BYTES: u64 = 64 * 1024;
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(());
    }
    let mut probe = TAIL_PROBE_BYTES;
    let (line_start, tail) = loop {
        let start = len.saturating_sub(probe);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::with_capacity(usize::try_from(len - start).unwrap_or_default());
        file.read_to_end(&mut bytes)?;
        if bytes.last() == Some(&b'\n') {
            return Ok(());
        }
        if let Some(index) = bytes.iter().rposition(|byte| *byte == b'\n') {
            let line_start = start.saturating_add(index as u64).saturating_add(1);
            break (line_start, bytes.split_off(index.saturating_add(1)));
        }
        if start == 0 {
            break (0, bytes);
        }
        probe = probe.saturating_mul(2);
    };
    if serde_json::from_slice::<Value>(&tail).is_ok() {
        file.seek(SeekFrom::End(0))?;
        file.write_all(b"\n")?;
    } else {
        file.set_len(line_start)?;
    }
    file.flush()
}

pub fn initialize() {
    let path = error_log_path();
    let today = beijing_now().date_naive();
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
    record_failure_with_metadata(event, operation, error, FailureMetadata::default(), context);
}

pub fn record_failure_with_metadata(
    event: impl Into<String>,
    operation: impl Into<String>,
    error: impl Into<String>,
    metadata: FailureMetadata,
    context: impl Serialize,
) {
    let now = beijing_now();
    let context = serde_json::to_value(context).unwrap_or_else(|serialization_error| {
        serde_json::json!({
            "contextSerializationError": serialization_error.to_string(),
        })
    });
    let record = ErrorRecord {
        timestamp: format_beijing_timestamp(now),
        platform: std::env::consts::OS.to_string(),
        versions: ErrorVersions::current(),
        event: event.into(),
        operation: operation.into(),
        error: error.into(),
        stage: metadata.stage,
        recoverable: metadata.recoverable,
        context,
    };
    if let Err(error) = append_record(&record, now.date_naive()) {
        eprintln!("写入 Codey 错误日志失败：{error}");
    }
}

pub fn record_process_failure(
    event: impl Into<String>,
    operation: impl Into<String>,
    error: impl Into<String>,
    stage: impl Into<String>,
) {
    record_process_failure_with_recoverability(event, operation, error, stage, false);
}

pub fn record_process_failure_with_recoverability(
    event: impl Into<String>,
    operation: impl Into<String>,
    error: impl Into<String>,
    stage: impl Into<String>,
    recoverable: bool,
) {
    record_failure_with_metadata(
        event,
        operation,
        error,
        FailureMetadata {
            stage: Some(stage.into()),
            recoverable: Some(recoverable),
        },
        serde_json::json!({}),
    );
}

pub fn install_panic_hook(component: &'static str, stage: &'static str) {
    PANIC_LOG_HOOK.get_or_init(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let message = panic_info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| {
                    panic_info
                        .payload()
                        .downcast_ref::<String>()
                        .map(String::as_str)
                })
                .unwrap_or("unknown panic");
            let location = panic_info
                .location()
                .map(|location| {
                    format!(
                        "{}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    )
                })
                .unwrap_or_else(|| "unknown".to_string());
            record_failure_with_metadata(
                "process_panicked",
                "uncaught_panic",
                message,
                FailureMetadata {
                    stage: Some(stage.to_string()),
                    recoverable: Some(false),
                },
                serde_json::json!({
                    "component": component,
                    "location": location,
                }),
            );
            previous_hook(panic_info);
        }));
    });
}

pub async fn record_failure_async<C>(
    event: impl Into<String>,
    operation: impl Into<String>,
    error: impl Into<String>,
    context: C,
) where
    C: Serialize + Send + 'static,
{
    record_failure_with_metadata_async(
        event,
        operation,
        error,
        FailureMetadata::default(),
        context,
    )
    .await;
}

pub async fn record_failure_with_metadata_async<C>(
    event: impl Into<String>,
    operation: impl Into<String>,
    error: impl Into<String>,
    metadata: FailureMetadata,
    context: C,
) where
    C: Serialize + Send + 'static,
{
    let event = event.into();
    let operation = operation.into();
    let error = error.into();
    if let Err(join_error) = tokio::task::spawn_blocking(move || {
        record_failure_with_metadata(event, operation, error, metadata, context);
    })
    .await
    {
        eprintln!("Codey 错误日志写入任务异常退出：{join_error}");
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
    let records = parse_helper_records(&input)?;
    append_records(&records, beijing_now().date_naive())
        .context("Codey 错误日志 helper 写入失败")?;
    Ok(true)
}

fn parse_helper_records(input: &str) -> anyhow::Result<Vec<ErrorRecord>> {
    let input: ErrorHelperInput =
        serde_json::from_str(input).context("解析 Codey 错误日志 helper 输入失败")?;
    let mut records = match input {
        ErrorHelperInput::Record(record) => vec![record],
        ErrorHelperInput::Batch(records) => records,
    };
    anyhow::ensure!(!records.is_empty(), "Codey 错误日志 helper 批次为空");
    anyhow::ensure!(
        records.len() <= MAX_HELPER_RECORDS,
        "Codey 错误日志 helper 批次过大"
    );
    for (index, record) in records.iter_mut().enumerate() {
        anyhow::ensure!(
            !record.event.trim().is_empty()
                && !record.operation.trim().is_empty()
                && !record.error.trim().is_empty(),
            "Codey 错误日志 helper 第 {} 条记录缺少失败信息",
            index + 1
        );
        normalize_record(record);
    }
    Ok(records)
}

fn append_record(record: &ErrorRecord, today: NaiveDate) -> anyhow::Result<()> {
    append_records(std::slice::from_ref(record), today)
}

fn append_records(records: &[ErrorRecord], today: NaiveDate) -> anyhow::Result<()> {
    anyhow::ensure!(!records.is_empty(), "Codey 错误日志批次为空");
    let lines = records
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .context("序列化 Codey 错误日志失败")?
        .join("\n");
    let path = error_log_path();
    let writer = ERROR_LOG_WRITER.get_or_init(|| Mutex::new(ErrorLogWriter));
    writer
        .lock()
        .map_err(|_| anyhow::anyhow!("Codey error log lock is poisoned"))?
        .append(&path, today, &lines)
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
    Ok(DateTime::<Utc>::from(modified)
        .with_timezone(&beijing_offset())
        .date_naive()
        != today)
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
    fn record_contains_minimal_diagnostic_fields() {
        let now = beijing_now();
        let record = ErrorRecord {
            timestamp: format_beijing_timestamp(now),
            platform: "windows".to_string(),
            versions: ErrorVersions {
                codey: Some("0.7.3".to_string()),
                electron: Some("151.0.0".to_string()),
                ..ErrorVersions::default()
            },
            event: "injection_failed".to_string(),
            operation: "inject_cdp_bridge".to_string(),
            error: "renderer unavailable".to_string(),
            stage: Some("startup.renderer_injection".to_string()),
            recoverable: Some(false),
            context: serde_json::json!({"debugPort": 9229}),
        };

        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["event"], "injection_failed");
        assert_eq!(value["operation"], "inject_cdp_bridge");
        assert_eq!(value["error"], "renderer unavailable");
        assert_eq!(value["platform"], "windows");
        assert_eq!(value["versions"]["codey"], "0.7.3");
        assert_eq!(value["versions"]["electron"], "151.0.0");
        assert_eq!(value["stage"], "startup.renderer_injection");
        assert_eq!(value["recoverable"], false);
        assert_eq!(value["context"]["debugPort"], 9229);
        assert_eq!(value["timestamp"].as_str().unwrap().len(), 25);
        assert!(value.get("timestampMs").is_none());
        assert!(value.get("pid").is_none());
        assert!(value.get("durationMs").is_none());
        assert!(value.get("attempts").is_none());
        assert!(value.get("timeoutMs").is_none());
    }

    #[test]
    fn helper_accepts_and_normalizes_a_failure_batch() {
        let records = parse_helper_records(
            &serde_json::json!([
                {
                    "timestamp": "2026-08-18T09:36:53Z",
                    "platform": "windows",
                    "event": "patch_failed",
                    "operation": "renderer_patch:model allowlist",
                    "error": "gate matched 0 times"
                },
                {
                    "timestamp": "2026-08-18T09:36:54Z",
                    "platform": "windows",
                    "event": "patch_failed",
                    "operation": "renderer_patch:model visibility",
                    "error": "gate matched 0 times"
                }
            ])
            .to_string(),
        )
        .unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].timestamp, "2026-08-18T17:36:53+08:00");
        assert_eq!(
            records[1].versions.codey.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn helper_rejects_empty_and_oversized_batches() {
        assert!(parse_helper_records("[]").is_err());
        let record = serde_json::json!({
            "timestamp": "2026-08-18T09:36:53Z",
            "platform": "windows",
            "event": "patch_failed",
            "operation": "renderer_patch:test",
            "error": "gate matched 0 times"
        });
        let oversized = Value::Array(vec![record; MAX_HELPER_RECORDS + 1]).to_string();
        assert!(parse_helper_records(&oversized).is_err());
    }

    #[test]
    fn legacy_helper_records_remain_compatible() {
        let mut record = serde_json::from_value::<ErrorRecord>(serde_json::json!({
            "timestamp": "2026-08-02T11:21:24.543+08:00",
            "timestampMs": 1_785_640_884_543_i64,
            "pid": 4255,
            "platform": "macos",
            "event": "patch_failed",
            "operation": "renderer_patch:model visibility",
            "error": "gate matched 0 times",
            "context": {"matchCount": 0}
        }))
        .unwrap();

        normalize_record(&mut record);
        assert_eq!(record.stage, None);
        assert_eq!(record.recoverable, None);
        assert_eq!(record.timestamp, "2026-08-02T11:21:24+08:00");
        assert_eq!(
            record.versions.codey.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(record.context["matchCount"], 0);
    }

    #[test]
    fn legacy_runtime_versions_move_out_of_context() {
        let mut record = serde_json::from_value::<ErrorRecord>(serde_json::json!({
            "timestamp": "2026-08-15T08:28:19.264Z",
            "platform": "windows",
            "event": "patch_failed",
            "operation": "renderer_patch:model visibility",
            "error": "gate matched 0 times",
            "context": {
                "matchCount": 0,
                "electronVersion": "151.0.0",
                "chromeVersion": "151.0.0",
                "nodeVersion": "24.14.0"
            }
        }))
        .unwrap();

        normalize_record(&mut record);

        assert_eq!(record.timestamp, "2026-08-15T16:28:19+08:00");
        assert_eq!(record.versions.electron.as_deref(), Some("151.0.0"));
        assert_eq!(record.versions.chrome.as_deref(), Some("151.0.0"));
        assert_eq!(record.versions.node.as_deref(), Some("24.14.0"));
        assert_eq!(record.context, serde_json::json!({"matchCount": 0}));
    }

    #[test]
    fn same_day_failures_are_appended() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(ERROR_LOG_FILE);
        let today = beijing_now().date_naive();
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
        let first_day = beijing_now().date_naive();
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
        let today = beijing_now().date_naive();
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
        let today = beijing_now().date_naive();
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
