use std::collections::BTreeMap;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;

pub const RECOVERABLE_WORKER_EXIT_CODE: i32 = 75;
const RECOVERY_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

pub async fn run(worker_argument: &'static str) -> Result<()> {
    let (client_tx, mut client_rx) = mpsc::channel(32);
    tokio::spawn(read_client_input(client_tx));

    let mut worker = Worker::spawn(worker_argument).await?;
    let mut state = ProtocolState::default();
    let mut client_open = true;
    let mut stdout = tokio::io::stdout();

    loop {
        enum Event {
            Client(Option<ClientInput>),
            WorkerOutput(std::io::Result<Option<Vec<u8>>>),
            WorkerExit(std::io::Result<ExitStatus>),
        }

        let event = {
            let Worker {
                child,
                stdin: _,
                stdout,
            } = &mut worker;
            tokio::select! {
                input = client_rx.recv(), if client_open => Event::Client(input),
                output = read_protocol_line(stdout) => Event::WorkerOutput(output),
                status = child.wait() => Event::WorkerExit(status),
            }
        };

        match event {
            Event::Client(Some(ClientInput::Line(line))) => {
                state.observe_client(&line);
                let forwarded = write_protocol_line(
                    worker
                        .stdin
                        .as_mut()
                        .context("FastCtx worker stdin 已关闭")?,
                    &line,
                )
                .await;
                if let Err(error) = forwarded {
                    let status = worker.child.wait().await.with_context(|| {
                        format!("向 FastCtx worker 转发 MCP 请求失败（{error}），且等待退出失败")
                    })?;
                    if !recover_or_finish(
                        status,
                        client_open,
                        worker_argument,
                        &mut worker,
                        &mut state,
                        &mut stdout,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                }
            }
            Event::Client(Some(ClientInput::Error(error))) => {
                bail!("读取 MCP stdin 失败：{error}");
            }
            Event::Client(Some(ClientInput::Eof)) | Event::Client(None) => {
                client_open = false;
                if let Some(mut stdin) = worker.stdin.take() {
                    stdin
                        .shutdown()
                        .await
                        .context("关闭 FastCtx worker stdin 失败")?;
                }
            }
            Event::WorkerOutput(Ok(Some(line))) => {
                state.observe_server(&line);
                stdout
                    .write_all(&line)
                    .await
                    .context("转发 FastCtx MCP 响应失败")?;
                stdout.flush().await.context("刷新 MCP stdout 失败")?;
            }
            Event::WorkerOutput(Ok(None)) => {
                let status = worker
                    .child
                    .wait()
                    .await
                    .context("等待 FastCtx worker 退出失败")?;
                if !recover_or_finish(
                    status,
                    client_open,
                    worker_argument,
                    &mut worker,
                    &mut state,
                    &mut stdout,
                )
                .await?
                {
                    return Ok(());
                }
            }
            Event::WorkerOutput(Err(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                let status = worker
                    .child
                    .wait()
                    .await
                    .context("等待 FastCtx worker 退出失败")?;
                if !recover_or_finish(
                    status,
                    client_open,
                    worker_argument,
                    &mut worker,
                    &mut state,
                    &mut stdout,
                )
                .await?
                {
                    return Ok(());
                }
            }
            Event::WorkerOutput(Err(error)) => {
                bail!("读取 FastCtx worker stdout 失败：{error}");
            }
            Event::WorkerExit(Ok(status)) => {
                drain_worker_output(&mut worker, &mut state, &mut stdout).await?;
                if !recover_or_finish(
                    status,
                    client_open,
                    worker_argument,
                    &mut worker,
                    &mut state,
                    &mut stdout,
                )
                .await?
                {
                    return Ok(());
                }
            }
            Event::WorkerExit(Err(error)) => bail!("等待 FastCtx worker 退出失败：{error}"),
        }
    }
}

async fn drain_worker_output<W>(
    worker: &mut Worker,
    state: &mut ProtocolState,
    stdout: &mut W,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    loop {
        match read_protocol_line(&mut worker.stdout).await {
            Ok(Some(line)) => {
                state.observe_server(&line);
                stdout
                    .write_all(&line)
                    .await
                    .context("转发 FastCtx worker 退出前的 MCP 响应失败")?;
            }
            Ok(None) => break,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => bail!("读取 FastCtx worker 退出前的 stdout 失败：{error}"),
        }
    }
    stdout
        .flush()
        .await
        .context("刷新 FastCtx worker 退出前的 MCP 响应失败")?;
    Ok(())
}

async fn recover_or_finish<W>(
    status: ExitStatus,
    client_open: bool,
    worker_argument: &'static str,
    worker: &mut Worker,
    state: &mut ProtocolState,
    stdout: &mut W,
) -> Result<bool>
where
    W: AsyncWrite + Unpin,
{
    if !client_open {
        return Ok(false);
    }
    if status.code() != Some(RECOVERABLE_WORKER_EXIT_CODE) {
        bail!("FastCtx worker 在 MCP stdin 仍开启时退出：{status}");
    }
    if !state.initialization_complete {
        bail!("FastCtx worker 在 MCP 初始化完成前断开，无法恢复会话");
    }

    for error in state.take_pending_errors() {
        stdout
            .write_all(&error)
            .await
            .context("返回 FastCtx transport 恢复错误失败")?;
    }
    stdout
        .flush()
        .await
        .context("刷新 FastCtx transport 恢复错误失败")?;

    *worker = recover_worker(worker_argument, state).await?;
    Ok(true)
}

async fn recover_worker(worker_argument: &'static str, state: &ProtocolState) -> Result<Worker> {
    let mut worker = Worker::spawn(worker_argument).await?;
    let recovery = tokio::time::timeout(
        RECOVERY_HANDSHAKE_TIMEOUT,
        replay_initialization(&mut worker, state),
    )
    .await;
    match recovery {
        Ok(Ok(())) => Ok(worker),
        Ok(Err(error)) => {
            worker.terminate().await;
            Err(error)
        }
        Err(_) => {
            worker.terminate().await;
            bail!("恢复 FastCtx MCP 初始化超过 15 秒")
        }
    }
}

async fn replay_initialization(worker: &mut Worker, state: &ProtocolState) -> Result<()> {
    let request = state
        .initialize_request
        .as_ref()
        .context("缺少可安全重放的 MCP initialize 请求")?;
    let initialize_id = state
        .initialize_id
        .as_deref()
        .context("MCP initialize 请求缺少 id")?;
    write_protocol_line(
        worker
            .stdin
            .as_mut()
            .context("FastCtx worker stdin 已关闭")?,
        request,
    )
    .await
    .context("向恢复后的 FastCtx worker 重放 initialize 失败")?;

    loop {
        let Some(line) = read_protocol_line(&mut worker.stdout)
            .await
            .context("读取恢复后的 FastCtx initialize 响应失败")?
        else {
            let status = worker
                .child
                .wait()
                .await
                .context("等待恢复后的 FastCtx worker 退出失败")?;
            bail!("恢复后的 FastCtx worker 在 initialize 响应前退出：{status}");
        };
        let value: Value =
            serde_json::from_slice(&line).context("恢复后的 FastCtx worker 返回了无效 JSON")?;
        let Some(object) = value.as_object() else {
            continue;
        };
        if response_key(object).as_deref() != Some(initialize_id) {
            continue;
        }
        if let Some(error) = object.get("error") {
            bail!("恢复后的 FastCtx worker 拒绝 initialize：{error}");
        }
        break;
    }

    if let Some(notification) = &state.initialized_notification {
        write_protocol_line(
            worker
                .stdin
                .as_mut()
                .context("FastCtx worker stdin 已关闭")?,
            notification,
        )
        .await
        .context("向恢复后的 FastCtx worker重放 initialized 通知失败")?;
    }
    Ok(())
}

struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Worker {
    async fn spawn(worker_argument: &'static str) -> Result<Self> {
        let executable = std::env::current_exe().context("定位 Codey FastCtx worker 失败")?;
        let mut command = Command::new(executable);
        command
            .arg(worker_argument)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn().context("启动 Codey FastCtx worker 失败")?;
        let stdin = child.stdin.take().context("FastCtx worker 缺少 stdin")?;
        let stdout = child.stdout.take().context("FastCtx worker 缺少 stdout")?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    async fn terminate(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

enum ClientInput {
    Line(Vec<u8>),
    Eof,
    Error(String),
}

async fn read_client_input(sender: mpsc::Sender<ClientInput>) {
    let mut stdin = BufReader::new(tokio::io::stdin());
    loop {
        match read_protocol_line(&mut stdin).await {
            Ok(Some(line)) => {
                if sender.send(ClientInput::Line(line)).await.is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = sender.send(ClientInput::Eof).await;
                return;
            }
            Err(error) => {
                let _ = sender.send(ClientInput::Error(error.to_string())).await;
                return;
            }
        }
    }
}

async fn read_protocol_line<R>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    let read = reader.read_until(b'\n', &mut line).await?;
    if read == 0 {
        return Ok(None);
    }
    if !line.ends_with(b"\n") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "MCP 消息在换行分隔符前结束",
        ));
    }
    Ok(Some(line))
}

async fn write_protocol_line<W>(writer: &mut W, line: &[u8]) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(line).await?;
    if !line.ends_with(b"\n") {
        writer.write_all(b"\n").await?;
    }
    writer.flush().await
}

#[derive(Default)]
struct ProtocolState {
    initialize_request: Option<Vec<u8>>,
    initialize_id: Option<String>,
    initialized_notification: Option<Vec<u8>>,
    initialization_complete: bool,
    pending: BTreeMap<String, PendingRequest>,
}

struct PendingRequest {
    id: Value,
    label: String,
}

impl ProtocolState {
    fn observe_client(&mut self, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if let Some(object) = value.as_object() {
            match object.get("method").and_then(Value::as_str) {
                Some("initialize") => {
                    self.initialize_request = Some(line.to_vec());
                    self.initialize_id = request_key(object);
                    self.initialization_complete = false;
                }
                Some("notifications/initialized") => {
                    self.initialized_notification = Some(line.to_vec());
                }
                _ => {}
            }
        }
        visit_objects(&value, &mut |object| {
            let Some(method) = object.get("method").and_then(Value::as_str) else {
                return;
            };
            let Some(id) = object.get("id").cloned() else {
                return;
            };
            let Some(key) = value_key(&id) else {
                return;
            };
            let label = if method == "tools/call" {
                object
                    .get("params")
                    .and_then(Value::as_object)
                    .and_then(|params| params.get("name"))
                    .and_then(Value::as_str)
                    .map(|name| format!("tools/call ({name})"))
                    .unwrap_or_else(|| method.to_string())
            } else {
                method.to_string()
            };
            self.pending.insert(key, PendingRequest { id, label });
        });
    }

    fn observe_server(&mut self, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        visit_objects(&value, &mut |object| {
            let Some(key) = response_key(object) else {
                return;
            };
            if self.initialize_id.as_deref() == Some(key.as_str()) && object.get("result").is_some()
            {
                self.initialization_complete = true;
            }
            self.pending.remove(&key);
        });
    }

    fn take_pending_errors(&mut self) -> Vec<Vec<u8>> {
        let pending = std::mem::take(&mut self.pending);
        pending
            .into_values()
            .map(|request| {
                let mut line = serde_json::to_vec(&json!({
                    "jsonrpc": "2.0",
                    "id": request.id,
                    "error": {
                        "code": -32001,
                        "message": format!(
                            "FastCtx 已在控制中心连接中断后恢复；运行中的 {} 请求未被重放，以免重复修改文件。请确认当前文件状态后重试。",
                            request.label
                        ),
                        "data": {
                            "recoverable": true,
                            "requestReplayed": false
                        }
                    }
                }))
                .expect("JSON-RPC recovery error is serializable");
                line.push(b'\n');
                line
            })
            .collect()
    }
}

fn visit_objects(value: &Value, visitor: &mut impl FnMut(&Map<String, Value>)) {
    match value {
        Value::Object(object) => visitor(object),
        Value::Array(values) => {
            for value in values {
                if let Value::Object(object) = value {
                    visitor(object);
                }
            }
        }
        _ => {}
    }
}

fn request_key(object: &Map<String, Value>) -> Option<String> {
    object.get("id").and_then(value_key)
}

fn response_key(object: &Map<String, Value>) -> Option<String> {
    if object.get("method").is_some()
        || (!object.contains_key("result") && !object.contains_key("error"))
    {
        return None;
    }
    request_key(object)
}

fn value_key(value: &Value) -> Option<String> {
    match value {
        Value::String(_) | Value::Number(_) => serde_json::to_string(value).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(value: Value) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        bytes
    }

    #[test]
    fn initialization_is_cached_for_safe_worker_recovery() {
        let mut state = ProtocolState::default();
        let initialize = line(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        }));
        let initialized = line(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }));
        state.observe_client(&initialize);
        state.observe_server(&line(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"capabilities": {}}
        })));
        state.observe_client(&initialized);

        assert_eq!(
            state.initialize_request.as_deref(),
            Some(initialize.as_slice())
        );
        assert_eq!(
            state.initialized_notification.as_deref(),
            Some(initialized.as_slice())
        );
        assert!(state.initialization_complete);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn interrupted_replace_gets_one_recoverable_error_without_replay() {
        let mut state = ProtocolState::default();
        state.observe_client(&line(json!({
            "jsonrpc": "2.0",
            "id": "replace-7",
            "method": "tools/call",
            "params": {"name": "replace", "arguments": {}}
        })));

        let errors = state.take_pending_errors();
        assert_eq!(errors.len(), 1);
        let error: Value = serde_json::from_slice(&errors[0]).unwrap();
        assert_eq!(error["id"], "replace-7");
        assert_eq!(error["error"]["data"]["recoverable"], true);
        assert_eq!(error["error"]["data"]["requestReplayed"], false);
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("tools/call (replace)")
        );
        assert!(state.pending.is_empty());
    }

    #[test]
    fn completed_requests_are_not_reported_again_after_a_transport_close() {
        let mut state = ProtocolState::default();
        state.observe_client(&line(json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {"name": "grep"}
        })));
        state.observe_server(&line(json!({
            "jsonrpc": "2.0",
            "id": 9,
            "result": {"content": []}
        })));

        assert!(state.take_pending_errors().is_empty());
    }
}
