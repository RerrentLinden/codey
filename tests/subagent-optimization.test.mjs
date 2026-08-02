import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("subagent optimization is opt-in and exposed through the settings switch", async () => {
  const [appSource, modelHookSource, sectionsSource, configSource, commandSource, launcherSource] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
    readFile(new URL("backend/src/config.rs", root), "utf8"),
    readFile(new URL("backend/src/commands.rs", root), "utf8"),
    readFile(new URL("backend/src/launcher.rs", root), "utf8"),
  ]);
  const uiSource = `${appSource}\n${sectionsSource}`;
  const modelSource = `${appSource}\n${modelHookSource}`;

  assert.match(configSource, /pub subagent_optimization: bool/);
  assert.match(configSource, /subagent_optimization: false/);
  assert.match(commandSource, /config\.subagent_optimization = config_input\.subagent_optimization/);
  assert.match(launcherSource, /config\.subagent_optimization/);
  assert.match(configSource, /pub subagent_model: String/);
  assert.match(configSource, /pub subagent_reasoning_effort: String/);
  assert.match(uiSource, /checked=\{config\.subagentOptimization\}/);
  assert.match(
    uiSource,
    /onCheckedChange=\{\(checked\) =>\s*onSubagentOptimizationChange\(checked\)\s*\}/,
  );
  assert.match(uiSource, /aria-label="启用子代理协作优化"/);
  assert.match(uiSource, /aria-label="选择子代理模型"/);
  assert.match(uiSource, /aria-label="选择子代理思考深度"/);
  assert.match(uiSource, /subagentModelOptions/);
  assert.match(modelSource, /invoke\("fetch_current_provider_models"\)/);
  assert.match(modelSource, /supportsModel\(result\.models, subagentModel\)/);
  assert.match(modelSource, /provider\.official \? "官方账号" : "第三方 API"/);
  assert.match(modelSource, /不支持 \$\{subagentModel\}，无法开启子代理协作优化/);
  assert.match(uiSource, /无需重启/);
  assert.doesNotMatch(uiSource, /下次启动启用 V2 并行配置，退出时自动恢复原文件/);
});

test("subagent optimization owns the requested V2 and default-agent settings", async () => {
  const source = await readFile(new URL("backend/src/codex_config.rs", root), "utf8");

  assert.match(source, /multi_agent\["enabled"\] = value\(true\)/);
  assert.match(source, /multi_agent\["hide_spawn_agent_metadata"\] = value\(true\)/);
  assert.match(source, /multi_agent\["tool_namespace"\] = value\("agents"\)/);
  assert.match(source, /multi_agent\["max_concurrent_threads_per_session"\] = value\(7\)/);
  assert.match(source, /multi_agent\["max_wait_timeout_ms"\] = value\(120_000\)/);
  assert.match(source, /doc\.as_table_mut\(\)\.remove\("agents"\)/);
  assert.match(source, /agents\["default_subagent_model"\] = value\(subagent_model\.trim\(\)\)/);
  assert.match(source, /agents\["default_subagent_reasoning_effort"\]/);
  assert.doesNotMatch(source, /\nmodel = "gpt-5\.6-luna"/);
  assert.match(source, /image_generation = false/);
  assert.match(source, /const SUBAGENT_GUIDANCE: &str/);
});

test("subagent defaults are hot-reloaded through the active app-server client", async () => {
  const [commandSource, cdpSource, rendererSource] = await Promise.all([
    readFile(new URL("backend/src/commands.rs", root), "utf8"),
    readFile(new URL("backend/src/cdp.rs", root), "utf8"),
    readFile(
      new URL("vendor/CodeyRuntime/assets/inject/renderer-inject.js", root),
      "utf8",
    ),
  ]);

  assert.match(commandSource, /hot_reload_runtime_subagent_defaults/);
  assert.match(cdpSource, /window\.__codeyApplySubagentDefaults/);
  assert.match(rendererSource, /default_subagent_model/);
  assert.match(rendererSource, /default_subagent_reasoning_effort/);
  assert.match(rendererSource, /thread\/resume/);
  assert.match(rendererSource, /thread\/start/);
});
