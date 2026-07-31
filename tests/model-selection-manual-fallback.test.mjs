import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("third-party model sync can fall back to manual model support configuration", async () => {
  const [dialogSource, hookSource, commandSource, modelCommandSource] = await Promise.all([
    readFile(new URL("src/AppDialogs.tsx", root), "utf8"),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
    readFile(new URL("backend/src/commands.rs", root), "utf8"),
    readFile(new URL("backend/src/commands/models.rs", root), "utf8"),
  ]);

  assert.match(dialogSource, /默认展示 7 个官方模型/);
  assert.match(dialogSource, /modelState\.officialModels\.map/);
  assert.match(dialogSource, /placeholder="输入其他模型 ID/);
  assert.match(hookSource, /可能不支持 \/v1\/models 或 \/models 接口/);
  assert.match(hookSource, /modelState\.officialModelIds\.find/);
  assert.match(hookSource, /已在上方官方模型列表中，请直接勾选，不可重复输入/);
  assert.match(
    hookSource,
    /"save_selected_models", \{ officialModels, thirdPartyModels \}/,
  );
  assert.match(commandSource, /argument::<Vec<String>>\(&args, "officialModels"\)/);
  assert.match(commandSource, /argument::<Vec<String>>\(&args, "thirdPartyModels"\)/);
  assert.match(
    modelCommandSource,
    /已在官方模型列表中，请直接勾选，不可作为其他模型手动添加/,
  );
  assert.match(
    modelCommandSource,
    /startup_model_sync_models_or_fallback\([\s\S]*saved_models/,
  );
});
