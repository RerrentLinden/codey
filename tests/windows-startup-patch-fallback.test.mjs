import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("Windows retries startup without the hard patch after safely cleaning its paused process", async () => {
  const launcher = (await readFile(
    new URL("../backend/src/launcher.rs", import.meta.url),
    "utf8",
  )).replace(/\r\n/g, "\n");
  const windowsSpawn = launcher.slice(
    launcher.indexOf("#[cfg(windows)]\n    {", launcher.indexOf("async fn spawn_codex")),
    launcher.indexOf("#[cfg(target_os = \"macos\")]", launcher.indexOf("async fn spawn_codex")),
  );
  const cleanup = launcher.slice(
    launcher.indexOf("async fn stop_windows_spawned_codex"),
    launcher.indexOf("#[cfg(target_os = \"macos\")]\nfn build_fresh_macos_open_command"),
  );

  assert.match(windowsSpawn, /stop_windows_spawned_codex\(&mut spawned, app_dir\)\.await/);
  assert.match(windowsSpawn, /spawn_windows_codex\(app_dir, debug_port, &runtime_arguments\)\.await/);
  assert.match(windowsSpawn, /fallback\.performance_status = "degraded"/);
  assert.match(cleanup, /terminate_windows_codex_processes\(app_dir, process_id\)\.await/);
  assert.match(cleanup, /-> Result<\(\)>/);
});
