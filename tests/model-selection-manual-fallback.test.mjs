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
  assert.match(dialogSource, /manualThirdPartyModelKeys\.has/);
  assert.match(dialogSource, /aria-label=\{`删除其他模型 \$\{model\}`\}/);
  assert.match(dialogSource, /onDeleteThirdPartyModel\(model\)/);
  assert.match(hookSource, /可能不支持 \/v1\/models 或 \/models 接口/);
  assert.match(hookSource, /modelState\.officialModelIds\.find/);
  assert.match(hookSource, /已在上方官方模型列表中，请直接勾选，不可重复输入/);
  assert.match(hookSource, /deleteDraftThirdPartyModel/);
  assert.match(hookSource, /deleteThirdPartyModel/);
  assert.match(hookSource, /manualThirdPartyModels/);
  assert.match(hookSource, /deletedThirdPartyModels: deletedModels/);
  assert.match(
    hookSource,
    /"save_selected_models",\s*\{\s*officialModels,\s*thirdPartyModels,/,
  );
  assert.match(commandSource, /argument::<Vec<String>>\(&args, "officialModels"\)/);
  assert.match(commandSource, /argument::<Vec<String>>\(&args, "thirdPartyModels"\)/);
  assert.match(commandSource, /optional_argument::<Vec<String>>\(&args, "manualThirdPartyModels"\)/);
  assert.match(commandSource, /optional_argument::<Vec<String>>\(&args, "deletedThirdPartyModels"\)/);
  assert.match(
    modelCommandSource,
    /已在官方模型列表中，请直接勾选，不可作为其他模型手动添加/,
  );
  assert.match(modelCommandSource, /官方模型 \{model\} 不能作为其他模型删除/);
  assert.match(modelCommandSource, /不是手动添加的其他模型，不能删除/);
  assert.match(modelCommandSource, /validate_manual_third_party_model_sources/);
  assert.match(modelCommandSource, /preserve_selected_third_party_models_except/);
  assert.match(
    modelCommandSource,
    /refreshed_model_state_async\(&config, false\)\.await\?/,
  );
  assert.match(modelCommandSource, /tokio::task::spawn_blocking/);
  assert.match(modelCommandSource, /rollback_model_catalog_after_config_save/);
  assert.match(
    modelCommandSource,
    /let model_catalog_fallback = catalog_refresh[\s\S]*?\.is_some_and\(\|refresh\| refresh\.fallback\)/,
  );
  assert.match(
    modelCommandSource,
    /"modelCatalogFallback":model_catalog_fallback/,
  );
  assert.match(
    modelCommandSource,
    /startup_model_sync_models_or_fallback\([\s\S]*saved_models/,
  );
  assert.match(modelCommandSource, /cdp::refresh_model_whitelist/);
  assert.match(modelCommandSource, /"modelHotReloaded"/);
  assert.match(hookSource, /modelHotReloaded/);
  assert.match(hookSource, /Codex 模型列表已立即更新/);
  assert.match(hookSource, /重启 Codex 后生效/);
});
