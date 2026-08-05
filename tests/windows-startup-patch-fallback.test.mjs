import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function loadWindowsStartupSource() {
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
  return { cleanup, windowsSpawn };
}

test("Windows pet slim mode degrades to compatible startup after safely cleaning its paused process", async () => {
  const { cleanup, windowsSpawn } = await loadWindowsStartupSource();
  const cleanupCall = windowsSpawn.indexOf(
    "stop_windows_spawned_codex(&mut spawned, app_dir).await",
  );
  const compatibleRestart = windowsSpawn.indexOf(
    "match spawn_windows_codex(app_dir, debug_port, &runtime_arguments).await",
  );

  assert.ok(cleanupCall >= 0);
  assert.ok(compatibleRestart > cleanupCall);
  assert.doesNotMatch(windowsSpawn, /宠物精简依赖启动硬补丁/);
  assert.doesNotMatch(windowsSpawn, /未以兼容模式重启/);
  assert.match(
    windowsSpawn,
    /if patch_options\.disable_pet \{[\s\S]*宠物精简启动补丁未能确认生效/,
  );
  assert.match(windowsSpawn, /fallback\.performance_status = "degraded"/);
  assert.match(windowsSpawn, /本次宠物精简失败，可能存在额外 Renderer/);
  assert.match(windowsSpawn, /"petSlimRequested": patch_options\.disable_pet/);
  assert.match(cleanup, /terminate_windows_codex_processes\(app_dir, process_id\)\.await/);
  assert.match(cleanup, /-> Result<\(\)>/);
});

test("Windows keeps generic degraded detail when pet slim mode is not required", async () => {
  const { windowsSpawn } = await loadWindowsStartupSource();

  assert.match(windowsSpawn, /if patch_options\.disable_pet/);
  assert.match(windowsSpawn, /spawn_windows_codex\(app_dir, debug_port, &runtime_arguments\)\.await/);
  assert.match(windowsSpawn, /fallback\.performance_status = "degraded"/);
  assert.match(windowsSpawn, /启动补丁未能确认生效，已自动以兼容模式启动/);
});
