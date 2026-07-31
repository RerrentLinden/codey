import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const normalizeLineEndings = (source) => source.replace(/\r\n/g, "\n");

test("every shutdown path reaps Codex and Codey process trees", async () => {
  const [library, launcher, commands, cleanup, processTree] = await Promise.all([
    readFile(new URL("../backend/src/lib.rs", import.meta.url), "utf8").then(
      normalizeLineEndings,
    ),
    readFile(
      new URL("../backend/src/launcher.rs", import.meta.url),
      "utf8",
    ).then(normalizeLineEndings),
    readFile(
      new URL("../backend/src/commands/runtime.rs", import.meta.url),
      "utf8",
    ).then(normalizeLineEndings),
    readFile(
      new URL("../backend/src/process_cleanup.rs", import.meta.url),
      "utf8",
    ).then(normalizeLineEndings),
    readFile(
      new URL("../backend/src/process_tree.rs", import.meta.url),
      "utf8",
    ).then(normalizeLineEndings),
  ]);

  const finalShutdown = library.slice(
    library.indexOf("let shutdown_reason = tokio::select!"),
    library.indexOf("cleanup.map_err"),
  );
  assert.match(finalShutdown, /stop_runtime_with_retry\(&state\)\.await/);
  assert.match(finalShutdown, /terminate_other_codey_processes\(\)\.await/);
  assert.doesNotMatch(
    finalShutdown,
    /if shutdown_reason == ShutdownReason::CodexExited/,
  );

  const stopWithRetry = library.slice(
    library.indexOf("async fn stop_runtime_with_retry"),
    library.indexOf("fn initial_startup_failure_error"),
  );
  assert.equal(stopWithRetry.match(/stop_codey_runtime\(state\)/g)?.length, 2);
  assert.match(stopWithRetry, /tokio::time::sleep/);

  const runtimeStop = launcher.slice(
    launcher.indexOf("pub async fn stop(&self)"),
    launcher.indexOf("fn watchdog_should_reinject"),
  );
  assert.match(runtimeStop, /terminate_unix_codex_processes/);
  assert.match(runtimeStop, /terminate_windows_codex_processes/);
  assert.match(launcher, /windows_terminate_process_if_matches/);
  assert.doesNotMatch(runtimeStop, /if !self\.codex_exited/);
  assert.match(launcher, /child_command\.process_group\(0\)/);
  assert.match(
    launcher,
    /let poll_delays = \[\s*Duration::from_millis\(100\),\s*Duration::from_millis\(200\),\s*Duration::from_millis\(350\),\s*Duration::from_millis\(550\),\s*Duration::from_millis\(800\),\s*\]/,
  );
  assert.match(cleanup, /process_ids_with_descendants/);
  assert.match(processTree, /matching_process_ids/);
  assert.match(cleanup, /windows_process_paths_equal/);
  assert.match(cleanup, /windows_terminate_process_if_matches/);
  assert.doesNotMatch(cleanup, /pgrep|taskkill/);
  assert.match(processTree, /identity\.start_time == process\.start_time/);

  const stopCommand = commands.slice(
    commands.indexOf("async fn stop_codey_runtime_locked"),
    commands.indexOf("#[cfg(test)]", commands.indexOf("pub async fn stop_codey_runtime")),
  );
  assert.match(stopCommand, /state\.runtime\.lock\(\)\.await\.take\(\)/);
  assert.match(stopCommand, /\*state\.runtime\.lock\(\)\.await = Some\(runtime\)/);
  assert.match(stopCommand, /runtime_operation\.lock\(\)\.await/);
});
