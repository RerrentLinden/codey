import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const root = new URL("../", import.meta.url);

test("settings modal keeps dismissal and stacking inside the overlay", async () => {
  const [appSource, shellSource, overlaySource, constants] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/SettingsModalShell.tsx", root), "utf8"),
    readFile(new URL("src/overlay.tsx", root), "utf8"),
    loadTypeScriptModule(new URL("../src/overlay.constants.ts", import.meta.url)),
  ]);

  assert.match(
    appSource,
    /function closeSettings\(\) \{[\s\S]*setConfig\(persistedConfigRef\.current\)[\s\S]*setDirty\(false\)[\s\S]*onClose\?\.\(\)/,
  );
  assert.match(
    shellSource,
    /<SemiModal[\s\S]*closeOnEsc=\{false\}[\s\S]*maskClosable=\{false\}[\s\S]*onCancel=\{onCancel\}/,
  );
  assert.match(appSource, /onCancel=\{handleCloseSettings\}/);
  assert.doesNotMatch(overlaySource, /codey-overlay-(?:backdrop|dialog)/);
  assert.equal(constants.SETTINGS_OVERLAY_Z_INDEX, 2_147_483_647);
  assert.equal(constants.SETTINGS_OVERLAY_Z_INDEX_CSS, "2147483647");
});

test("settings controls and popups share the modal busy and portal boundaries", async () => {
  const [appSource, featurePolicySource, promptSource, channelCardSource, channelDialogSource] =
    await Promise.all([
      readFile(new URL("src/App.tsx", root), "utf8"),
      readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
      readFile(new URL("src/PromptOptimizationCard.tsx", root), "utf8"),
      readFile(new URL("src/notifications/NotificationChannelsCard.tsx", root), "utf8"),
      readFile(new URL("src/notifications/NotificationChannelDialog.tsx", root), "utf8"),
    ]);

  assert.match(featurePolicySource, /className="gpu-mode-fieldset"\s*disabled=\{isBusy\}/);
  for (const setting of [
    "slimCodexPet",
    "disableTraceLogWrites",
    "protectCrashpadPending",
    "hideFullAccessWarning",
  ]) {
    assert.match(
      featurePolicySource,
      new RegExp(`checked=\\{config\\.${setting}\\}[\\s\\S]{0,80}disabled=\\{isBusy\\}`),
    );
  }
  assert.match(appSource, /const popupContainer = modalContainer \?\? null/);
  assert.match(
    channelCardSource,
    /<NotificationChannelDialog[\s\S]*popupContainer=\{popupContainer\}/,
  );
  for (const source of [featurePolicySource, promptSource, channelDialogSource]) {
    assert.match(source, /getPopupContainer=\{\(\) => popupContainer \?\? document\.body\}/);
  }
});
