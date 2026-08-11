import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("settings panels keep stable handlers and skip unrelated parent renders", async () => {
  const [
    app,
    appUpdates,
    notice,
    confirmation,
    appPathDialog,
    sections,
    modelSelection,
  ] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/useAppUpdates.ts", root), "utf8"),
    readFile(new URL("src/useAppNotice.tsx", root), "utf8"),
    readFile(new URL("src/useConfirmationDialog.tsx", root), "utf8"),
    readFile(new URL("src/CodexAppPathDialogHost.tsx", root), "utf8"),
    Promise.all(
      [
        "OperationsPanel.tsx",
        "AppUpdateCard.tsx",
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
  assert.match(appPathDialog, /useSyncExternalStore\(/);
  assert.match(appPathDialog, /export const CodexAppPathDialogHost = memo\(/);
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
    "AppUpdateCard",
    "ModelSection",
    "FeaturePolicyCard",
  ]) {
    assert.match(sections, new RegExp(`export const ${component} = memo\\(`));
  }
});

test("runtime polling preserves stable slices and narrows panel props", async () => {
  const [app, runtimeHook, runtimeSnapshot, updateCard, featureCard, operations] =
    await Promise.all([
      readFile(new URL("src/App.tsx", root), "utf8"),
      readFile(new URL("src/useRuntimeStatus.ts", root), "utf8"),
      readFile(new URL("src/runtimeStatusSnapshot.ts", root), "utf8"),
      readFile(new URL("src/AppUpdateCard.tsx", root), "utf8"),
      readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
      readFile(new URL("src/OperationsPanel.tsx", root), "utf8"),
    ]);

  assert.match(runtimeHook, /reconcileRuntimeStatus/);
  assert.equal(
    runtimeHook.match(/reconcileRuntimeStatus\(current, next\)/g)?.length,
    4,
  );
  assert.match(runtimeSnapshot, /if \(valuesEqual\(current, next\)\) return current/);
  assert.match(runtimeSnapshot, /current\.traceLogStats/);
  assert.match(runtimeSnapshot, /current\.injectionScripts/);
  assert.match(app, /const operationsStatus = useMemo\(/);
  assert.match(app, /status=\{operationsStatus\}/);
  assert.match(app, /appVersion=\{status\.appVersion\}/);
  assert.match(app, /isMacClient=\{status\.clientPlatform === "macos"\}/);
  assert.doesNotMatch(updateCard, /RuntimeStatus/);
  assert.doesNotMatch(featureCard, /RuntimeStatus/);
  assert.match(operations, /type OperationsRuntimeStatus = Pick</);
});
