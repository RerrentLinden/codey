// Exercise the host independently of the desktop application and its UI runtime.
#[path = "../../../backend/src/codey_plugins/mod.rs"]
mod host;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Write, path::Path, process::Command};

fn package(path: &Path, library: &[u8], version: &str) {
    package_with_header(path, library, version, "x-plugin-demo");
}

fn package_with_header(path: &Path, library: &[u8], version: &str, header: &str) {
    let filename = if cfg!(target_os = "macos") {
        "libplugin.dylib"
    } else if cfg!(target_os = "windows") {
        "plugin.dll"
    } else {
        "libplugin.so"
    };
    let manifest = json!({
        "id":"dev.codey.header-demo","name":"Demo","version":version,
        "abiVersion":1,"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,
        "entry":format!("lib/{filename}"),"librarySha256":format!("{:x}",Sha256::digest(library)),
        "capabilities":["request.beforeSend"],"headerNames":[header],"configSchema":"config.schema.json"
    });
    let schema = json!({"type":"object","properties":{"value":{"type":"string","default":"hello-codey","minLength":1,"maxLength":128}},"required":["value"],"additionalProperties":false});
    let mut archive = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in [
        (
            "manifest.json".to_owned(),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        (
            "config.schema.json".to_owned(),
            serde_json::to_vec(&schema).unwrap(),
        ),
        (format!("lib/{filename}"), library.to_vec()),
    ] {
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes).unwrap();
    }
    archive.finish().unwrap();
}

fn uninstall_loaded_plugin(root: &Path, remove_data: bool) {
    #[cfg(windows)]
    let trash_before: std::collections::BTreeSet<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();

    let result = host::uninstall("dev.codey.header-demo", remove_data);
    #[cfg(not(windows))]
    {
        let _ = root;
        result.unwrap();
    }
    #[cfg(windows)]
    if let Err(error) = result {
        // The host pins native mappings until process exit, so Windows may keep
        // a loaded DLL in the trash after the uninstall state has committed.
        let trash: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| !trash_before.contains(path))
            .collect();
        assert_eq!(trash.len(), 1, "{error}");
        assert!(
            trash[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".trash-"),
            "{error}"
        );
        let expected = format!(
            "插件已卸载，但文件清理失败；请退出 Codey 后删除 {}: ",
            trash[0].canonicalize().unwrap().display()
        );
        assert!(error.starts_with(&expected), "{error}");
        assert!(error.ends_with("(os error 5)"), "{error}");
        let mut pending = trash;
        let mut retained_dll = false;
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else if entry.file_name() == "plugin.dll" {
                    retained_dll = true;
                }
            }
        }
        assert!(retained_dll, "{error}");
    }
    assert!(host::list().unwrap().plugins.is_empty());
}

#[test]
fn complete_native_plugin_lifecycle() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let temp = tempfile::tempdir().unwrap();
    // Compile a separate consumer project, with no Codey workspace membership.
    let project = temp.path().join("standalone-plugin");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::copy(
        workspace.join("examples/plugins/header-demo/src/lib.rs"),
        project.join("src/lib.rs"),
    )
    .unwrap();
    let sdk_path = serde_json::to_string(env!("CARGO_MANIFEST_DIR")).unwrap();
    fs::write(project.join("Cargo.toml"), format!(
        "[package]\nname = \"codey-plugin-header-demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[lib]\ncrate-type = [\"cdylib\"]\n[dependencies]\ncodey-plugin-sdk = {{ path = {sdk_path} }}\n[workspace]\n"
    )).unwrap();
    let target = temp.path().join("target");
    let build = Command::new("cargo")
        .args([
            "build",
            "--offline",
            "-p",
            "codey-plugin-header-demo",
            "--target-dir",
        ])
        .arg(&target)
        .arg("--config")
        .arg(format!("build.build-dir={:?}", temp.path().join("build")))
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let name = if cfg!(target_os = "macos") {
        "libcodey_plugin_header_demo.dylib"
    } else if cfg!(target_os = "windows") {
        "codey_plugin_header_demo.dll"
    } else {
        "libcodey_plugin_header_demo.so"
    };
    let library = fs::read(target.join("debug").join(name)).unwrap();
    let path = temp.path().join("demo.codey-plugin");
    package(&path, &library, "1.0.0");
    host::initialize(temp.path().join("host")).unwrap();
    let inspection = host::inspect(&path).unwrap();
    assert!(host::install(&path, "wrong-hash").is_err());
    assert!(host::list().unwrap().plugins.is_empty());
    let installed = host::install(&path, &inspection.sha256).unwrap();
    assert!(!installed.plugins[0].enabled);
    assert!(installed.plugins[0].config_ui.is_none());
    assert!(
        host::get_config_ui("dev.codey.header-demo")
            .unwrap()
            .is_none()
    );
    assert!(host::invoke("dev.codey.header-demo", "ping", json!(null)).is_err());
    // A failed consent persistence must not publish a newly loaded instance.
    let state_path = temp.path().join("host/state.json");
    let saved_state = temp.path().join("saved-state.json");
    fs::rename(&state_path, &saved_state).unwrap();
    fs::create_dir(&state_path).unwrap();
    assert!(host::set_enabled("dev.codey.header-demo", true).is_err());
    assert!(!host::has_request_plugins());
    assert!(host::dispatch_request_headers(&json!({}), &BTreeMap::new()).is_empty());
    assert!(host::invoke("dev.codey.header-demo", "ping", json!(null)).is_err());
    assert!(!host::list().unwrap().plugins[0].enabled);
    fs::remove_dir(&state_path).unwrap();
    fs::rename(&saved_state, &state_path).unwrap();
    host::set_enabled("dev.codey.header-demo", true).unwrap();
    let context = host::invoke("dev.codey.header-demo", "storage.context", json!(null)).unwrap();
    let plugin_dir = temp
        .path()
        .join("host/installed/dev.codey.header-demo")
        .canonicalize()
        .unwrap();
    assert_eq!(context["pluginDir"], plugin_dir.to_str().unwrap());
    assert_eq!(
        context["dataDir"],
        plugin_dir.join("data").to_str().unwrap()
    );
    assert_eq!(context["logDir"], plugin_dir.join("logs").to_str().unwrap());
    host::invoke(
        "dev.codey.header-demo",
        "storage.write",
        json!({"saved":42}),
    )
    .unwrap();
    assert!(plugin_dir.join("logs/plugin.log").exists());
    assert!(plugin_dir.join("logs/host.log").exists());
    assert!(host::has_request_plugins());
    assert_eq!(
        host::invoke("dev.codey.header-demo", "ping", json!({"echo":1})).unwrap()["value"],
        "hello-codey"
    );
    assert!(host::configure("dev.codey.header-demo", json!({"value":false})).is_err());
    let patched = host::dispatch_request_headers(
        &json!({"accountId":"account-handle"}),
        &BTreeMap::from([("authorization".into(), "Bearer secret".into())]),
    );
    assert_eq!(patched.len(), 1);
    assert_eq!(patched[0].name, "x-plugin-demo");
    assert_eq!(patched[0].value.as_deref(), Some("hello-codey"));
    let configured =
        host::configure("dev.codey.header-demo", json!({"value":"new-value"})).unwrap();
    assert!(configured.plugins[0].restart_required);
    assert_eq!(
        host::invoke("dev.codey.header-demo", "ping", json!(null)).unwrap()["value"],
        "hello-codey"
    );
    host::set_enabled("dev.codey.header-demo", false).unwrap();
    host::set_enabled("dev.codey.header-demo", true).unwrap();
    assert_eq!(
        host::invoke("dev.codey.header-demo", "storage.read", json!(null)).unwrap(),
        json!({"saved":42})
    );
    assert_eq!(
        host::invoke("dev.codey.header-demo", "storage.context", json!(null)).unwrap(),
        context
    );
    assert_eq!(
        host::invoke("dev.codey.header-demo", "ping", json!(null)).unwrap()["value"],
        "new-value"
    );
    package(&path, &library, "1.1.0");
    let inspection = host::inspect(&path).unwrap();
    let upgraded = host::install(&path, &inspection.sha256).unwrap();
    assert!(upgraded.plugins[0].enabled && upgraded.plugins[0].restart_required);
    assert!(host::uninstall("dev.codey.header-demo", false).is_err());
    host::set_enabled("dev.codey.header-demo", false).unwrap();
    #[cfg(unix)]
    for relative in ["installed", "installed/dev.codey.header-demo"] {
        let source = temp.path().join("host").join(relative);
        let outside = temp.path().join("outside-installed");
        fs::rename(&source, &outside).unwrap();
        let entries_before = fs::read_dir(&outside).unwrap().count();
        std::os::unix::fs::symlink(&outside, &source).unwrap();
        assert!(host::uninstall("dev.codey.header-demo", true).is_err());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), entries_before);
        assert_eq!(host::list().unwrap().plugins.len(), 1);
        fs::remove_file(&source).unwrap();
        fs::rename(&outside, &source).unwrap();
    }
    uninstall_loaded_plugin(&temp.path().join("host"), false);
    assert!(plugin_dir.join("data/example.json").exists());
    assert!(plugin_dir.join("logs/plugin.log").exists());
    assert!(!plugin_dir.join("versions").exists());
    assert!(host::list().unwrap().plugins.is_empty());
    let restored = host::install(&path, &inspection.sha256).unwrap();
    assert_eq!(restored.plugins[0].config["value"], "new-value");
    host::set_enabled("dev.codey.header-demo", true).unwrap();
    assert_eq!(
        host::invoke("dev.codey.header-demo", "storage.read", json!(null)).unwrap(),
        json!({"saved":42})
    );
    host::set_enabled("dev.codey.header-demo", false).unwrap();
    uninstall_loaded_plugin(&temp.path().join("host"), true);
    assert!(!plugin_dir.exists());
    let reset = host::install(&path, &inspection.sha256).unwrap();
    assert_eq!(reset.plugins[0].config["value"], "hello-codey");
    package_with_header(&path, &library, "1.2.0", "x-different-header");
    let inspection = host::inspect(&path).unwrap();
    host::install(&path, &inspection.sha256).unwrap();
    host::set_enabled("dev.codey.header-demo", true).unwrap();
    assert!(host::dispatch_request_headers(&json!({}), &BTreeMap::new()).is_empty());
    assert!(!host::has_request_plugins());
    let failed = host::list().unwrap();
    assert_eq!(failed.plugins[0].status, "error");
    assert!(failed.plugins[0].last_error.is_some());
    package(&path, &library, "1.3.0");
    let inspection = host::inspect(&path).unwrap();
    host::install(&path, &inspection.sha256).unwrap();
    host::configure("dev.codey.header-demo", json!({"value":"recovered"})).unwrap();
    host::set_enabled("dev.codey.header-demo", true).unwrap();
    // Build a library exporting only the original entry, proving host fallback
    // passes the original config rather than the new context envelope.
    host::set_enabled("dev.codey.header-demo", false).unwrap();
    let source_path = project.join("src/lib.rs");
    let source = fs::read_to_string(&source_path).unwrap().replace(
        "codey_plugin_sdk::export_plugin!(HeaderDemo);",
        r#"#[unsafe(no_mangle)]
        pub extern "C" fn codey_plugin_entry_v1() -> *const codey_plugin_sdk::PluginApiV1 {
            static API: codey_plugin_sdk::PluginApiV1 = codey_plugin_sdk::PluginApiV1 {
                abi_version: 1,
                struct_size: std::mem::size_of::<codey_plugin_sdk::PluginApiV1>() as u32,
                create: codey_plugin_sdk::create::<HeaderDemo>,
                invoke: codey_plugin_sdk::invoke::<HeaderDemo>,
                destroy: codey_plugin_sdk::destroy::<HeaderDemo>,
                free_buffer: codey_plugin_sdk::free_buffer,
            };
            &API
        }"#,
    );
    fs::write(&source_path, source).unwrap();
    let build = Command::new("cargo")
        .args(["build", "--offline", "--target-dir"])
        .arg(&target)
        .arg("--config")
        .arg(format!("build.build-dir={:?}", temp.path().join("build")))
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let legacy = fs::read(target.join("debug").join(name)).unwrap();
    package(&path, &legacy, "1.4.0");
    let inspection = host::inspect(&path).unwrap();
    host::install(&path, &inspection.sha256).unwrap();
    host::set_enabled("dev.codey.header-demo", true).unwrap();
    assert_eq!(
        host::invoke("dev.codey.header-demo", "ping", json!(null)).unwrap()["value"],
        "recovered"
    );
    assert_eq!(
        host::invoke("dev.codey.header-demo", "storage.context", json!(null)).unwrap(),
        Value::Null
    );
    host::shutdown();
    assert!(!host::has_request_plugins());
    assert!(host::invoke("dev.codey.header-demo", "ping", json!(null)).is_err());
    assert!(host::set_enabled("dev.codey.header-demo", true).is_err());
}

#[test]
fn inspection_does_not_execute_libraries() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("fake.codey-plugin");
    package(&path, b"this is not a native library", "1.0.0");
    assert!(host::inspect(&path).is_ok());
}

#[test]
fn archive_traversal_duplicates_and_symlinks_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for (index, names) in [
        vec!["../escape"],
        vec!["manifest.json", "MANIFEST.JSON"],
        vec!["dir", "dir/file"],
    ]
    .iter()
    .enumerate()
    {
        let path = temp.path().join(format!("bad{index}.codey-plugin"));
        let mut archive = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        for name in names {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"{}").unwrap();
        }
        archive.finish().unwrap();
        assert!(host::inspect(&path).is_err());
    }
    let path = temp.path().join("symlink.codey-plugin");
    let mut archive = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    archive
        .add_symlink(
            "linked",
            "/tmp/outside",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.finish().unwrap();
    assert!(host::inspect(&path).is_err());
}
