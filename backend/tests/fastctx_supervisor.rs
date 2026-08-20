use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(30);

#[test]
fn control_center_crash_keeps_the_mcp_connection_usable() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    let event_log = temp.path().join("runtime-events.log");
    let local = home.join("local-app-data");
    let runtime = home.join("runtime");
    let temp_files = home.join("tmp");
    let app_state = home.join(".codex-session-delete");
    for directory in [&home, &workspace, &local, &runtime, &temp_files] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(workspace.join("sample.txt"), "recovered\n").unwrap();

    let mut command = Command::new(env!("CARGO_BIN_EXE_codey-fastctx"));
    command
        .arg("--codey-fastctx-mcp")
        .current_dir(&workspace)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("LOCALAPPDATA", &local)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("TMPDIR", &temp_files)
        .env("TMP", &temp_files)
        .env("TEMP", &temp_files)
        .env("CODEY_APP_STATE_DIR", &app_state)
        .env("FASTCTX_TEST_RUNTIME_EVENT_LOG", &event_log)
        .env("FASTCTX_TEST_RUNTIME_IDLE_MS", "60000")
        .env("FASTCTX_NO_PARENT_WATCH", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (responses_tx, responses_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if responses_tx.send(line).is_err() {
                return;
            }
        }
    });
    let captured_stderr = Arc::new(Mutex::new(String::new()));
    let stderr_capture = Arc::clone(&captured_stderr);
    std::thread::spawn(move || {
        let mut captured = stderr_capture.lock().unwrap();
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            captured.push_str(&line);
            captured.push('\n');
        }
    });

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "codey-supervisor-test", "version": "1"}
            }
        }),
    );
    let initialized = response_with_id(&responses_rx, 1);
    assert!(initialized.get("result").is_some(), "{initialized}");
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
    );
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
    );
    let listed = response_with_id(&responses_rx, 2);
    let tools = listed["result"]["tools"].as_array().unwrap();
    let mut names = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["glob", "grep", "inspect_local_file", "replace"]);
    for tool in tools {
        assert_portable_tool_schema(&tool["inputSchema"]);
    }

    let first_host = wait_for_host_starts(&event_log, 1)[0];
    terminate_process(first_host, true);
    let hosts = wait_for_host_starts(&event_log, 2);
    assert_ne!(hosts[0], hosts[1]);
    let recovery = wait_for_error_record(&app_state.join("codey-errors.log"));
    assert_eq!(recovery["event"], "fastctx_transport_closed");
    assert_eq!(recovery["operation"], "run_fastctx_mcp_worker");
    assert_eq!(recovery["stage"], "runtime.fastctx_mcp_worker");
    assert_eq!(recovery["recoverable"], true);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "glob",
                "arguments": {"pattern": "**/*"}
            }
        }),
    );
    let response = response_with_id(&responses_rx, 3);
    assert_eq!(response["result"]["isError"], false, "{response}");

    drop(stdin);
    let status = wait_for_child(&mut child);
    let stderr = captured_stderr.lock().unwrap().clone();
    assert!(status.success(), "sidecar failed with {status}: {stderr}");
    terminate_process(hosts[1], false);
}

fn assert_portable_tool_schema(value: &Value) {
    let values = value
        .as_object()
        .unwrap_or_else(|| panic!("FastCtx schema node must be an object: {value}"));
    for forbidden in [
        "$ref",
        "$defs",
        "oneOf",
        "anyOf",
        "allOf",
        "const",
        "$schema",
        "additionalProperties",
        "format",
    ] {
        assert!(
            !values.contains_key(forbidden),
            "FastCtx schema contains unsupported {forbidden}: {value}"
        );
    }
    if let Some(schema_type) = values.get("type") {
        assert!(
            schema_type.is_string(),
            "FastCtx schema type must be a scalar string: {value}"
        );
        assert_ne!(schema_type.as_str(), Some("null"), "{value}");
    }
    if let Some(properties) = values.get("properties").and_then(Value::as_object) {
        for property in properties.values() {
            assert_portable_tool_schema(property);
        }
    }
    if let Some(items) = values.get("items") {
        assert_portable_tool_schema(items);
    }
}

fn send(stdin: &mut impl Write, value: Value) {
    serde_json::to_writer(&mut *stdin, &value).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
}

fn response_with_id(receiver: &mpsc::Receiver<std::io::Result<String>>, id: i64) -> Value {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for MCP response {id}"
        );
        let line = receiver
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("MCP stdout closed before response {id}: {error}"))
            .unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        if value.get("id").and_then(Value::as_i64) == Some(id) {
            return value;
        }
    }
}

fn wait_for_host_starts(path: &Path, count: usize) -> Vec<u32> {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        let pids = std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix("START ")?.parse().ok())
            .collect::<Vec<_>>();
        if pids.len() >= count {
            return pids;
        }
        assert!(
            Instant::now() < deadline,
            "only {} FastCtx host starts observed",
            pids.len()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_error_record(path: &Path) -> Value {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Some(record) = contents
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|record| record["event"] == "fastctx_transport_closed")
        {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "recoverable FastCtx transport record was not written to {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_child(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("Codey FastCtx sidecar did not exit after stdin closed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn supervisor_forwards_large_worker_lines_without_losing_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let (mut child, mut stdin, responses_rx) = spawn_supervisor_with_test_worker(&temp);
    initialize_test_worker_session(&mut stdin, &responses_rx);

    const RESPONSE_BYTES: usize = 256 * 1024;
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "test/large_response",
            "params": {"bytes": RESPONSE_BYTES}
        }),
    );
    // 大行写入期间并发 client 消息：select! 若直接对 worker 管道 read_until，
    // 取消分支时会丢弃已读入的半行字节。
    for _ in 0..5 {
        send(
            &mut stdin,
            json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": {"id": 2, "reason": "test"}
            }),
        );
    }
    let large = response_with_id(&responses_rx, 2);
    let text = large["result"]["text"].as_str().unwrap();
    assert!(text.bytes().all(|byte| byte == b'A'));
    let prefix = "{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"text\":\"";
    let suffix = "\"}}";
    assert_eq!(text.len(), RESPONSE_BYTES - prefix.len() - suffix.len() - 1);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "test/large_response",
            "params": {"bytes": 64}
        }),
    );
    let small = response_with_id(&responses_rx, 3);
    assert!(small.get("result").is_some(), "{small}");

    drop(stdin);
    let status = wait_for_child(&mut child);
    assert!(status.success(), "supervisor failed with {status}");
}

#[test]
fn supervisor_stops_recovering_after_repeated_worker_disconnects() {
    let temp = tempfile::tempdir().unwrap();
    let (mut child, mut stdin, responses_rx) = spawn_supervisor_with_test_worker(&temp);
    initialize_test_worker_session(&mut stdin, &responses_rx);

    // 前两次断开后恢复：在途请求获得可恢复错误，后续请求由新 worker 正常服务。
    for (exit_id, probe_id) in [(10, 11), (20, 21)] {
        send(
            &mut stdin,
            json!({
                "jsonrpc": "2.0",
                "id": exit_id,
                "method": "test/exit",
                "params": {"code": 75}
            }),
        );
        let interrupted = response_with_id(&responses_rx, exit_id);
        assert_eq!(
            interrupted["error"]["code"].as_i64(),
            Some(-32001),
            "{interrupted}"
        );
        assert_eq!(
            interrupted["error"]["data"]["recoverable"].as_bool(),
            Some(true),
            "{interrupted}"
        );

        send(
            &mut stdin,
            json!({
                "jsonrpc": "2.0",
                "id": probe_id,
                "method": "test/large_response",
                "params": {"bytes": 64}
            }),
        );
        let probe = response_with_id(&responses_rx, probe_id);
        assert!(probe.get("result").is_some(), "{probe}");
    }

    // 窗口内第三次断开：错误仍返回给在途请求，但监督器不再拉起新 worker。
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 30,
            "method": "test/exit",
            "params": {"code": 75}
        }),
    );
    let interrupted = response_with_id(&responses_rx, 30);
    assert_eq!(
        interrupted["error"]["code"].as_i64(),
        Some(-32001),
        "{interrupted}"
    );

    let status = wait_for_child(&mut child);
    assert!(
        !status.success(),
        "supervisor must bail after repeated recoveries"
    );
}

#[cfg(unix)]
#[test]
fn supervisor_reaps_worker_after_client_stdin_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (mut child, mut stdin, responses_rx) = spawn_supervisor_with_test_worker(&temp);
    initialize_test_worker_session(&mut stdin, &responses_rx);
    let worker_pid = wait_for_test_worker_start(temp.path(), 1)[0];

    stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":99").unwrap();
    stdin.flush().unwrap();
    drop(stdin);

    let status = wait_for_child(&mut child);
    assert!(
        !status.success(),
        "truncated stdin must fail the supervisor"
    );
    wait_for_process_gone(worker_pid);
}

#[cfg(unix)]
#[test]
fn supervisor_reaps_worker_after_worker_stdout_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (mut child, mut stdin, responses_rx) =
        spawn_supervisor_with_test_worker_env(&temp, &[("CODEY_FASTCTX_MAX_FRAME_BYTES", "65536")]);
    initialize_test_worker_session(&mut stdin, &responses_rx);
    let worker_pid = wait_for_test_worker_start(temp.path(), 1)[0];

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "test/large_response",
            "params": {"bytes": 128 * 1024}
        }),
    );
    drop(stdin);

    let status = wait_for_child(&mut child);
    assert!(
        !status.success(),
        "oversized worker stdout must fail the supervisor"
    );
    wait_for_process_gone(worker_pid);
}

#[cfg(unix)]
#[test]
fn supervisor_reaps_worker_after_forwarding_stdout_failure() {
    let temp = tempfile::tempdir().unwrap();
    let CloseableStdoutSupervisor {
        mut child,
        mut stdin,
        responses_rx,
        close_stdout,
        stdout_closed,
        stdout_ready,
    } = spawn_supervisor_with_closeable_stdout(&temp);
    initialize_test_worker_session(&mut stdin, &responses_rx);
    let worker_pid = wait_for_test_worker_start(temp.path(), 1)[0];

    // `response_with_id` can receive the initialize response before the reader
    // thread has finished its post-line close check. Wait for that boundary so
    // the close request is deterministically applied after response 98 instead
    // of occasionally closing the pipe after the initialize response.
    stdout_ready
        .recv_timeout(PROCESS_TIMEOUT)
        .expect("supervisor stdout reader did not reach the next-line boundary");

    close_stdout.send(()).unwrap();
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 98,
            "method": "test/large_response",
            "params": {"bytes": 64}
        }),
    );
    let response = response_with_id(&responses_rx, 98);
    assert!(response.get("result").is_some(), "{response}");
    stdout_closed
        .recv_timeout(PROCESS_TIMEOUT)
        .expect("supervisor stdout reader did not close");
    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "test/large_response",
            "params": {"bytes": 64}
        }),
    );
    drop(stdin);

    let status = wait_for_child(&mut child);
    assert!(
        !status.success(),
        "closed supervisor stdout must fail response forwarding"
    );
    wait_for_process_gone(worker_pid);
}

fn spawn_supervisor_with_test_worker(
    temp: &tempfile::TempDir,
) -> (
    Child,
    std::process::ChildStdin,
    mpsc::Receiver<std::io::Result<String>>,
) {
    spawn_supervisor_with_test_worker_env(temp, &[])
}

fn spawn_supervisor_with_test_worker_env(
    temp: &tempfile::TempDir,
    extra_env: &[(&str, &str)],
) -> (
    Child,
    std::process::ChildStdin,
    mpsc::Receiver<std::io::Result<String>>,
) {
    let mut child = spawn_test_supervisor_process(temp, extra_env);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (responses_tx, responses_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if responses_tx.send(line).is_err() {
                return;
            }
        }
    });
    (child, stdin, responses_rx)
}

#[cfg(unix)]
struct CloseableStdoutSupervisor {
    child: Child,
    stdin: std::process::ChildStdin,
    responses_rx: mpsc::Receiver<std::io::Result<String>>,
    close_stdout: mpsc::Sender<()>,
    stdout_closed: mpsc::Receiver<()>,
    stdout_ready: mpsc::Receiver<()>,
}

#[cfg(unix)]
fn spawn_supervisor_with_closeable_stdout(temp: &tempfile::TempDir) -> CloseableStdoutSupervisor {
    let mut child = spawn_test_supervisor_process(temp, &[]);
    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (responses_tx, responses_rx) = mpsc::channel();
    let (close_tx, close_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next() {
            if responses_tx.send(line).is_err() || close_rx.try_recv().is_ok() {
                drop(lines);
                let _ = closed_tx.send(());
                return;
            }
            if ready_tx.send(()).is_err() {
                return;
            }
        }
    });
    CloseableStdoutSupervisor {
        child,
        stdin,
        responses_rx,
        close_stdout: close_tx,
        stdout_closed: closed_rx,
        stdout_ready: ready_rx,
    }
}

fn spawn_test_supervisor_process(temp: &tempfile::TempDir, extra_env: &[(&str, &str)]) -> Child {
    let home = temp.path().join("home");
    let workspace = temp.path().join("workspace");
    let local = home.join("local-app-data");
    let runtime = home.join("runtime");
    let temp_files = home.join("tmp");
    let app_state = home.join(".codex-session-delete");
    for directory in [&home, &workspace, &local, &runtime, &temp_files] {
        std::fs::create_dir_all(directory).unwrap();
    }

    let mut command = Command::new(env!("CARGO_BIN_EXE_codey-fastctx"));
    command
        .arg("--codey-fastctx-mcp")
        .current_dir(&workspace)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("LOCALAPPDATA", &local)
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("TMPDIR", &temp_files)
        .env("TMP", &temp_files)
        .env("TEMP", &temp_files)
        .env("CODEY_APP_STATE_DIR", &app_state)
        .env(
            "CODEY_FASTCTX_TEST_WORKER_ARGUMENT",
            "--codey-fastctx-mcp-test-worker",
        )
        .env(
            "CODEY_FASTCTX_TEST_WORKER_PID_LOG",
            temp.path().join("test-worker-pids.log"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in extra_env {
        command.env(name, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().unwrap()
}

#[cfg(unix)]
fn wait_for_test_worker_start(root: &Path, count: usize) -> Vec<u32> {
    let path = root.join("test-worker-pids.log");
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        let pids = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix("START ")?.parse::<u32>().ok())
            .collect::<Vec<_>>();
        if pids.len() >= count {
            return pids;
        }
        assert!(
            Instant::now() < deadline,
            "only {} FastCtx test worker starts observed in {}",
            pids.len(),
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn wait_for_process_gone(pid: u32) {
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    loop {
        let result = unsafe { libc::kill(pid as i32, 0) };
        if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "FastCtx worker {pid} survived supervisor shutdown"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn initialize_test_worker_session(
    stdin: &mut impl Write,
    responses_rx: &mpsc::Receiver<std::io::Result<String>>,
) {
    send(
        stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "codey-supervisor-test", "version": "1"}
            }
        }),
    );
    let initialized = response_with_id(responses_rx, 1);
    assert!(initialized.get("result").is_some(), "{initialized}");
    send(
        stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }),
    );
}

#[cfg(unix)]
fn terminate_process(pid: u32, required: bool) {
    let result = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    if required {
        assert_eq!(result, 0, "failed to terminate FastCtx host {pid}");
    }
}

#[cfg(windows)]
fn terminate_process(pid: u32, required: bool) {
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    if required {
        assert!(status.success(), "failed to terminate FastCtx host {pid}");
    }
}
