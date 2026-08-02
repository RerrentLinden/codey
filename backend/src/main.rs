#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    if let Err(error) = run() {
        eprintln!("Codey 运行失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    if codey_lib::run_error_log_helper_if_requested()? {
        return Ok(());
    }
    if codey_lib::run_update_helper_if_requested()? {
        return Ok(());
    }
    if std::env::args_os()
        .nth(1)
        .is_some_and(|argument| argument == "--codey-fastctx-mcp")
    {
        // 旧版临时配置可能仍以该参数调用主程序。FastCtx 服务现已拆分为
        // codey-fastctx sidecar，这里把请求代理过去，绝不把主程序当作
        // MCP 服务启动。
        let sidecar_name = if cfg!(windows) {
            "codey-fastctx.exe"
        } else {
            "codey-fastctx"
        };
        let sidecar = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join(sidecar_name)))
            .filter(|path| path.is_file());
        let Some(sidecar) = sidecar else {
            anyhow::bail!("FastCtx 服务已拆分为 codey-fastctx，但未在 Codey 程序目录找到它");
        };
        let status = std::process::Command::new(sidecar)
            .args(std::env::args_os().skip(1))
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    }

    let mut builder = tokio::runtime::Builder::new_multi_thread();
    // Codey is an I/O coordinator. Blocking filesystem/SQLite work already
    // runs on Tokio's blocking pool, so two async workers avoid creating a
    // CPU-count-sized thread team for every helper instance.
    builder.worker_threads(2);
    builder.enable_all().build()?.block_on(codey_lib::run())
}
