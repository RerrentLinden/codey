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

    let first_host = wait_for_host_starts(&event_log, 1)[0];
    terminate_process(first_host, true);
    let hosts = wait_for_host_starts(&event_log, 2);
    assert_ne!(hosts[0], hosts[1]);
    let recovery = wait_for_error_record(&home.join(".codex-session-delete/codey-errors.log"));
    assert_eq!(recovery["event"], "fastctx_transport_closed");
    assert_eq!(recovery["operation"], "run_fastctx_mcp_worker");
    assert_eq!(recovery["stage"], "runtime.fastctx_mcp_worker");
    assert_eq!(recovery["recoverable"], true);

    send(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "glob",
                "arguments": {"pattern": "**/*"}
            }
        }),
    );
    let response = response_with_id(&responses_rx, 2);
    assert_eq!(response["result"]["isError"], false, "{response}");

    drop(stdin);
    let status = wait_for_child(&mut child);
    let stderr = captured_stderr.lock().unwrap().clone();
    assert!(status.success(), "sidecar failed with {status}: {stderr}");
    terminate_process(hosts[1], false);
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
