#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! FastCtx MCP STDIO 服务的独立程序。拆分出主程序是为了让 Codey 本体不携带
//! FastCtx 及其内嵌 o200k 分词器常量；本程序仅在用户启用 FastCtx 上下文工具
//! 后由 Codex 按需拉起。

use std::ffi::OsStr;

const CODEY_FASTCTX_MCP_ARGUMENT: &str = "--codey-fastctx-mcp";

fn main() {
    if let Err(error) = run() {
        eprintln!("Codey FastCtx 运行失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let force_mcp_server = should_force_mcp_server(std::env::args_os());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let result = if force_mcp_server {
                fastctx::cli::run_server().await
            } else {
                // FastCtx 会用当前可执行文件拉起 runtime-bootstrap 和
                // runtime-host。必须把这些内部子命令交回它的 CLI 分发器；
                // 否则子进程会再次进入 MCP 模式并卡满启动超时。
                fastctx::cli::run().await
            };
            result.map(|_| ()).map_err(anyhow::Error::msg)
        })
}

fn should_force_mcp_server<I, S>(arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    arguments
        .into_iter()
        .nth(1)
        .is_some_and(|argument| argument.as_ref() == OsStr::new(CODEY_FASTCTX_MCP_ARGUMENT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codey_marker_forces_the_stdio_mcp_entry() {
        assert!(should_force_mcp_server([
            "codey-fastctx",
            CODEY_FASTCTX_MCP_ARGUMENT,
        ]));
    }

    #[test]
    fn fastctx_internal_runtime_commands_reach_the_cli_dispatcher() {
        assert!(!should_force_mcp_server([
            "codey-fastctx",
            "runtime-bootstrap",
        ]));
        assert!(!should_force_mcp_server(["codey-fastctx", "runtime-host",]));
    }
}
