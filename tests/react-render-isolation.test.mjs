import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const root = new URL("../", import.meta.url);

test("settings panels keep stable handlers and skip unrelated parent renders", async () => {
  const [
    app,
    appUpdates,
    notice,
    confirmation,
    sections,
    modelSelection,
  ] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/useAppUpdates.ts", root), "utf8"),
    readFile(new URL("src/useAppNotice.tsx", root), "utf8"),
    readFile(new URL("src/useConfirmationDialog.tsx", root), "utf8"),
    Promise.all(
      [
        "OperationsPanel.tsx",
        "ModelSection.tsx",
        "FeaturePolicyCard.tsx",
      ].map((file) => readFile(new URL(`src/${file}`, root), "utf8")),
    ).then((sources) => sources.join("\n")),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
  ]);

  assert.doesNotMatch(app, /useState<Notice>/);
  assert.doesNotMatch(app, /useState<Confirmation/);
  assert.match(notice, /useSyncExternalStore\(/);
  assert.match(notice, /export const NoticeToast = memo\(/);
  assert.match(confirmation, /useSyncExternalStore\(/);
  assert.match(confirmation, /export const ConfirmationDialogHost = memo\(/);
  assert.doesNotMatch(app, /CodexAppPathDialog/);
  assert.doesNotMatch(app, /async function checkForUpdates\(/);
  assert.match(appUpdates, /export function useAppUpdates/);
  assert.match(appUpdates, /invoke<UpdateCheck>\("check_for_updates"\)/);
  assert.equal(
    appUpdates.match(/invoke<UpdateCheck>\("check_for_updates"\)/g)?.length,
    1,
  );
  assert.match(
    appUpdates,
    /updateCheckInFlightRef = useRef<Promise<UpdateCheck> \| null>/,
  );
  assert.match(appUpdates, /const result = await requestUpdateCheck\(\)/);
  assert.match(appUpdates, /invoke<UpdateDownload>\("download_update"\)/);
  assert.match(appUpdates, /invoke\("install_downloaded_update"/);
  assert.match(app, /onRepairPluginMarketplace=\{handleRepairPluginMarketplace\}/);
  assert.match(app, /onRefresh=\{handleRefreshTraceLogStats\}/);
  assert.match(app, /onToggleDraftModel=\{toggleDraftModel\}/);
  assert.match(app, /onFetchCurrentModels=\{fetchCurrentModels\}/);
  assert.match(app, /onSetDefaultModel=\{setDefaultModel\}/);
  assert.match(app, /onSave=\{saveModelSelection\}/);
  assert.doesNotMatch(modelSelection, /withTimeout/);
  assert.doesNotMatch(app, /handleFetchCurrentModels|handleSetDefaultModel/);
  assert.doesNotMatch(
    app,
    /onRepairPluginMarketplace=\{\(\) => void repairPluginMarketplace\(\)\}/,
  );

  for (const component of [
    "OperationsPanel",
    "ModelSection",
    "FeaturePolicyCard",
  ]) {
    assert.match(sections, new RegExp(`export const ${component} = memo\\(`));
  }
});

test("runtime polling preserves referentially stable status slices", async () => {
  const { reconcileRuntimeStatus } = await loadTypeScriptModule(
    new URL("../src/runtimeStatusSnapshot.ts", import.meta.url),
  );
  const current = {
    running: false,
    appVersion: "1.0.0",
    maintenance: { sessionStatus: "ready", sessionFilesFixed: 2 },
    injectionScripts: [{ id: "bridge", status: "effective" }],
    traceLogStats: { pending: false, rows: 3 },
    crashpadPendingStats: { pending: false, reports: 1 },
  };

  const equalSnapshot = structuredClone(current);
  assert.equal(reconcileRuntimeStatus(current, equalSnapshot), current);

  const changedRoot = reconcileRuntimeStatus(current, {
    ...structuredClone(current),
    running: true,
  });
  assert.notEqual(changedRoot, current);
  assert.equal(changedRoot.maintenance, current.maintenance);
  assert.equal(changedRoot.injectionScripts, current.injectionScripts);
  assert.equal(changedRoot.traceLogStats, current.traceLogStats);
  assert.equal(changedRoot.crashpadPendingStats, current.crashpadPendingStats);

  const changedMaintenance = reconcileRuntimeStatus(current, {
    ...structuredClone(current),
    maintenance: { sessionStatus: "error", sessionFilesFixed: 2 },
  });
  assert.notEqual(changedMaintenance.maintenance, current.maintenance);
});
