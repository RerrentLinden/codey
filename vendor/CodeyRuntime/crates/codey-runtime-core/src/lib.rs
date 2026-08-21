pub mod app_paths;
pub mod assets;
pub mod bridge;
pub mod cdp;
pub(crate) mod codex_home;
pub(crate) mod codex_local_storage;
pub mod codex_sqlite;
mod computer_use_guard;
pub mod config_manager;
pub mod diagnostic_log;
pub(crate) mod http_client;
pub mod install;
pub mod launcher;
pub mod model_catalog;
pub mod model_suffix;
pub mod models;
pub(crate) mod native_menu;
pub mod paths;
pub mod plugin_marketplace;
pub mod ports;
pub mod protocol_proxy;
pub mod relay_config;
pub mod relay_rotation;
pub mod relay_switch;
pub mod routes;
pub mod script_market;
pub mod settings;
pub mod status;
pub(crate) mod stepwise;
pub mod update;
pub mod upstream_worktree;
pub mod user_scripts;
pub mod version;
pub mod watcher;
#[cfg(windows)]
mod windows_integration;
pub mod zed_remote;

#[cfg(windows)]
pub fn windows_create_no_window() -> u32 {
    windows_integration::CREATE_NO_WINDOW
}

#[cfg(windows)]
pub fn windows_open_url(url: &str) -> anyhow::Result<()> {
    windows_integration::open_url(url)
}

#[cfg(windows)]
pub fn windows_activate_process_window(process_id: u32) -> bool {
    windows_integration::activate_process_window(process_id)
}

#[cfg(windows)]
pub fn windows_apply_codey_icon_to_process_window(
    process_id: u32,
    icon_resource_path: std::path::PathBuf,
) -> bool {
    windows_integration::apply_codey_icon_to_process_window(process_id, icon_resource_path)
}

#[cfg(windows)]
pub fn windows_enumerate_processes() -> Vec<windows_integration::WindowsProcessInfo> {
    windows_integration::enumerate_processes()
}

#[cfg(windows)]
pub fn windows_process_paths_equal(left: &std::path::Path, right: &std::path::Path) -> bool {
    windows_integration::process_paths_equal(left, right)
}

#[cfg(windows)]
pub fn windows_terminate_process_if_matches(
    process_id: u32,
    expected_path: &std::path::Path,
    expected_creation_time: u64,
) -> bool {
    windows_integration::terminate_process_if_matches(
        process_id,
        expected_path,
        expected_creation_time,
    )
}

#[cfg(windows)]
pub fn windows_terminate_process_if_creation_matches(
    process_id: u32,
    expected_creation_time: u64,
) -> bool {
    windows_integration::terminate_process_if_creation_matches(process_id, expected_creation_time)
}
