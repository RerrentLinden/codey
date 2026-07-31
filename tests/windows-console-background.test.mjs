import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const normalizeLineEndings = (source) => source.replace(/\r\n/g, "\n");

test("Windows builds Codey as a GUI process without a console window", async () => {
  const [main, library, manifest] = await Promise.all([
    readFile(new URL("../backend/src/main.rs", import.meta.url), "utf8")
      .then(normalizeLineEndings),
    readFile(new URL("../backend/src/lib.rs", import.meta.url), "utf8")
      .then(normalizeLineEndings),
    readFile(new URL("../backend/Cargo.toml", import.meta.url), "utf8")
      .then(normalizeLineEndings),
  ]);

  assert.match(
    main,
    /^#!\[cfg_attr\(target_os = "windows", windows_subsystem = "windows"\)\]/,
  );
  assert.doesNotMatch(library, /hide_exclusive_windows_console|ShowWindow|GetConsoleWindow/);
  assert.doesNotMatch(manifest, /Win32_System_Console|Win32_UI_WindowsAndMessaging/);
});

test("Windows startup failures are visible and terminate the background process", async () => {
  const library = normalizeLineEndings(
    await readFile(new URL("../backend/src/lib.rs", import.meta.url), "utf8"),
  );
  const failureStart = library.indexOf(
    "if let Err(error) = commands::launch_codey_runtime(&state).await",
  );
  const shutdownWait = library.indexOf("let shutdown_reason = tokio::select!");

  assert.notEqual(failureStart, -1);
  assert.notEqual(shutdownWait, -1);
  assert.ok(failureStart < shutdownWait);

  const failureBranch = library.slice(failureStart, shutdownWait);
  assert.match(failureBranch, /stop_runtime_with_retry\(&state\)\.await/);
  assert.match(failureBranch, /show_initial_startup_failure\(&error\)\.await/);
  assert.match(failureBranch, /return Err\(/);

  const cleanupHelper = library.slice(
    library.indexOf("async fn stop_runtime_with_retry"),
    library.indexOf("fn initial_startup_failure_error"),
  );
  assert.match(cleanupHelper, /stop_codey_runtime\(state\)\.await/);
  assert.match(cleanupHelper, /tokio::time::sleep/);
  assert.equal(cleanupHelper.match(/stop_codey_runtime\(state\)/g)?.length, 2);

  assert.match(
    library,
    /rfd::MessageDialog::new\(\)[\s\S]*?MessageLevel::Error[\s\S]*?MessageButtons::Ok[\s\S]*?\.show\(\)/,
  );
  assert.match(library, /tokio::task::spawn_blocking/);
  assert.match(library, /\.set_title\("Codey 启动失败"\)/);
  assert.match(library, /Codey 将退出。处理上述问题后，请重新启动 Codey。/);
});

test("Windows background helpers never create console windows", async () => {
  const [launcher, processCleanup] = await Promise.all([
    readFile(new URL("../backend/src/launcher.rs", import.meta.url), "utf8")
      .then(normalizeLineEndings),
    readFile(new URL("../backend/src/process_cleanup.rs", import.meta.url), "utf8")
      .then(normalizeLineEndings),
  ]);

  assert.equal(
    launcher.match(
      /creation_flags\(codey_runtime_core::windows_create_no_window\(\)\)/g,
    )?.length,
    2,
  );
  assert.doesNotMatch(processCleanup, /Command::new\("taskkill"\)/);
  assert.match(
    processCleanup,
    /codey_runtime_core::windows_terminate_process_if_matches/,
  );
});

test("Windows packaged Codex exit uses an OS process wait instead of polling snapshots", async () => {
  const [launcher, coreLauncher] = await Promise.all([
    readFile(new URL("../backend/src/launcher.rs", import.meta.url), "utf8")
      .then(normalizeLineEndings),
    readFile(
      new URL(
        "../vendor/CodeyRuntime/crates/codey-runtime-core/src/launcher.rs",
        import.meta.url,
      ),
      "utf8",
    ).then(normalizeLineEndings),
  ]);
  const watcher = launcher.slice(
    launcher.indexOf("#[cfg(windows)]\nfn spawn_codex_exit_watcher"),
    launcher.indexOf("struct SpawnedCodex"),
  );

  assert.match(
    watcher,
    /codey_runtime_core::launcher::wait_for_windows_process_id\(process_id\)/,
  );
  assert.doesNotMatch(watcher, /missing_streak/);
  assert.match(
    coreLauncher,
    /pub async fn wait_for_windows_process_id\(process_id: u32\)/,
  );
  assert.match(coreLauncher, /WaitForSingleObject\(handle, INFINITE\)/);
});

test("Windows updates survive shutdown through the native helper", async () => {
  const [main, updates, updateHelper] = await Promise.all([
    readFile(new URL("../backend/src/main.rs", import.meta.url), "utf8").then(
      normalizeLineEndings,
    ),
    readFile(
      new URL("../backend/src/commands/updates.rs", import.meta.url),
      "utf8",
    ).then(normalizeLineEndings),
    readFile(
      new URL("../backend/src/update_helper.rs", import.meta.url),
      "utf8",
    ).then(normalizeLineEndings),
  ]);

  assert.match(
    main,
    /run_update_helper_if_requested\(\)\?[\s\S]*Builder::new_multi_thread/,
  );
  assert.match(
    updates,
    /crate::update_helper::spawn_update_installer\(update_path\)/,
  );
  assert.doesNotMatch(updates, /powershell\.exe|install-codey-update\.ps1/i);
  assert.match(
    updateHelper,
    /std::fs::copy\(&executable, &helper_path\)[\s\S]*Command::new\(&helper_path\)/,
  );
  assert.match(
    updateHelper,
    /let install_result = install_windows_update[\s\S]*let restart_result = restart_codey/,
  );
  assert.match(updateHelper, /raw_arg\(nsis_install_directory_argument/);
});
