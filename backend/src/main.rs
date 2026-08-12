#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    if let Err(error) = run() {
        eprintln!("Codey 运行失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    if codey_lib::run_subagent_gate_hook_if_requested()? {
        return Ok(());
    }
    if codey_lib::run_error_log_helper_if_requested()? {
        return Ok(());
    }
    if codey_lib::run_update_helper_if_requested()? {
        return Ok(());
    }
    codey_lib::run_desktop_application()
}
