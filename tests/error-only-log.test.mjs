import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), "utf8");
}

test("startup keeps the original loading page without progress tracing", async () => {
  const [app, types, main, styles] = await Promise.all([
    source("src/App.tsx"),
    source("src/App.types.ts"),
    source("src/main.tsx"),
    source("src/styles.css"),
  ]);
  const frontend = [app, types, main, styles].join("\n");

  assert.match(app, /if \(!config \|\| !provider\)/);
  assert.match(app, /正在载入 Codey/);
  assert.match(app, /<p>\{notice\.text\}<\/p>/);
  assert.doesNotMatch(frontend, /StartupLoading|startupProgress|startup_progress/);
  await assert.rejects(access(new URL("src/StartupLoading.tsx", root)));
});

test("error log is failure-only, daily, structured, and cross-process serialized", async () => {
  const [
    errorLog,
    launcher,
    cdp,
    commands,
    runtimeCommands,
    lib,
    startupPatch,
    renderer,
  ] =
    await Promise.all([
      source("backend/src/error_log.rs"),
      source("backend/src/launcher.rs"),
      source("backend/src/cdp.rs"),
      source("backend/src/commands.rs"),
      source("backend/src/commands/runtime.rs"),
      source("backend/src/lib.rs"),
      source("backend/src/codex_startup_patch.rs"),
      source("public/renderer-inject.js"),
    ]);

  assert.match(errorLog, /codey-errors\.log/);
  assert.match(errorLog, /file_is_from_different_day/);
  assert.match(errorLog, /truncate\(true\)/);
  assert.match(errorLog, /lock_exclusive/);
  assert.match(errorLog, /--codey-record-error/);
  assert.match(errorLog, /repair_incomplete_tail/);
  assert.match(errorLog, /struct FailureMetadata/);
  for (const field of [
    "stage",
    "duration_ms",
    "attempts",
    "timeout_ms",
    "recoverable",
  ]) {
    assert.match(errorLog, new RegExp(`${field}: Option`));
  }
  assert.match(errorLog, /record_renderer_failure/);
  assert.match(errorLog, /renderer_failure_descriptor/);
  assert.match(errorLog, /renderer_failure_context/);
  assert.match(commands, /"\/diagnostics\/error"/);
  assert.match(cdp, /timeout_at\(\s*deadline/);

  for (const operation of [
    "inject_cdp_bridge",
    "reinject_cdp_bridge",
    "install_startup_patch",
    "configure_codex_pet_slim",
    "apply_runtime_provider_config",
    "configure_trace_log_guard",
    "restore_runtime_provider_config",
  ]) {
    assert.match(launcher, new RegExp(`"${operation}"`));
  }
  assert.match(cdp, /"injection_script_failed"/);
  assert.match(cdp, /"injection_status_failed"/);
  assert.match(runtimeCommands, /"runtime_restart_failed"/);
  assert.match(commands, /"repair_plugin_marketplace"/);
  assert.match(runtimeCommands, /"launch_codey_runtime"/);
  assert.doesNotMatch(lib, /"auto_launch_codey_runtime"/);
  assert.match(startupPatch, /recordCodeyPatchFailure/);
  assert.match(startupPatch, /spawnSync/);
  assert.match(startupPatch, /startup\.renderer_asset_patch/);
  assert.match(startupPatch, /electronVersion/);
  assert.match(renderer, /"startup_stalled"/);
  assert.match(renderer, /"wait_for_electron_bridge"/);
  assert.match(renderer, /"wait_for_codex_host_shell"/);
  assert.match(renderer, /hostShellReadyTimeoutMs = 20_000/);
  assert.match(renderer, /reportRendererFailure/);
  assert.doesNotMatch(renderer, /error\.stack/);
});
