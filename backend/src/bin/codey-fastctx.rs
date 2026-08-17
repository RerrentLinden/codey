#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! FastCtx MCP STDIO 服务的独立程序。拆分出主程序是为了让 Codey 本体不携带
//! FastCtx 及其内嵌 o200k 分词器常量；本程序仅在用户启用 FastCtx 上下文工具
//! 后由 Codex 按需拉起。

use std::ffi::OsStr;

#[path = "codey-fastctx/supervisor.rs"]
mod fastctx_supervisor;

const CODEY_FASTCTX_MCP_ARGUMENT: &str = "--codey-fastctx-mcp";
const CODEY_FASTCTX_MCP_WORKER_ARGUMENT: &str = "--codey-fastctx-mcp-worker";

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
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            match mode {
                FastCtxMode::Mcp => {
                    fastctx_supervisor::run(CODEY_FASTCTX_MCP_WORKER_ARGUMENT).await
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
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FastCtxMode {
    Mcp,
    McpWorker,
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
            Some(argument) if argument == OsStr::new("runtime-bootstrap") => Self::RuntimeBootstrap,
            Some(argument) if argument == OsStr::new("runtime-host") => Self::RuntimeHost,
            _ => Self::Cli,
        }
    }

    const fn stage(self) -> &'static str {
        match self {
            Self::Mcp => "runtime.fastctx_mcp",
            Self::McpWorker => "runtime.fastctx_mcp_worker",
            Self::RuntimeBootstrap => "runtime.fastctx_bootstrap",
            Self::RuntimeHost => "runtime.fastctx_host",
            Self::Cli => "runtime.fastctx_cli",
        }
    }

    const fn operation(self) -> &'static str {
        match self {
            Self::Mcp => "run_fastctx_mcp",
            Self::McpWorker => "run_fastctx_mcp_worker",
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
