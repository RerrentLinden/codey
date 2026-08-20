#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! FastCtx MCP STDIO 服务的独立程序。拆分出主程序是为了让 Codey 本体不携带
//! FastCtx 及其内嵌 o200k 分词器常量；本程序仅在用户启用 FastCtx 上下文工具
//! 后由 Codex 按需拉起。

use std::ffi::OsStr;

#[path = "codey-fastctx/supervisor.rs"]
mod fastctx_supervisor;

const CODEY_FASTCTX_MCP_ARGUMENT: &str = "--codey-fastctx-mcp";
const CODEY_FASTCTX_MCP_WORKER_ARGUMENT: &str = "--codey-fastctx-mcp-worker";
// 集成测试（backend/tests/fastctx_supervisor.rs）专用：监督器通过
// CODEY_FASTCTX_TEST_WORKER_ARGUMENT 把 worker 子进程换成该模式的可脚本化
// MCP 桩，以确定性构造大行输出与可恢复退出。真实宿主不会传入此参数。
const CODEY_FASTCTX_TEST_WORKER_ARGUMENT: &str = "--codey-fastctx-mcp-test-worker";
const TEST_WORKER_ARGUMENT_ENV: &str = "CODEY_FASTCTX_TEST_WORKER_ARGUMENT";
const TEST_WORKER_PID_LOG_ENV: &str = "CODEY_FASTCTX_TEST_WORKER_PID_LOG";

fn main() {
    let mode = FastCtxMode::from_arguments(std::env::args_os());
    codey_lib::install_crash_log_hook("fastctx", mode.stage());
    if let Err(error) = run(mode) {
        let error = format!("{error:#}");
        let event = classify_fastctx_failure(&error);
        let recoverable = mode.failure_is_recoverable(event);
        codey_lib::record_process_failure_with_recoverability(
            event,
            mode.operation(),
            error.clone(),
            mode.stage(),
            recoverable,
        );
        eprintln!("Codey FastCtx 运行失败：{error}");
        std::process::exit(if recoverable {
            fastctx_supervisor::RECOVERABLE_WORKER_EXIT_CODE
        } else {
            1
        });
    }
}

fn run(mode: FastCtxMode) -> anyhow::Result<()> {
    if mode == FastCtxMode::TestWorker {
        return run_test_worker();
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(async move {
        match mode {
            FastCtxMode::Mcp => {
                let worker_argument = std::env::var(TEST_WORKER_ARGUMENT_ENV)
                    .ok()
                    .filter(|argument| !argument.is_empty())
                    .unwrap_or_else(|| CODEY_FASTCTX_MCP_WORKER_ARGUMENT.to_string());
                fastctx_supervisor::run(&worker_argument).await
            }
            FastCtxMode::McpWorker => fastctx::cli::run_server()
                .await
                .map(|_| ())
                .map_err(anyhow::Error::msg),
            _ => {
                // FastCtx 会用当前可执行文件拉起 runtime-bootstrap 和
                // runtime-host。必须把这些内部子命令交回它的 CLI 分发器；
                // 否则子进程会再次进入 MCP 模式并卡满启动超时。
                fastctx::cli::run()
                    .await
                    .map(|_| ())
                    .map_err(anyhow::Error::msg)
            }
        }
    });
    // stdio 读取线程运行在不可取消的阻塞池里：直接 drop 运行时会无限等待它们，
    // 错误路径（如监督器熔断 bail）因此永远不退出，宿主观察不到 MCP 失败。
    runtime.shutdown_background();
    result
}

/// 可脚本化的 MCP 桩，仅服务 backend/tests/fastctx_supervisor.rs：
/// - `initialize`：回最小 capabilities；
/// - `test/large_response`（参数 `bytes`）：按请求字节数分块写出单行大响应，
///   强迫监督器跨多次 poll 读取；
/// - `test/exit`（参数 `code`）：以指定状态码退出，模拟 transport 断开；
/// - 其他方法（含 `notifications/cancelled`）：忽略。
fn run_test_worker() -> anyhow::Result<()> {
    use std::fs::OpenOptions;
    use std::io::{BufRead, BufReader, Write};

    use anyhow::Context;

    if let Some(path) = std::env::var_os(TEST_WORKER_PID_LOG_ENV) {
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("打开测试 worker PID 日志失败：{}", path.to_string_lossy()))?;
        writeln!(log, "START {}", std::process::id())?;
        log.flush()?;
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in BufReader::new(stdin.lock()).lines() {
        let line = line.context("测试 worker 读取请求失败")?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(object) = value.as_object() else {
            continue;
        };
        let method = object
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match method {
            "initialize" => {
                let id = object.get("id").cloned().unwrap_or(serde_json::Value::Null);
                let response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"capabilities": {}}
                });
                serde_json::to_writer(&mut stdout, &response)?;
                stdout.write_all(b"\n")?;
                stdout.flush()?;
            }
            "test/large_response" => {
                let id = object.get("id").cloned().unwrap_or(serde_json::Value::Null);
                let bytes = object
                    .get("params")
                    .and_then(|params| params.get("bytes"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as usize;
                write_large_test_response(&mut stdout, &id, bytes)?;
            }
            "test/exit" => {
                let code = object
                    .get("params")
                    .and_then(|params| params.get("code"))
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0) as i32;
                stdout.flush()?;
                std::process::exit(code);
            }
            _ => {}
        }
    }
    stdout.flush()?;
    Ok(())
}

fn write_large_test_response(
    stdout: &mut impl std::io::Write,
    id: &serde_json::Value,
    bytes: usize,
) -> anyhow::Result<()> {
    let prefix = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"text\":\"");
    let suffix = "\"}}";
    let fill = bytes.saturating_sub(prefix.len() + suffix.len() + 1);
    stdout.write_all(prefix.as_bytes())?;
    let mut remaining = fill;
    while remaining > 0 {
        let chunk = remaining.min(2048);
        stdout.write_all(&vec![b'A'; chunk])?;
        stdout.flush()?;
        remaining -= chunk;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    stdout.write_all(suffix.as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FastCtxMode {
    Mcp,
    McpWorker,
    TestWorker,
    RuntimeBootstrap,
    RuntimeHost,
    Cli,
}

impl FastCtxMode {
    fn from_arguments<I, S>(arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match arguments.into_iter().nth(1).as_ref().map(AsRef::as_ref) {
            Some(argument) if argument == OsStr::new(CODEY_FASTCTX_MCP_ARGUMENT) => Self::Mcp,
            Some(argument) if argument == OsStr::new(CODEY_FASTCTX_MCP_WORKER_ARGUMENT) => {
                Self::McpWorker
            }
            Some(argument) if argument == OsStr::new(CODEY_FASTCTX_TEST_WORKER_ARGUMENT) => {
                Self::TestWorker
            }
            Some(argument) if argument == OsStr::new("runtime-bootstrap") => Self::RuntimeBootstrap,
            Some(argument) if argument == OsStr::new("runtime-host") => Self::RuntimeHost,
            _ => Self::Cli,
        }
    }

    const fn stage(self) -> &'static str {
        match self {
            Self::Mcp => "runtime.fastctx_mcp",
            Self::McpWorker => "runtime.fastctx_mcp_worker",
            Self::TestWorker => "runtime.fastctx_test_worker",
            Self::RuntimeBootstrap => "runtime.fastctx_bootstrap",
            Self::RuntimeHost => "runtime.fastctx_host",
            Self::Cli => "runtime.fastctx_cli",
        }
    }

    const fn operation(self) -> &'static str {
        match self {
            Self::Mcp => "run_fastctx_mcp",
            Self::McpWorker => "run_fastctx_mcp_worker",
            Self::TestWorker => "run_fastctx_test_worker",
            Self::RuntimeBootstrap => "run_fastctx_runtime_bootstrap",
            Self::RuntimeHost => "run_fastctx_runtime_host",
            Self::Cli => "run_fastctx_cli",
        }
    }

    fn failure_is_recoverable(self, event: &str) -> bool {
        self == Self::McpWorker && event == "fastctx_transport_closed"
    }
}

fn classify_fastctx_failure(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if [
        "transport closed",
        "connection closed",
        "control-center connection failed",
        "control-center input task failed",
        "control-center output task failed",
        "cannot forward mcp stdin to the fastctx control center",
        "unexpectedly closed",
        "broken pipe",
        "stdin read",
        "unexpected eof",
    ]
    .iter()
    .any(|marker| error.contains(marker))
    {
        "fastctx_transport_closed"
    } else {
        "fastctx_process_failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codey_marker_forces_the_stdio_mcp_entry() {
        assert_eq!(
            FastCtxMode::from_arguments(["codey-fastctx", CODEY_FASTCTX_MCP_ARGUMENT]),
            FastCtxMode::Mcp
        );
    }

    #[test]
    fn fastctx_internal_runtime_commands_reach_the_cli_dispatcher() {
        assert_ne!(
            FastCtxMode::from_arguments(["codey-fastctx", "runtime-bootstrap"]),
            FastCtxMode::Mcp
        );
        assert_ne!(
            FastCtxMode::from_arguments(["codey-fastctx", "runtime-host"]),
            FastCtxMode::Mcp
        );
    }

    #[test]
    fn supervisor_and_worker_arguments_have_distinct_modes() {
        assert_eq!(
            FastCtxMode::from_arguments(["codey-fastctx", CODEY_FASTCTX_MCP_ARGUMENT]),
            FastCtxMode::Mcp
        );
        assert_eq!(
            FastCtxMode::from_arguments(["codey-fastctx", CODEY_FASTCTX_MCP_WORKER_ARGUMENT]),
            FastCtxMode::McpWorker
        );
        assert_eq!(
            FastCtxMode::from_arguments(["codey-fastctx", CODEY_FASTCTX_TEST_WORKER_ARGUMENT]),
            FastCtxMode::TestWorker
        );
    }

    #[test]
    fn runtime_commands_have_distinct_diagnostic_stages() {
        assert_eq!(
            FastCtxMode::from_arguments(["codey-fastctx", "runtime-bootstrap"]).stage(),
            "runtime.fastctx_bootstrap"
        );
        assert_eq!(
            FastCtxMode::from_arguments(["codey-fastctx", "runtime-host"]).stage(),
            "runtime.fastctx_host"
        );
    }

    #[test]
    fn transport_failures_are_classified_separately() {
        assert_eq!(
            classify_fastctx_failure("control center connection unexpectedly closed"),
            "fastctx_transport_closed"
        );
        assert_eq!(
            classify_fastctx_failure(
                "The FastCtx control-center connection failed: The pipe has been ended. (os error 109)"
            ),
            "fastctx_transport_closed"
        );
        assert_eq!(
            classify_fastctx_failure("Cannot start the MCP server"),
            "fastctx_process_failed"
        );
        assert!(FastCtxMode::McpWorker.failure_is_recoverable("fastctx_transport_closed"));
        assert!(!FastCtxMode::Mcp.failure_is_recoverable("fastctx_transport_closed"));
        assert!(!FastCtxMode::McpWorker.failure_is_recoverable("fastctx_process_failed"));
    }
}
