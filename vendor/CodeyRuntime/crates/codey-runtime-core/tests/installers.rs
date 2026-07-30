use codey_runtime_core::install::{
    InstallOptions, MANAGER_BUNDLE_ID, SILENT_BINARY, SILENT_BUNDLE_ID, app_bundle_names,
    build_macos_app_bundle, build_windows_entrypoint_plan, companion_binary_path_from_exe,
    default_install_root_strategy, macos_companion_bundle_identifier_from_exe, shortcut_names,
};

#[test]
fn windows_entrypoint_plan_contains_silent_and_manager_entrypoints() {
    let options = InstallOptions {
        install_root: Some("C:/Users/A/Desktop".into()),
        launcher_path: Some("C:/Tools/codey.exe".into()),
        manager_path: Some("C:/Tools/codey-manager.exe".into()),
        remove_owned_data: false,
    };

    let plan = build_windows_entrypoint_plan(&options);

    assert!(plan.silent_shortcut.ends_with("Codey.lnk"));
    assert!(plan.manager_shortcut.ends_with("Codey 管理工具.lnk"));
    assert_eq!(plan.launcher_path, "C:/Tools/codey.exe");
    assert_eq!(plan.manager_path, "C:/Tools/codey-manager.exe");
    assert_eq!(plan.silent_icon_path, "C:/Tools/codey.exe");
    assert_eq!(plan.manager_icon_path, "C:/Tools/codey-manager.exe");
    assert_eq!(plan.uninstall_key, "CodeyRuntime");
    assert_eq!(plan.legacy_uninstall_key, "Codey");
    assert_eq!(
        plan.uninstaller_path.replace('\\', "/"),
        "C:/Tools/uninstall.exe"
    );
    assert_eq!(
        plan.uninstall_command.replace('\\', "/"),
        "\"C:/Tools/uninstall.exe\""
    );
    assert_eq!(
        plan.quiet_uninstall_command.replace('\\', "/"),
        "\"C:/Tools/uninstall.exe\" /S"
    );
    assert_ne!(plan.uninstall_command, "\"C:/Tools/codey-manager.exe\"");
}

#[test]
fn windows_entrypoint_plan_can_request_owned_data_removal_without_shell_script() {
    let options = InstallOptions {
        install_root: Some("C:/Users/A/Desktop".into()),
        launcher_path: None,
        manager_path: None,
        remove_owned_data: true,
    };

    let plan = build_windows_entrypoint_plan(&options);

    assert!(plan.silent_shortcut.ends_with("Codey.lnk"));
    assert!(plan.manager_shortcut.ends_with("Codey 管理工具.lnk"));
    assert!(plan.remove_owned_data);
}

#[test]
fn macos_bundle_metadata_contains_silent_and_manager_apps() {
    let options = InstallOptions {
        install_root: Some("/Applications".into()),
        launcher_path: Some("/opt/Codey/codey".into()),
        manager_path: Some("/opt/Codey/codey-manager".into()),
        remove_owned_data: false,
    };

    let silent = build_macos_app_bundle(&options, false);
    let manager = build_macos_app_bundle(&options, true);

    assert!(silent.app_path.ends_with("Codey.app"));
    assert!(manager.app_path.ends_with("Codey 管理工具.app"));
    assert!(silent.info_plist.contains("<string>Codey</string>"));
    assert!(
        manager
            .info_plist
            .contains("<string>Codey 管理工具</string>")
    );
    assert_eq!(silent.binary_target_name.as_deref(), Some("codey"));
    assert_eq!(manager.binary_target_name.as_deref(), Some("codey-manager"));
    assert!(silent.launch_script.contains("$DIR/codey"));
    assert!(manager.launch_script.contains("$DIR/codey-manager"));
}

#[test]
fn installer_exports_expected_two_entrypoint_names() {
    assert_eq!(shortcut_names(), ("Codey.lnk", "Codey 管理工具.lnk"));
    assert_eq!(app_bundle_names(), ("Codey.app", "Codey 管理工具.app"));
}

#[test]
fn macos_dmg_includes_applications_shortcut_for_drag_install() {
    let Ok(script) = std::fs::read_to_string("../../scripts/installer/macos/package-dmg.sh") else {
        return;
    };

    assert!(script.contains("ln -s /Applications \"$STAGE/Applications\""));
}

#[test]
fn companion_binary_path_resolves_macos_silent_app_next_to_manager_app() {
    let root = tempfile::tempdir().unwrap();
    let manager_macos = root
        .path()
        .join("Codey 管理工具.app")
        .join("Contents")
        .join("MacOS");
    let silent_macos = root.path().join("Codey.app").join("Contents").join("MacOS");
    std::fs::create_dir_all(&manager_macos).unwrap();
    std::fs::create_dir_all(&silent_macos).unwrap();
    let manager_exe = manager_macos.join("CodeyRuntimeManager");
    let silent_binary = silent_macos.join("codey");
    std::fs::write(&manager_exe, "").unwrap();
    std::fs::write(&silent_binary, "").unwrap();

    let companion = companion_binary_path_from_exe(&manager_exe, SILENT_BINARY);

    assert_eq!(companion, silent_binary);
    assert_ne!(companion, manager_macos.join("codey"));
}

#[test]
fn companion_binary_path_resolves_macos_manager_app_next_to_silent_app() {
    let root = tempfile::tempdir().unwrap();
    let silent_macos = root.path().join("Codey.app").join("Contents").join("MacOS");
    let manager_macos = root
        .path()
        .join("Codey 管理工具.app")
        .join("Contents")
        .join("MacOS");
    std::fs::create_dir_all(&silent_macos).unwrap();
    std::fs::create_dir_all(&manager_macos).unwrap();
    let silent_exe = silent_macos.join("CodeyRuntime");
    let manager_binary = manager_macos.join("codey-manager");
    std::fs::write(&silent_exe, "").unwrap();
    std::fs::write(&manager_binary, "").unwrap();

    let companion =
        companion_binary_path_from_exe(&silent_exe, codey_runtime_core::install::MANAGER_BINARY);

    assert_eq!(companion, manager_binary);
}

#[test]
fn macos_companion_launch_uses_bundle_ids_from_app_translocation() {
    let manager_exe = std::path::Path::new(
        "/private/var/folders/x/AppTranslocation/manager-id/d/Codey 管理工具.app/Contents/MacOS/CodeyRuntimeManager",
    );
    let silent_exe = std::path::Path::new(
        "/private/var/folders/x/AppTranslocation/silent-id/d/Codey.app/Contents/MacOS/CodeyRuntime",
    );

    assert_eq!(
        macos_companion_bundle_identifier_from_exe(manager_exe, SILENT_BINARY),
        Some(SILENT_BUNDLE_ID)
    );
    assert_eq!(
        macos_companion_bundle_identifier_from_exe(
            silent_exe,
            codey_runtime_core::install::MANAGER_BINARY,
        ),
        Some(MANAGER_BUNDLE_ID)
    );
}

#[test]
fn macos_companion_launch_keeps_bare_binary_development_mode() {
    let manager_exe = std::path::Path::new("/tmp/target/debug/codey-manager");

    assert_eq!(
        macos_companion_bundle_identifier_from_exe(manager_exe, SILENT_BINARY),
        None
    );
}

#[test]
fn macos_bundle_does_not_wrap_the_bundle_executable_in_itself() {
    let root = tempfile::tempdir().unwrap();
    let silent_macos = root.path().join("Codey.app").join("Contents").join("MacOS");
    let manager_macos = root
        .path()
        .join("Codey 管理工具.app")
        .join("Contents")
        .join("MacOS");
    std::fs::create_dir_all(&silent_macos).unwrap();
    std::fs::create_dir_all(&manager_macos).unwrap();
    let silent_executable = silent_macos.join("CodeyRuntime");
    let silent_binary = silent_macos.join("codey");
    let manager_executable = manager_macos.join("CodeyRuntimeManager");
    let manager_binary = manager_macos.join("codey-manager");
    std::fs::write(&silent_executable, "").unwrap();
    std::fs::write(&silent_binary, "").unwrap();
    std::fs::write(&manager_executable, "").unwrap();
    std::fs::write(&manager_binary, "").unwrap();
    let options = InstallOptions {
        install_root: Some(root.path().into()),
        launcher_path: Some(silent_executable),
        manager_path: Some(manager_executable),
        remove_owned_data: false,
    };

    let silent = build_macos_app_bundle(&options, false);
    let manager = build_macos_app_bundle(&options, true);

    assert_eq!(silent.binary_source, Some(silent_binary));
    assert_eq!(manager.binary_source, Some(manager_binary));
    assert!(silent.launch_script.contains("$DIR/codey"));
    assert!(manager.launch_script.contains("$DIR/codey-manager"));
}

#[test]
fn windows_default_install_root_uses_known_folder_before_userprofile_desktop() {
    let strategy = default_install_root_strategy();

    if cfg!(windows) {
        assert_eq!(strategy, "windows-known-folder");
    } else if cfg!(target_os = "macos") {
        assert_eq!(strategy, "macos-applications");
    } else {
        assert_eq!(strategy, "user-dirs-desktop");
    }
}
