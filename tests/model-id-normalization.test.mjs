import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

async function loadModelIdHelpers() {
  const source = await readFile(
    new URL("../src/modelIds.ts", import.meta.url),
    "utf8",
  );
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ESNext,
      target: ts.ScriptTarget.ES2020,
    },
  }).outputText;
  return import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);
}

test("model IDs compare case-insensitively while preserving first spelling", async () => {
  const {
    includesModelId,
    modelIdsEqual,
    modelKey,
    uniqueModelIds,
  } = await loadModelIdHelpers();

  assert.equal(modelKey(" Provider-Coder "), "provider-coder");
  assert.equal(modelIdsEqual("Provider-Coder", " provider-coder "), true);
  assert.equal(
    includesModelId(["Provider-Coder", "Provider-Reasoner"], "PROVIDER-CODER"),
    true,
  );
  assert.deepEqual(
    uniqueModelIds([
      " Provider-Coder ",
      "provider-coder",
      "Provider-Reasoner",
      "",
    ]),
    ["Provider-Coder", "Provider-Reasoner"],
  );
});
