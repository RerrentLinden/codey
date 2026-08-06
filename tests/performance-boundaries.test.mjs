import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("hot paths keep bounded work and avoid duplicate whole-payload processing", async () => {
  const [
    providerModels,
    modelSelection,
    appDialogs,
    startupMaintenance,
    diagnosticLog,
    protocolProxy,
    proxyLauncher,
    bridge,
    rendererInject,
    pluginFix,
  ] = await Promise.all([
    readFile(new URL("backend/src/provider_models.rs", root), "utf8"),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
    readFile(new URL("src/AppDialogs.tsx", root), "utf8"),
    readFile(new URL("backend/src/startup_maintenance.rs", root), "utf8"),
    readFile(
      new URL(
        "vendor/CodeyRuntime/crates/codey-runtime-core/src/diagnostic_log.rs",
        root,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "vendor/CodeyRuntime/crates/codey-runtime-core/src/protocol_proxy.rs",
        root,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "vendor/CodeyRuntime/crates/codey-runtime-core/src/launcher.rs",
        root,
      ),
      "utf8",
    ),
    readFile(
      new URL(
        "vendor/CodeyRuntime/crates/codey-runtime-core/src/bridge.rs",
        root,
      ),
      "utf8",
    ),
    readFile(new URL("public/renderer-inject.js", root), "utf8"),
    readFile(new URL("public/plugin-marketplace-fix.js", root), "utf8"),
  ]);

  assert.match(providerModels, /\.timeout\(PROVIDER_MODEL_REQUEST_TIMEOUT\)/);
  assert.match(providerModels, /MAX_PROVIDER_MODEL_RESPONSE_BYTES/);
  assert.match(providerModels, /while let Some\(chunk\) = response/);
  assert.doesNotMatch(providerModels, /response\s*\.bytes\(\)/);

  assert.match(modelSelection, /const seenKeys = new Set<string>\(\)/);
  assert.doesNotMatch(modelSelection, /!models\.includes\(normalized\)/);
  assert.match(appDialogs, /const MODEL_PICKER_PAGE_SIZE = 200/);
  assert.match(appDialogs, /\{open && <DialogContent/);
  assert.match(appDialogs, /filteredThirdPartyModels\.slice/);

  assert.match(startupMaintenance, /let cache_matches =/);
  assert.match(startupMaintenance, /if !cache_matches \{/);

  assert.match(diagnosticLog, /mpsc::sync_channel\(LOG_QUEUE_CAPACITY\)/);
  assert.match(diagnosticLog, /\.try_send\(LogCommand::Append/);
  assert.match(diagnosticLog, /LOG_FLUSH_BATCH_SIZE/);
  assert.match(protocolProxy, /Cow::Borrowed\(request_json\)/);
  assert.match(
    proxyLauncher,
    /open_responses_proxy_request_value_with_settings_and_user_agent/,
  );
  assert.match(proxyLauncher, /with_owned_request\(request_json\)/);

  assert.match(bridge, /BRIDGE_MAX_CONCURRENT_READ_HANDLERS: usize = 8/);
  assert.match(bridge, /BRIDGE_MAX_PENDING_CALLS: usize = 256/);
  assert.match(bridge, /fn bridge_path_can_run_concurrently/);

  assert.match(rendererInject, /let accountUsagePollingEnabled = true/);
  assert.match(rendererInject, /if \(!accountUsagePollingEnabled/);
  assert.match(pluginFix, /return originalFetch\(\.\.\.args\)/);
});
