import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { readAppStyles } from "./helpers/read-app-styles.mjs";

const root = new URL("../", import.meta.url);

test("script injection diagnostics report runtime evidence without continuous polling", async () => {
  const [
    cdp,
    launcher,
    commands,
    runtimeCommands,
    app,
    runtimeHook,
    overlay,
    types,
    sections,
    pluginFix,
    styles,
  ] = await Promise.all([
    readFile(new URL("backend/src/cdp.rs", root), "utf8"),
    readFile(new URL("backend/src/launcher.rs", root), "utf8"),
    readFile(new URL("backend/src/commands.rs", root), "utf8"),
    readFile(new URL("backend/src/commands/runtime.rs", root), "utf8"),
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/useRuntimeStatus.ts", root), "utf8"),
    readFile(new URL("src/overlay.tsx", root), "utf8"),
    readFile(new URL("src/App.types.ts", root), "utf8"),
    readFile(new URL("src/OperationsPanel.tsx", root), "utf8"),
    readFile(new URL("public/plugin-marketplace-fix.js", root), "utf8"),
    readAppStyles(root),
  ]);

  assert.match(cdp, /window\.__codeyInjectionStatus/);
  assert.match(cdp, /MAX_INJECTION_ERROR_CHARS:\s*usize\s*=\s*500/);
  assert.match(cdp, /read_injection_statuses\(&websocket_url, scripts\)/);
  assert.match(
    launcher,
    /injection_statuses:\s*Arc<RwLock<Arc<\[cdp::InjectionScriptStatus\]>>>/,
  );
  assert.match(launcher, /watchdog_statuses\.write\(\)\.await/);
  assert.match(
    runtimeCommands,
    /runtime\.injection_statuses\.read\(\)\.await\.clone\(\)/,
  );
  assert.match(runtimeCommands, /"injectionScripts"/);
  assert.match(
    commands,
    /"refresh_injection_status"\s*=>\s*refresh_injection_status/,
  );
  assert.match(runtimeHook, /invoke\("refresh_injection_status"\)/);
  assert.match(runtimeHook, /if \(shouldRefreshInjectionStatus\)/);
  assert.match(runtimeHook, /codey-injection-status-changed/);
  assert.match(runtimeHook, /codey-settings-opened/);
  assert.match(
    overlay,
    /window\.dispatchEvent\(new CustomEvent\(SETTINGS_OPENED_EVENT\)\)/,
  );
  assert.match(runtimeHook, /injectionStatusRefreshRef/);
  assert.match(app, /active: !embedded \|\| modalVisible/);
  assert.match(runtimeHook, /if \(!activeRef\.current\) return/);
  assert.match(
    runtimeHook,
    /const STATUS_POLL_MAX_CONSECUTIVE_ERRORS = 5/,
  );
  assert.match(runtimeHook, /STATUS_POLL_MAX_DURATION_MS/);
  assert.match(runtimeHook, /GIT_GUARD_PROBE_MAX_DURATION_MS = 30_000/);
  assert.match(runtimeHook, /WMI_SAMPLER_PROBE_MAX_DURATION_MS = 60_000/);
  assert.match(runtimeHook, /gitGuardStatus !== "executed"/);
  assert.match(runtimeHook, /wmiSamplerStatus !== "executed"/);
  assert.match(runtimeHook, /const next = await refreshInjectionStatus\(\)/);
  assert.match(cdp, /completedEntry\.status === \\"pending\\"/);
  assert.match(pluginFix, /markPluginBridgeEffective/);
  assert.match(pluginFix, /entry\.status = "effective"/);
  assert.match(pluginFix, /codey-injection-status-changed/);
  assert.match(
    types,
    /status:\s*"effective"\s*\|\s*"executed"\s*\|\s*"failed"\s*\|\s*"unknown"/,
  );
  assert.match(sections, /脚本生效状态/);
  assert.match(sections, /生效探针通过/);
  assert.match(sections, /脚本已执行，但没有生效证据/);
  assert.match(sections, /Codex 启动后将记录每个脚本的注入结果/);
  assert.match(
    sections,
    /性能策略已生效：WMI 采样保护与泄漏回收已确认/,
  );
  assert.doesNotMatch(sections, /setInterval/);
  assert.match(styles, /text-wrap:\s*balance/);
  assert.match(styles, /word-break:\s*normal/);
  assert.match(cdp, /guard\.ensureInstalled\?\.\(\)/);
  assert.match(cdp, /snapshot\.mainProcessProtected === true/);
  assert.match(cdp, /Object\.prototype\.hasOwnProperty\.call/);
  assert.match(cdp, /for \(const delay of \[50, 200, 750\]\)/);
});
