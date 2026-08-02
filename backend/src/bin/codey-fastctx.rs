#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! FastCtx MCP STDIO 服务的独立程序。拆分出主程序是为了让 Codey 本体不携带
//! FastCtx 及其内嵌 o200k 分词器常量；本程序仅在用户启用 FastCtx 上下文工具
//! 后由 Codex 按需拉起。

fn main() {
    if let Err(error) = run() {
        eprintln!("Codey FastCtx 运行失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            fastctx::cli::run_server()
                .await
                .map(|_| ())
                .map_err(anyhow::Error::msg)
        })
}
