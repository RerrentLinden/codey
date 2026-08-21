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
  assert.match(cdp, /enum InjectionScriptVisibility/);
  assert.match(
    cdp,
    /"renderer-controls"[\s\S]*?Internal,\s*All,/,
  );
  assert.match(cdp, /visibility: descriptor\.visibility\.as_str\(\)\.to_string\(\)/);
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
  assert.match(runtimeCommands, /"subagentOptimizationActive"/);
  assert.match(runtimeCommands, /"notificationChannelsActive"/);
  assert.match(runtimeCommands, /"activeNotificationChannelCount"/);
  assert.match(runtimeCommands, /"traceLogWriteProtectionActive"/);
  assert.match(runtimeCommands, /"crashpadDiskProtectionActive"/);
  assert.match(runtimeCommands, /waiting_watcher_task[\s\S]*?is_finished/);
  assert.match(
    launcher,
    /crashpad_pending_protection_active[\s\S]*?target_os = "macos"[\s\S]*?crashpad_guard_task[\s\S]*?is_finished/,
  );
  assert.match(
    commands,
    /"refresh_injection_status"\s*=>\s*refresh_injection_status/,
  );
  assert.match(runtimeHook, /refreshInjectionStatus: refreshesInjectionStatus/);
  assert.match(runtimeHook, /runtimeStatusFlightRef/);
  assert.equal(
    runtimeHook.match(/invoke<RuntimeStatus>\("runtime_status"/g)?.length,
    1,
  );
  assert.match(runtimeHook, /codey-injection-status-changed/);
  assert.match(runtimeHook, /codey-settings-opened/);
  assert.match(
    overlay,
    /window\.dispatchEvent\(new CustomEvent\(SETTINGS_OPENED_EVENT\)\)/,
  );
  assert.match(overlay, /let visible = false/);
  assert.match(overlay, /const open = \(\) => \{\s*if \(visible\) return;/);
  assert.match(runtimeHook, /type StatusPollTask/);
  assert.match(runtimeHook, /clear: \(\) => void/);
  assert.match(runtimeHook, /requestGenerationRef/);
  assert.match(runtimeHook, /mountedRef\.current &&/);
  assert.match(runtimeHook, /activeRef\.current &&/);
  assert.match(runtimeHook, /const queuedGeneration = requestGenerationRef\.current/);
  assert.match(runtimeHook, /requestCanCommit\(queuedGeneration\)/);
  assert.match(runtimeHook, /statusPollScheduler\.clear\(\)/);
  assert.match(app, /active: !embedded \|\| modalVisible/);
  assert.match(runtimeHook, /if \(!activeRef\.current\) return/);
  assert.match(
    runtimeHook,
    /const STATUS_POLL_MAX_CONSECUTIVE_ERRORS = 5/,
  );
  assert.match(runtimeHook, /STATUS_POLL_MAX_DURATION_MS/);
  assert.match(runtimeHook, /INJECTION_PROBE_MAX_DURATION_MS = 60_000/);
  assert.match(
    runtimeHook,
    /script\.source === "builtin" && script\.status === "executed"/,
  );
  assert.doesNotMatch(runtimeHook, /script\.id === "git-request-guard"/);
  assert.doesNotMatch(runtimeHook, /script\.id === "windows-wmi-sampler"/);
  assert.match(runtimeHook, /dueTasks\.some\(\(task\) => task\.refreshesInjectionStatus\)/);
  assert.match(cdp, /completedEntry\.status === \\"pending\\"/);
  assert.match(pluginFix, /markPluginBridgeEffective/);
  assert.match(pluginFix, /entry\.status = "effective"/);
  assert.match(pluginFix, /codey-injection-status-changed/);
  assert.doesNotMatch(pluginFix, /codey-plugin-marketplace-refresh/);
  assert.doesNotMatch(pluginFix, /__codeyPluginCacheVersion/);
  assert.match(
    types,
    /visibility:\s*"feature"\s*\|\s*"internal"/,
  );
  assert.match(
    types,
    /status:\s*"effective"\s*\|\s*"executed"\s*\|\s*"inactive"\s*\|\s*"failed"\s*\|\s*"unknown"/,
  );
  assert.match(types, /subagentOptimizationActive\?: boolean/);
  assert.match(types, /notificationChannelsActive\?: boolean/);
  assert.match(types, /activeNotificationChannelCount\?: number/);
  assert.match(types, /traceLogWriteProtectionActive\?: boolean/);
  assert.match(types, /crashpadDiskProtectionActive\?: boolean/);
  assert.match(sections, /已生效功能/);
  assert.match(
    sections,
    /injectionScripts\.filter\(\(script\) => script\.visibility === "feature"\)/,
  );
  assert.match(
    sections,
    /userFacingInjectionScripts[\s\S]*?script\.status === "effective"/,
  );
  assert.doesNotMatch(sections, /INTERNAL_OPTIMIZATION_SCRIPT_IDS/);
  assert.match(sections, /name: "FastCtx 上下文加速"/);
  assert.match(sections, /status\.fastContextToolsActive === true/);
  assert.match(sections, /fastContextToolsStatus\.userConfigured/);
  assert.match(sections, /name: "子代理优化"/);
  assert.match(sections, /status\.subagentOptimizationActive === true/);
  assert.match(sections, /name: "消息通知"/);
  assert.match(sections, /status\.notificationChannelsActive === true/);
  assert.match(sections, /activeNotificationChannelCount/);
  assert.match(sections, /name: "写盘保护"/);
  assert.match(sections, /status\.traceLogWriteProtectionActive === true/);
  assert.match(sections, /status\.crashpadDiskProtectionActive === true/);
  assert.match(sections, /Trace 日志与 Crashpad 磁盘保护均已生效/);
  assert.doesNotMatch(sections, /渲染器控制/);
  assert.match(app, /await refreshStatus\(\)\.catch/);
  assert.match(commands, /trace_log_write_protection_active\.store/);
  assert.match(sections, /enabledOptimizationFeatures\.map/);
  assert.match(
    sections,
    /enabledFeatureCount: enabledOptimizationFeatures\.length/,
  );
  assert.match(sections, /已启用 \{item\.enabledFeatureCount\} 项/);
  assert.doesNotMatch(sections, /injectionScripts\.map/);
  assert.doesNotMatch(sections, /effectiveInjectionScripts\.map/);
  assert.doesNotMatch(sections, /injection-script-state/);
  assert.doesNotMatch(sections, /injection-status-summary/);
  assert.doesNotMatch(sections, /id: "opt-fastctx"/);
  assert.doesNotMatch(sections, /id: "opt-injection"/);
  assert.doesNotMatch(sections, /id: "opt-patch"/);
  assert.match(sections, /Codex 启动后将在这里显示已生效功能/);
  assert.doesNotMatch(sections, /setInterval/);
  assert.match(styles, /text-wrap:\s*balance/);
  assert.match(styles, /word-break:\s*normal/);
  assert.match(cdp, /guard\.ensureInstalled\?\.\(\)/);
  assert.match(cdp, /snapshot\.mainProcessProtected === true/);
  assert.match(cdp, /Object\.prototype\.hasOwnProperty\.call/);
  assert.match(cdp, /for \(const delay of \[50, 200, 750\]\)/);
});
