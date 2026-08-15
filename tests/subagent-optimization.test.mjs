import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("subagent optimization exposes per-task model and reasoning controls", async () => {
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
  for (const role of [
    "codey_quick_scan",
    "codey_deep_research",
    "codey_visual_analysis",
    "codey_worker",
    "codey_visual_worker",
    "default",
  ]) {
    assert.match(uiSource, new RegExp(`id: "${role}"`));
  }
  for (const label of [
    "快速定位",
    "深度检索",
    "视觉分析",
    "代码实施",
    "视觉实施",
    "通用兜底",
  ]) {
    assert.match(uiSource, new RegExp(`name: "${label}"`));
  }
  assert.match(uiSource, /config\.subagentOptimization \? \(/);
  assert.match(uiSource, /className="subagent-task-help"/);
  assert.match(uiSource, /content=\{task\.description\}/);
  assert.match(uiSource, /aria-labelledby=\{`\$\{task\.id\}-model-label`\}/);
  assert.match(uiSource, /aria-labelledby=\{`\$\{task\.id\}-effort-label`\}/);
  assert.match(uiSource, /config\.subagentRoles\[task\.id\]/);
  assert.match(uiSource, /\[task\.id\]: \{ model, reasoningEffort \}/);
  assert.match(
    uiSource,
    /subagentPolicyControlsDisabled \|\|\s*subagentModelOptions\.length === 0/,
  );
  assert.match(
    uiSource,
    /subagentPolicyControlsDisabled \|\|\s*reasoningEfforts\.length === 0/,
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

test("per-task subagent files are composed at startup and hot-refreshed after save", async () => {
  const [commandSource, configSource, launcherSource, rendererSource, vendorRendererSource] = await Promise.all([
    readFile(new URL("backend/src/commands.rs", root), "utf8"),
    readFile(new URL("backend/src/codex_config.rs", root), "utf8"),
    readFile(new URL("backend/src/launcher.rs", root), "utf8"),
    readFile(new URL("public/renderer-inject.js", root), "utf8"),
    readFile(new URL("vendor/CodeyRuntime/assets/inject/renderer-inject.js", root), "utf8"),
  ]);

  assert.match(commandSource, /hot_reload_runtime_subagent_config/);
  assert.match(commandSource, /refresh_runtime_subagent_roles/);
  assert.doesNotMatch(commandSource, /let refresh_subagent_defaults = false/);
  assert.match(configSource, /fn prepare_runtime_agent_files/);
  assert.match(configSource, /document\["model"\] = value\(model\)/);
  assert.match(
    configSource,
    /document\["model_reasoning_effort"\] = value\(&reasoning_effort\)/,
  );
  assert.match(configSource, /register_runtime_agents/);
  assert.match(configSource, /agents\.\{\}\.config_file/);
  assert.match(configSource, /pub fn refresh_runtime_subagent_roles/);
  assert.match(configSource, /verify_runtime_agent_files/);
  assert.match(configSource, /restore_runtime_agent_files_and_lease/);
  assert.match(launcherSource, /subagent_roles: Some\(&subagent_roles\)/);
  assert.match(launcherSource, /supports_subagent_config_hot_reload/);
  assert.doesNotMatch(rendererSource, /__codeyApplySubagentDefaults/);
  assert.doesNotMatch(vendorRendererSource, /__codeyApplySubagentDefaults/);
  assert.doesNotMatch(vendorRendererSource, /patchAppServerSubagentRequestParams/);
});

test("subagent optimization installs root waiting and nested-spawn runtime gates", async () => {
  const [gateSource, configSource, mainSource] = await Promise.all([
    readFile(new URL("backend/src/subagent_gate.rs", root), "utf8"),
    readFile(new URL("backend/src/codex_config.rs", root), "utf8"),
    readFile(new URL("backend/src/main.rs", root), "utf8"),
  ]);

  assert.match(mainSource, /run_subagent_gate_hook_if_requested/);
  assert.match(configSource, /enable_subagent_gate_hooks\(doc, config_path\)/);
  assert.match(configSource, /toml_event: "PreToolUse"/);
  assert.match(configSource, /toml_event: "PostToolUse"/);
  assert.match(configSource, /matcher: Some\("\.\*wait_agent\$"\)/);
  assert.match(configSource, /toml_event: "SubagentStart"/);
  assert.match(configSource, /toml_event: "SubagentStop"/);
  assert.match(configSource, /toml_event: "Stop"/);
  assert.match(configSource, /trusted_hash/);
  assert.match(gateSource, /nonempty\(input\.agent_id\.as_deref\(\)\)\.is_some\(\)/);
  assert.match(gateSource, /permissionDecision": "deny"/);
  assert.match(gateSource, /"decision": "block"/);
  assert.match(gateSource, /is_collaboration_tool/);
  assert.match(gateSource, /is_spawn_agent_tool/);
  assert.match(gateSource, /subagent_spawn_denial/);
  assert.match(gateSource, /子代理不能继续派生子代理/);
  assert.match(gateSource, /post_wait_continuation/);
  assert.match(gateSource, /不得分析局部结果/);
  assert.match(gateSource, /SubagentStop/);
});
