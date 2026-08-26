import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("subagent settings expose the five supported role controls", async () => {
  const [featurePolicySource, modelHookSource, modelOptionsSource, comboboxSource] = await Promise.all([
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
    readFile(new URL("src/subagentModels.ts", root), "utf8"),
    readFile(new URL("src/components/ModelCombobox.tsx", root), "utf8"),
  ]);

  assert.match(featurePolicySource, /checked=\{config\.subagentOptimization\}/);
  assert.match(
    featurePolicySource,
    /onCheckedChange=\{\(checked\) =>\s*onSubagentOptimizationChange\(checked\)\s*\}/,
  );
  for (const [id, name] of [
    ["codey_quick_scan", "快速定位"],
    ["codey_deep_research", "深度检索"],
    ["codey_visual_analysis", "视觉分析"],
    ["codey_worker", "代码实施"],
    ["codey_visual_worker", "视觉实施"],
  ]) {
    assert.match(featurePolicySource, new RegExp(`id: "${id}"`));
    assert.match(featurePolicySource, new RegExp(`name: "${name}"`));
  }
  assert.match(featurePolicySource, /config\.subagentRoles\[task\.id\]/);
  assert.match(featurePolicySource, /<ModelCombobox/);
  assert.match(
    modelHookSource,
    /buildSubagentModelOptions\(\s*config,\s*modelState,\s*officialAccountAvailable/,
  );
  assert.match(modelOptionsSource, /for \(const profile of config\.profiles\)/);
  assert.match(modelOptionsSource, /value = routeModelAlias\(profile, modelId\)/);
  assert.match(modelOptionsSource, /const usesOfficialMetadata = official/);
  assert.match(modelOptionsSource, /THIRD_PARTY_REASONING_EFFORTS/);
  assert.match(modelOptionsSource, /resolveSubagentModelOption/);
  assert.match(comboboxSource, /<Combobox\.Search/);
  assert.match(comboboxSource, /搜索模型或线路/);
  assert.match(comboboxSource, /<Combobox\.Group/);
});
