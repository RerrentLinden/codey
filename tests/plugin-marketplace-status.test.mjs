import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);
const normalizeLineEndings = (source) => source.replace(/\r\n/g, "\n");

test("plugin marketplace repair is explicit and status checks stay read-only", async () => {
  const [marketplaceSource, pluginCommands, launcherSource, appSource, sectionsSource] =
    await Promise.all([
      readFile(new URL("backend/src/plugin_marketplace.rs", root), "utf8")
        .then(normalizeLineEndings),
      readFile(new URL("backend/src/commands/plugins.rs", root), "utf8")
        .then(normalizeLineEndings),
      readFile(new URL("backend/src/launcher.rs", root), "utf8")
        .then(normalizeLineEndings),
      readFile(new URL("src/App.tsx", root), "utf8")
        .then(normalizeLineEndings),
      readFile(new URL("src/OperationsPanel.tsx", root), "utf8")
        .then(normalizeLineEndings),
    ]);

  const statusFunction = pluginCommands.match(
    /pub\(super\) async fn plugin_marketplace_status\(\)[\s\S]*?\n}\n\npub\(super\) async fn repair_plugin_marketplace/,
  )?.[0] || "";
  const repairFunction = pluginCommands.match(
    /pub\(super\) async fn repair_plugin_marketplace\(\)[\s\S]*?\n}\n\nfn decorate_plugin_marketplace_status/,
  )?.[0] || "";

  assert.match(marketplaceSource, /pub fn marketplaces_status\(home: &Path\) -> Value/);
  assert.doesNotMatch(statusFunction, /ensure_marketplaces/);
  assert.match(statusFunction, /marketplaces_status/);
  assert.match(repairFunction, /ensure_marketplaces/);
  assert.doesNotMatch(launcherSource, /plugin_marketplace::ensure_marketplaces/);
  assert.doesNotMatch(launcherSource, /plugin_marketplace::marketplaces_status/);

  assert.match(
    appSource,
    /invoke<PluginMarketplaceStatus>\(\s*"plugin_marketplace_status"\s*,?\s*\)/,
  );
  assert.match(
    appSource,
    /invoke<PluginMarketplaceStatus>\(\s*"repair_plugin_marketplace"\s*,?\s*\)/,
  );
  assert.match(sectionsSource, /仅检查当前状态，不会在打开配置页时自动修复/);
  assert.match(sectionsSource, /远程市场：未缓存本地快照，无需修复/);
  assert.match(sectionsSource, /remoteMarketplaceCached/);
  assert.match(sectionsSource, /remoteRegistered/);
  assert.match(sectionsSource, /onRepairPluginMarketplace/);
  assert.match(sectionsSource, /手动修复/);
});
