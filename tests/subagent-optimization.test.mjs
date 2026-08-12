import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("subagent optimization exposes dynamic model and reasoning controls", async () => {
  const [appSource, modelHookSource, featurePolicySource] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
  ]);
  const uiSource = `${appSource}\n${featurePolicySource}`;

  assert.match(uiSource, /checked=\{config\.subagentOptimization\}/);
  assert.match(
    uiSource,
    /onCheckedChange=\{\(checked\) =>\s*onSubagentOptimizationChange\(checked\)\s*\}/,
  );
  assert.match(uiSource, /aria-label="启用子代理协作优化"/);
  assert.match(uiSource, /aria-label="选择子代理模型"/);
  assert.match(uiSource, /aria-label="选择子代理思考深度"/);
  assert.match(
    uiSource,
    /subagentPolicyControlsDisabled \|\|\s*subagentModelOptions\.length === 0/,
  );
  assert.match(
    uiSource,
    /subagentPolicyControlsDisabled \|\|\s*subagentReasoningEfforts\.length === 0/,
  );
  assert.match(
    modelHookSource,
    /modelState\.officialModels\s*\.filter\(\(model\) => model\.supported\)/,
  );
  assert.match(modelHookSource, /\.\.\.modelState\.thirdPartyModels\s*\.map/);
  assert.doesNotMatch(modelHookSource, /subagentModelIds|subagentModelKeys/);
  assert.doesNotMatch(
    uiSource,
    /check-subagent-model|当前线路没有 Codex 子代理工具可用的模型/,
  );
  assert.doesNotMatch(uiSource, /仅接受 Sol \/ Terra/);
});

test("subagent defaults are hot-reloaded through the packaged app-server bridge", async () => {
  const [commandSource, cdpSource, rendererSource, sessionToolsSource] =
    await Promise.all([
      readFile(new URL("backend/src/commands.rs", root), "utf8"),
      readFile(new URL("backend/src/cdp.rs", root), "utf8"),
      readFile(new URL("public/renderer-inject.js", root), "utf8"),
      readFile(new URL("public/codey-inject.js", root), "utf8"),
    ]);

  const hotReloadIndex = commandSource.indexOf(
    "async fn hot_reload_runtime_subagent_defaults",
  );
  const cdpRefreshIndex = commandSource.indexOf(
    "cdp::refresh_subagent_defaults",
    hotReloadIndex,
  );
  const lifecycleLockIndex = commandSource.indexOf(
    "state.runtime_operation.lock().await",
    cdpRefreshIndex,
  );
  const leaseCommitIndex = commandSource.indexOf(
    "mark_runtime_subagent_defaults_applied",
    lifecycleLockIndex,
  );
  assert.ok(hotReloadIndex >= 0);
  assert.ok(cdpRefreshIndex > hotReloadIndex);
  assert.ok(lifecycleLockIndex > cdpRefreshIndex);
  assert.ok(leaseCommitIndex > lifecycleLockIndex);
  assert.match(cdpSource, /window\.__codeyApplySubagentDefaults/);
  assert.match(rendererSource, /window\.__codeyApplySubagentDefaults/);
  assert.match(rendererSource, /method: "config\/batchWrite"/);
  assert.match(rendererSource, /keyPath: "agents\.default_subagent_model"/);
  assert.match(
    rendererSource,
    /keyPath: "agents\.default_subagent_reasoning_effort"/,
  );
  assert.match(rendererSource, /reloadUserConfig: true/);
  assert.match(sessionToolsSource, /window\.__codeyLoadCodexSignalDispatcher/);
});

test("subagent optimization installs a root-only runtime wait gate", async () => {
  const [gateSource, configSource, mainSource] = await Promise.all([
    readFile(new URL("backend/src/subagent_gate.rs", root), "utf8"),
    readFile(new URL("backend/src/codex_config.rs", root), "utf8"),
    readFile(new URL("backend/src/main.rs", root), "utf8"),
  ]);

  assert.match(mainSource, /run_subagent_gate_hook_if_requested/);
  assert.match(configSource, /enable_subagent_gate_hooks\(doc, config_path\)/);
  assert.match(configSource, /toml_event: "PreToolUse"/);
  assert.match(configSource, /toml_event: "SubagentStart"/);
  assert.match(configSource, /toml_event: "SubagentStop"/);
  assert.match(configSource, /toml_event: "Stop"/);
  assert.match(configSource, /trusted_hash/);
  assert.match(gateSource, /nonempty\(input\.agent_id\.as_deref\(\)\)\.is_some\(\)/);
  assert.match(gateSource, /permissionDecision": "deny"/);
  assert.match(gateSource, /"decision": "block"/);
  assert.match(gateSource, /is_collaboration_tool/);
  assert.match(gateSource, /SubagentStop/);
});
