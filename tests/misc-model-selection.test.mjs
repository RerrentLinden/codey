import assert from "node:assert/strict";
import test from "node:test";

import { readSource } from "./helpers/read-source.mjs";

test("misc model setting is part of the saved configuration", async () => {
  const [typesSource, mockApiSource] = await Promise.all([
    readSource("src/App.types.ts"),
    readSource("src/dev/mockApi.ts"),
  ]);

  assert.match(typesSource, /miscModel: string;/);
  assert.match(mockApiSource, /miscModel: "",/);
});

test("misc model and retry controls sit in the route card above readonly note", async () => {
  const modelSectionSource = await readSource("src/ModelSection.tsx");

  assert.match(modelSectionSource, /<ModelCombobox/);
  assert.match(modelSectionSource, /aria-label="杂事模型"/);
  assert.match(modelSectionSource, /恢复默认/);
  assert.match(modelSectionSource, /codex-auto-review/);
  assert.match(modelSectionSource, /miscModel: ""/);
  assert.match(modelSectionSource, /miscModel: value/);
  assert.match(modelSectionSource, /streamMaxRetries/);
  assert.match(modelSectionSource, /会话重试/);
  assert.match(modelSectionSource, /route-auxiliary-bar/);

  const miscIndex = modelSectionSource.indexOf('aria-label="杂事模型"');
  const retryIndex = modelSectionSource.indexOf('aria-label="会话错误重试次数"');
  const noteIndex = modelSectionSource.indexOf('className="readonly-note"');
  assert.ok(miscIndex >= 0 && retryIndex >= 0 && noteIndex >= 0);
  assert.ok(miscIndex < noteIndex && retryIndex < noteIndex);
});

test("misc model card is replaced by inline route card controls", async () => {
  const appSource = await readSource("src/App.tsx");

  assert.doesNotMatch(appSource, /<MiscModelCard/);
  assert.doesNotMatch(appSource, /import \{ MiscModelCard \}/);
  assert.match(appSource, /<ModelSection[\s\S]*?subagentModelOptions=\{subagentModelOptions\}/);
});

