use std::collections::{BTreeMap, btree_map};
use std::fmt;
use std::io;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use serde::de::{IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer as _};
use serde_json::value::RawValue;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const DEFAULT_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const DEFAULT_MAX_QUEUED_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_PENDING_REQUESTS: usize = 1024;
const MAX_POOLED_FRAMES: usize = 4;
const MAX_POOLED_FRAME_CAPACITY: usize = 512 * 1024;
const INITIAL_FRAME_CAPACITY: usize = 8 * 1024;
const MAX_ID_BYTES: usize = 256;
const MAX_LABEL_CHARS: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolLimits {
    pub max_frame_bytes: usize,
    pub max_queued_bytes: usize,
    pub max_pending_requests: usize,
}

impl Default for ProtocolLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
            max_queued_bytes: DEFAULT_MAX_QUEUED_BYTES,
            max_pending_requests: DEFAULT_MAX_PENDING_REQUESTS,
        }
    }
}

impl ProtocolLimits {
    pub fn from_environment() -> Self {
        let defaults = Self::default();
        let max_frame_bytes = bounded_env_usize(
            "CODEY_FASTCTX_MAX_FRAME_BYTES",
            defaults.max_frame_bytes,
            64 * 1024,
            64 * 1024 * 1024,
        );
        let max_queued_bytes = bounded_env_usize(
            "CODEY_FASTCTX_MAX_QUEUED_BYTES",
            defaults.max_queued_bytes.max(max_frame_bytes),
            max_frame_bytes,
            128 * 1024 * 1024,
        );
        let max_pending_requests = bounded_env_usize(
            "CODEY_FASTCTX_MAX_PENDING_REQUESTS",
            defaults.max_pending_requests,
            16,
            16 * 1024,
        );
        Self {
            max_frame_bytes,
            max_queued_bytes,
            max_pending_requests,
        }
    }
}

fn bounded_env_usize(name: &str, default: usize, minimum: usize, maximum: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (*value >= minimum) && (*value <= maximum))
        .unwrap_or(default)
}

#[derive(Clone, Default)]
struct FramePool {
    frames: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl FramePool {
    fn take(&self) -> Vec<u8> {
        self.frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(INITIAL_FRAME_CAPACITY))
    }

    fn recycle(&self, mut bytes: Vec<u8>) {
        if bytes.capacity() > MAX_POOLED_FRAME_CAPACITY {
            return;
        }
        bytes.clear();
        let mut frames = self
            .frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if frames.len() < MAX_POOLED_FRAMES {
            frames.push(bytes);
        }
    }
}

pub struct ProtocolFrame {
    bytes: Option<Vec<u8>>,
    permit: Option<OwnedSemaphorePermit>,
    pool: FramePool,
}

impl ProtocolFrame {
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_deref().unwrap_or_default()
    }
}

impl Deref for ProtocolFrame {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

impl Drop for ProtocolFrame {
    fn drop(&mut self) {
        self.permit.take();
        if let Some(bytes) = self.bytes.take() {
            self.pool.recycle(bytes);
        }
    }
}

#[derive(Clone)]
pub struct FrameReader {
    limits: ProtocolLimits,
    pool: FramePool,
    queued_bytes: Arc<Semaphore>,
}

impl FrameReader {
    pub fn new(limits: ProtocolLimits) -> Self {
        Self {
            limits,
            pool: FramePool::default(),
            queued_bytes: Arc::new(Semaphore::new(limits.max_queued_bytes)),
        }
    }

    pub async fn read_line<R>(&self, reader: &mut R) -> io::Result<Option<ProtocolFrame>>
    where
        R: AsyncBufRead + Unpin,
    {
        let mut bytes = self.pool.take();
        bytes.clear();
        let read = {
            let mut limited = reader.take((self.limits.max_frame_bytes + 1) as u64);
            limited.read_until(b'\n', &mut bytes).await?
        };
        if read == 0 {
            self.pool.recycle(bytes);
            return Ok(None);
        }
        if bytes.len() > self.limits.max_frame_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("MCP 单帧超过 {} 字节上限", self.limits.max_frame_bytes),
            ));
        }
        if !bytes.ends_with(b"\n") {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "MCP 消息在换行分隔符前结束",
            ));
        }
        if bytes.len() > self.limits.max_queued_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "MCP 单帧需要 {} 字节，但队列预算只有 {} 字节",
                    bytes.len(),
                    self.limits.max_queued_bytes
                ),
            ));
        }
        let permit_count = u32::try_from(bytes.len().max(1))
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "MCP 帧无法计入字节预算"))?;
        let permit = self
            .queued_bytes
            .clone()
            .acquire_many_owned(permit_count)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "MCP 帧队列已经关闭"))?;
        Ok(Some(ProtocolFrame {
            bytes: Some(bytes),
            permit: Some(permit),
            pool: self.pool.clone(),
        }))
    }
}

pub struct ProtocolState {
    initialize_request: Option<Vec<u8>>,
    initialize_id: Option<String>,
    initialized_notification: Option<Vec<u8>>,
    initialization_complete: bool,
    pending: BTreeMap<String, PendingRequest>,
    max_pending_requests: usize,
}

impl Default for ProtocolState {
    fn default() -> Self {
        Self {
            initialize_request: None,
            initialize_id: None,
            initialized_notification: None,
            initialization_complete: false,
            pending: BTreeMap::new(),
            max_pending_requests: DEFAULT_MAX_PENDING_REQUESTS,
        }
    }
}

impl ProtocolState {
    pub fn new(max_pending_requests: usize) -> Self {
        Self {
            max_pending_requests,
            ..Self::default()
        }
    }

    pub fn with_default_limits() -> Self {
        Self::new(DEFAULT_MAX_PENDING_REQUESTS)
    }

    pub fn observe_client(&mut self, line: &[u8]) -> Result<()> {
        let payload = trim_json_leading_whitespace(trim_line_ending(line));
        match payload.first().copied() {
            Some(b'{') => {
                let Ok(envelope) = serde_json::from_slice::<ProtocolEnvelope<'_>>(payload) else {
                    return Ok(());
                };
                self.observe_client_envelope(&envelope, Some(line))?;
            }
            Some(b'[') => {
                if serde_json::from_slice::<IgnoredAny>(payload).is_err() {
                    return Ok(());
                }
                self.observe_client_batch(payload)?;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn observe_server(&mut self, line: &[u8]) {
        let payload = trim_json_leading_whitespace(trim_line_ending(line));
        match payload.first().copied() {
            Some(b'{') => {
                if let Ok(envelope) = serde_json::from_slice::<ProtocolEnvelope<'_>>(payload) {
                    self.observe_server_envelope(&envelope);
                }
            }
            Some(b'[') if serde_json::from_slice::<IgnoredAny>(payload).is_ok() => {
                let _ = visit_envelope_batch(payload, |envelope| {
                    self.observe_server_envelope(envelope);
                    Ok(())
                });
            }
            _ => {}
        }
    }

    pub fn initialize_request(&self) -> Option<&[u8]> {
        self.initialize_request.as_deref()
    }

    pub fn initialize_id(&self) -> Option<&str> {
        self.initialize_id.as_deref()
    }

    pub fn initialized_notification(&self) -> Option<&[u8]> {
        self.initialized_notification.as_deref()
    }

    pub fn initialization_complete(&self) -> bool {
        self.initialization_complete
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn take_pending_errors(&mut self) -> PendingErrorIter {
        PendingErrorIter {
            pending: std::mem::take(&mut self.pending).into_values(),
        }
    }

    fn observe_client_envelope(
        &mut self,
        envelope: &ProtocolEnvelope<'_>,
        original_line: Option<&[u8]>,
    ) -> Result<()> {
        let pending = pending_request(envelope);
        if let Some((key, _)) = pending.as_ref() {
            self.ensure_pending_id_available(key, 0)?;
        }

        match envelope.method {
            Some("initialize") => {
                if let Some(line) = original_line {
                    self.initialize_request = Some(line.to_vec());
                    self.initialize_id = envelope.id.and_then(canonical_id_key);
                    self.initialization_complete = false;
                }
            }
            Some("notifications/initialized") => {
                if let Some(line) = original_line {
                    self.initialized_notification = Some(line.to_vec());
                }
            }
            _ => {}
        }

        if let Some((key, request)) = pending {
            self.pending.insert(key, request);
        }
        Ok(())
    }

    fn observe_client_batch(&mut self, payload: &[u8]) -> Result<()> {
        let mut staged = BTreeMap::new();
        visit_envelope_batch(payload, |envelope| {
            let Some((key, request)) = pending_request(envelope) else {
                return Ok(());
            };
            if staged.contains_key(&key) {
                bail!("FastCtx MCP 拒绝批处理中重复的 JSON-RPC request id：{key}");
            }
            self.ensure_pending_id_available(&key, staged.len())?;
            staged.insert(key, request);
            Ok(())
        })?;
        self.pending.append(&mut staged);
        Ok(())
    }

    fn ensure_pending_id_available(&self, key: &str, staged_requests: usize) -> Result<()> {
        if self.pending.contains_key(key) {
            bail!("FastCtx MCP 拒绝重复的 JSON-RPC request id：{key}");
        }
        if self.pending.len().saturating_add(staged_requests) >= self.max_pending_requests {
            bail!(
                "FastCtx MCP 在途请求达到 {} 条上限，拒绝继续积累未完成状态",
                self.max_pending_requests
            );
        }
        Ok(())
    }

    fn observe_server_envelope(&mut self, envelope: &ProtocolEnvelope<'_>) {
        if envelope.method.is_some() || (!envelope.result_present && !envelope.error_present) {
            return;
        }
        let Some(key) = envelope.id.and_then(canonical_id_key) else {
            return;
        };
        if self.initialize_id.as_deref() == Some(key.as_str()) && envelope.result_present {
            self.initialization_complete = true;
        }
        self.pending.remove(&key);
    }
}

fn pending_request(envelope: &ProtocolEnvelope<'_>) -> Option<(String, PendingRequest)> {
    let method = envelope.method?;
    let id = envelope.id?;
    let key = canonical_id_key(id)?;
    let (label, kind) = if method == "tools/call" {
        let tool_name = envelope.params.and_then(tool_name_from_params);
        (
            tool_name
                .map(|name| format!("tools/call ({})", bounded_label(name)))
                .unwrap_or_else(|| method.to_string()),
            PendingKind::for_tool_call(tool_name),
        )
    } else {
        (bounded_label(method), PendingKind::Other)
    };
    Some((
        key,
        PendingRequest {
            id: id.to_owned(),
            label,
            kind,
        },
    ))
}

struct EnvelopeBatchVisitor<'a, F> {
    visit: &'a mut F,
}

impl<'de, F> Visitor<'de> for EnvelopeBatchVisitor<'_, F>
where
    F: FnMut(&ProtocolEnvelope<'de>) -> Result<()>,
{
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON-RPC batch array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while let Some(envelope) = sequence.next_element::<ProtocolEnvelope<'de>>()? {
            (self.visit)(&envelope).map_err(serde::de::Error::custom)?;
        }
        Ok(())
    }
}

fn visit_envelope_batch<'de, F>(payload: &'de [u8], mut visit: F) -> Result<()>
where
    F: FnMut(&ProtocolEnvelope<'de>) -> Result<()>,
{
    let mut deserializer = serde_json::Deserializer::from_slice(payload);
    deserializer.deserialize_seq(EnvelopeBatchVisitor { visit: &mut visit })?;
    deserializer.end()?;
    Ok(())
}

pub struct PendingErrorIter {
    pending: btree_map::IntoValues<String, PendingRequest>,
}

impl Iterator for PendingErrorIter {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        self.pending.next().map(recovery_error_line)
    }
}

struct PendingRequest {
    id: Box<RawValue>,
    label: String,
    kind: PendingKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingKind {
    ReadOnly,
    Write,
    Other,
}

impl PendingKind {
    fn for_tool_call(tool_name: Option<&str>) -> Self {
        match tool_name {
            Some("replace") => Self::Write,
            Some("grep" | "glob" | "inspect_local_file") => Self::ReadOnly,
            _ => Self::Other,
        }
    }
}

fn recovery_error_line(request: PendingRequest) -> Vec<u8> {
    let message = match request.kind {
        PendingKind::Write => format!(
            "FastCtx 已在控制中心连接中断后恢复；运行中的 {} 请求未被重放，以免重复修改文件。请确认当前文件状态后重试。",
            request.label
        ),
        PendingKind::ReadOnly => format!(
            "FastCtx 已在控制中心连接中断后恢复；运行中的 {} 请求未被重放；只读请求无副作用，可直接重试。",
            request.label
        ),
        PendingKind::Other => format!(
            "FastCtx 已在控制中心连接中断后恢复；运行中的 {} 请求未被重放。请确认状态后重试。",
            request.label
        ),
    };
    let mut line = serde_json::to_vec(&RecoveryError {
        jsonrpc: "2.0",
        id: request.id,
        error: RecoveryErrorBody {
            code: -32001,
            message,
            data: RecoveryErrorData {
                recoverable: true,
                request_replayed: false,
            },
        },
    })
    .expect("JSON-RPC recovery error is serializable");
    line.push(b'\n');
    line
}

#[derive(Serialize)]
struct RecoveryError {
    jsonrpc: &'static str,
    id: Box<RawValue>,
    error: RecoveryErrorBody,
}

#[derive(Serialize)]
struct RecoveryErrorBody {
    code: i32,
    message: String,
    data: RecoveryErrorData,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryErrorData {
    recoverable: bool,
    request_replayed: bool,
}

#[derive(Deserialize)]
struct ProtocolEnvelope<'a> {
    #[serde(default, borrow)]
    id: Option<&'a RawValue>,
    #[serde(default, borrow)]
    method: Option<&'a str>,
    #[serde(default, borrow)]
    params: Option<&'a RawValue>,
    #[serde(default, rename = "result", deserialize_with = "present")]
    result_present: bool,
    #[serde(default, rename = "error", deserialize_with = "present")]
    error_present: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseDisposition {
    Unrelated,
    Success,
    Error,
    Invalid,
}

pub fn response_disposition(line: &[u8], expected_id: &str) -> ResponseDisposition {
    let payload = trim_json_leading_whitespace(trim_line_ending(line));
    let Ok(envelope) = serde_json::from_slice::<ProtocolEnvelope<'_>>(payload) else {
        return ResponseDisposition::Invalid;
    };
    let Some(id) = envelope.id.and_then(canonical_id_key) else {
        return ResponseDisposition::Unrelated;
    };
    if id != expected_id {
        return ResponseDisposition::Unrelated;
    }
    if envelope.method.is_some() {
        return ResponseDisposition::Invalid;
    }
    if envelope.error_present {
        ResponseDisposition::Error
    } else if envelope.result_present {
        ResponseDisposition::Success
    } else {
        ResponseDisposition::Invalid
    }
}

fn present<'de, D>(deserializer: D) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    IgnoredAny::deserialize(deserializer)?;
    Ok(true)
}

#[derive(Deserialize)]
struct ToolCallParams<'a> {
    #[serde(default, borrow)]
    name: Option<&'a str>,
}

fn tool_name_from_params(params: &RawValue) -> Option<&str> {
    serde_json::from_str::<ToolCallParams<'_>>(params.get())
        .ok()
        .and_then(|params| params.name)
}

fn canonical_id_key(id: &RawValue) -> Option<String> {
    if id.get().len() > MAX_ID_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(id.get()).ok()?;
    match value {
        serde_json::Value::String(_) | serde_json::Value::Number(_) => {
            serde_json::to_string(&value).ok()
        }
        _ => None,
    }
}

fn bounded_label(value: &str) -> String {
    let mut characters = value.chars();
    let mut label = characters
        .by_ref()
        .take(MAX_LABEL_CHARS)
        .collect::<String>();
    if characters.next().is_some() {
        label.push('…');
    }
    label
}

fn trim_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn trim_json_leading_whitespace(line: &[u8]) -> &[u8] {
    let first_content = line
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        .unwrap_or(line.len());
    &line[first_content..]
}

// Bring Serialize into this module after the protocol structs so the hot path
// remains explicit about which types are deserialized versus emitted.
use serde::Serialize;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio::io::BufReader;

    fn line(value: Value) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        bytes
    }

    #[test]
    fn protocol_limits_are_bounded_even_when_environment_is_absent_or_invalid() {
        let defaults = ProtocolLimits::default();
        assert_eq!(defaults.max_frame_bytes, DEFAULT_MAX_FRAME_BYTES);
        assert_eq!(defaults.max_queued_bytes, DEFAULT_MAX_QUEUED_BYTES);
        assert_eq!(defaults.max_pending_requests, DEFAULT_MAX_PENDING_REQUESTS);

        let configured = ProtocolLimits::from_environment();
        assert!((64 * 1024..=64 * 1024 * 1024).contains(&configured.max_frame_bytes));
        assert!(
            (configured.max_frame_bytes..=128 * 1024 * 1024).contains(&configured.max_queued_bytes)
        );
        assert!((16..=16 * 1024).contains(&configured.max_pending_requests));
    }

    #[tokio::test]
    async fn frame_reader_reuses_successful_frames_and_reports_clean_eof() {
        let limits = ProtocolLimits {
            max_frame_bytes: 64 * 1024,
            max_queued_bytes: 128 * 1024,
            max_pending_requests: 16,
        };
        let reader = FrameReader::new(limits);
        let bytes = b"{\"jsonrpc\":\"2.0\"}\n";
        let mut input = BufReader::new(bytes.as_slice());
        let frame = reader.read_line(&mut input).await.unwrap().unwrap();
        assert_eq!(frame.as_bytes(), bytes);
        assert_eq!(&*frame, bytes);
        drop(frame);
        assert!(reader.read_line(&mut input).await.unwrap().is_none());

        let mut second = BufReader::new(bytes.as_slice());
        let recycled = reader.read_line(&mut second).await.unwrap().unwrap();
        assert_eq!(recycled.as_bytes(), bytes);
    }

    #[tokio::test]
    async fn frame_reader_rejects_a_truncated_frame() {
        let limits = ProtocolLimits {
            max_frame_bytes: 64 * 1024,
            max_queued_bytes: 128 * 1024,
            max_pending_requests: 16,
        };
        let reader = FrameReader::new(limits);
        let mut input = BufReader::new(b"{\"id\":1}".as_slice());
        let error = match reader.read_line(&mut input).await {
            Ok(_) => panic!("truncated frame must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn frame_reader_rejects_an_impossible_queue_budget_without_waiting() {
        let limits = ProtocolLimits {
            max_frame_bytes: 64,
            max_queued_bytes: 4,
            max_pending_requests: 16,
        };
        let reader = FrameReader::new(limits);
        let mut input = BufReader::new(b"{\"id\":1}\n".as_slice());
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            reader.read_line(&mut input),
        )
        .await
        .expect("invalid limits must fail instead of waiting forever");
        assert!(matches!(
            result,
            Err(error) if error.kind() == io::ErrorKind::InvalidInput
        ));
    }

    #[test]
    fn initialization_and_batch_responses_are_reconciled_without_payload_retention() {
        let mut state = ProtocolState::default();
        let initialize = line(json!({
            "jsonrpc": "2.0",
            "id": "init-1",
            "method": "initialize",
            "params": {}
        }));
        state.observe_client(&initialize).unwrap();
        assert_eq!(state.initialize_request(), Some(initialize.as_slice()));
        assert_eq!(state.initialize_id(), Some("\"init-1\""));
        assert!(!state.initialization_complete());

        let initialized = line(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
        state.observe_client(&initialized).unwrap();
        assert_eq!(
            state.initialized_notification(),
            Some(initialized.as_slice())
        );
        state.observe_server(&line(json!({
            "jsonrpc": "2.0",
            "id": "init-1",
            "result": {"capabilities": {}}
        })));
        assert!(state.initialization_complete());

        state
            .observe_client(&line(json!([
                {"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {}}
            ])))
            .unwrap();
        assert_eq!(state.pending_len(), 2);
        state.observe_server(&line(json!([
            {"jsonrpc": "2.0", "id": 1, "result": {"tools": []}},
            {"jsonrpc": "2.0", "id": 2, "error": {"code": -1}}
        ])));
        assert_eq!(state.pending_len(), 0);
    }

    #[test]
    fn leading_json_whitespace_does_not_bypass_protocol_observation() {
        let mut state = ProtocolState::with_default_limits();
        let mut initialize = b" \t".to_vec();
        initialize.extend(line(json!({
            "jsonrpc": "2.0",
            "id": "init-1",
            "method": "initialize",
            "params": {}
        })));
        state.observe_client(&initialize).unwrap();
        assert_eq!(state.initialize_id(), Some("\"init-1\""));

        let mut response = b" \t".to_vec();
        response.extend(line(json!({
            "jsonrpc": "2.0",
            "id": "init-1",
            "result": {"capabilities": {}}
        })));
        state.observe_server(&response);
        assert!(state.initialization_complete());

        let mut batch = b" \t".to_vec();
        batch.extend(line(json!([
            {"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"}
        ])));
        state.observe_client(&batch).unwrap();
        assert_eq!(state.pending_len(), 2);
    }

    #[test]
    fn recovery_response_requires_a_real_result_for_the_expected_id() {
        assert_eq!(
            response_disposition(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":null}\n", "7"),
            ResponseDisposition::Success
        );
        assert_eq!(
            response_disposition(b"{\"jsonrpc\":\"2.0\",\"id\":7}\n", "7"),
            ResponseDisposition::Invalid
        );
        assert_eq!(
            response_disposition(
                b"{\"jsonrpc\":\"2.0\",\"id\":7,\"error\":{\"code\":-1}}\n",
                "7"
            ),
            ResponseDisposition::Error
        );
        assert_eq!(
            response_disposition(b"{\"jsonrpc\":\"2.0\",\"id\":8,\"result\":{}}\n", "7"),
            ResponseDisposition::Unrelated
        );
    }

    #[test]
    fn malformed_notifications_and_unsupported_ids_do_not_create_pending_state() {
        let mut state = ProtocolState::with_default_limits();
        for bytes in [
            b"not-json\n".as_slice(),
            b"{bad-json}\n".as_slice(),
            b"[{bad-json}]\n".as_slice(),
        ] {
            state.observe_client(bytes).unwrap();
            state.observe_server(bytes);
        }
        state
            .observe_client(&line(json!({"method": "notifications/progress"})))
            .unwrap();
        state
            .observe_client(&line(json!({
                "id": {"unsupported": true},
                "method": "tools/list"
            })))
            .unwrap();
        state
            .observe_client(&line(json!({
                "id": "x".repeat(MAX_ID_BYTES + 1),
                "method": "tools/list"
            })))
            .unwrap();
        state.observe_server(&line(json!({"id": 1})));
        state.observe_server(&line(json!({
            "id": 1,
            "method": "notifications/progress",
            "result": {}
        })));
        assert_eq!(state.pending_len(), 0);
    }

    #[test]
    fn recovery_errors_distinguish_read_write_and_other_requests() {
        let mut state = ProtocolState::with_default_limits();
        for (id, method, params) in [
            ("write", "tools/call", json!({"name": "replace"})),
            ("read", "tools/call", json!({"name": "glob"})),
            ("other", "resources/read", json!({})),
            (
                "long-label",
                "tools/call",
                json!({"name": "x".repeat(MAX_LABEL_CHARS + 1)}),
            ),
        ] {
            state
                .observe_client(&line(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": method,
                    "params": params
                })))
                .unwrap();
        }
        let errors = state
            .take_pending_errors()
            .map(|bytes| serde_json::from_slice::<Value>(&bytes).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 4);
        let message_for = |id: &str| {
            errors
                .iter()
                .find(|error| error["id"] == id)
                .and_then(|error| error["error"]["message"].as_str())
                .unwrap()
        };
        assert!(message_for("write").contains("以免重复修改文件"));
        assert!(message_for("read").contains("只读请求无副作用"));
        assert!(message_for("other").contains("请确认状态后重试"));
        assert!(message_for("long-label").contains('…'));
        assert_eq!(state.pending_len(), 0);
    }

    #[test]
    fn large_server_payload_is_observed_without_retaining_result_data() {
        let mut state = ProtocolState::with_default_limits();
        state
            .observe_client(&line(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": {"name": "grep", "arguments": {}}
            })))
            .unwrap();
        let large = line(json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": {"text": "A".repeat(1024 * 1024)}
        }));
        state.observe_server(&large);
        assert_eq!(state.pending_len(), 0);
    }

    #[test]
    fn pending_requests_are_bounded_before_forwarding() {
        let mut state = ProtocolState::new(1);
        state
            .observe_client(&line(json!({"id": 1, "method": "tools/list"})))
            .unwrap();
        let error = state
            .observe_client(&line(json!({"id": 2, "method": "tools/list"})))
            .unwrap_err();
        assert!(error.to_string().contains("达到 1 条上限"));

        let mut batch_state = ProtocolState::new(1);
        let error = batch_state
            .observe_client(&line(json!([
                {"id": 1, "method": "tools/list"},
                {"id": 2, "method": "tools/list"}
            ])))
            .unwrap_err();
        assert!(error.to_string().contains("达到 1 条上限"));
        assert_eq!(batch_state.pending_len(), 0);
    }

    #[test]
    fn duplicate_request_id_is_rejected_without_replacing_the_original_request() {
        let mut state = ProtocolState::with_default_limits();
        state
            .observe_client(&line(json!({
                "jsonrpc": "2.0",
                "id": "same-id",
                "method": "tools/call",
                "params": {"name": "replace"}
            })))
            .unwrap();

        let error = state
            .observe_client(&line(json!({
                "jsonrpc": "2.0",
                "id": "same-id",
                "method": "tools/call",
                "params": {"name": "glob"}
            })))
            .unwrap_err();
        assert!(error.to_string().contains("重复的 JSON-RPC request id"));
        assert_eq!(state.pending_len(), 1);

        let recovery: Value = serde_json::from_slice(
            &state
                .take_pending_errors()
                .next()
                .expect("the original request must remain pending"),
        )
        .unwrap();
        assert_eq!(recovery["id"], "same-id");
        assert!(
            recovery["error"]["message"]
                .as_str()
                .unwrap()
                .contains("以免重复修改文件"),
            "the duplicate read request must not overwrite the original write request: {recovery}"
        );
    }

    #[test]
    fn duplicate_ids_make_a_client_batch_fail_atomically() {
        let mut state = ProtocolState::with_default_limits();
        state
            .observe_client(&line(json!({
                "jsonrpc": "2.0",
                "id": "already-pending",
                "method": "tools/list"
            })))
            .unwrap();

        let error = state
            .observe_client(&line(json!([
                {"jsonrpc": "2.0", "id": "new", "method": "tools/list"},
                {"jsonrpc": "2.0", "id": "new", "method": "tools/call", "params": {"name": "replace"}}
            ])))
            .unwrap_err();
        assert!(error.to_string().contains("批处理中重复"));
        assert_eq!(state.pending_len(), 1);

        let error = state
            .observe_client(&line(json!([
                {"jsonrpc": "2.0", "id": "another-new", "method": "tools/list"},
                {"jsonrpc": "2.0", "id": "already-pending", "method": "tools/list"}
            ])))
            .unwrap_err();
        assert!(error.to_string().contains("重复的 JSON-RPC request id"));
        assert_eq!(state.pending_len(), 1);
    }

    #[test]
    fn bounded_labels_scan_only_through_the_truncation_boundary() {
        let exact = "界".repeat(MAX_LABEL_CHARS);
        let longer = format!("{exact}界");
        assert_eq!(bounded_label(&exact), exact);
        assert_eq!(bounded_label(&longer), format!("{exact}…"));
    }

    #[tokio::test]
    async fn frame_reader_rejects_oversized_lines_without_unbounded_growth() {
        let limits = ProtocolLimits {
            max_frame_bytes: 64 * 1024,
            max_queued_bytes: 128 * 1024,
            max_pending_requests: 16,
        };
        let reader = FrameReader::new(limits);
        let bytes = vec![b'A'; limits.max_frame_bytes + 1];
        let mut input = BufReader::new(bytes.as_slice());
        let result = reader.read_line(&mut input).await;
        assert!(matches!(
            result,
            Err(error) if error.kind() == io::ErrorKind::InvalidData
        ));
    }

    #[test]
    fn interrupted_write_is_never_replayed() {
        let mut state = ProtocolState::with_default_limits();
        state
            .observe_client(&line(json!({
                "jsonrpc": "2.0",
                "id": "replace-7",
                "method": "tools/call",
                "params": {"name": "replace", "arguments": {}}
            })))
            .unwrap();
        let errors = state.take_pending_errors().collect::<Vec<_>>();
        assert_eq!(errors.len(), 1);
        let error: Value = serde_json::from_slice(&errors[0]).unwrap();
        assert_eq!(error["id"], "replace-7");
        assert_eq!(error["error"]["data"]["requestReplayed"], false);
    }
}
