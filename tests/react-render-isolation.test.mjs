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
    dialogs,
    trace,
    notificationCard,
    notificationDialog,
    feishuEditor,
    telegramEditor,
    channelRegistry,
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
    readFile(new URL("src/AppDialogs.tsx", root), "utf8"),
    readFile(new URL("src/TraceLogModule.tsx", root), "utf8"),
    readFile(
      new URL("src/notifications/NotificationChannelsCard.tsx", root),
      "utf8",
    ),
    readFile(
      new URL("src/notifications/NotificationChannelDialog.tsx", root),
      "utf8",
    ),
    readFile(
      new URL("src/notifications/FeishuChannelEditor.tsx", root),
      "utf8",
    ),
    readFile(
      new URL("src/notifications/TelegramChannelEditor.tsx", root),
      "utf8",
    ),
    readFile(new URL("src/notifications/channelRegistry.tsx", root), "utf8"),
    readFile(new URL("src/useModelSelection.ts", root), "utf8"),
  ]);

  assert.match(app, /function useStableEvent</);
  assert.match(app, /useAppUpdates\(\{/);
  assert.match(app, /useAppNoticeController\(\)/);
  assert.match(app, /useConfirmationController\(\)/);
  assert.match(app, /useCodexAppPathDialogController\(\)/);
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
  assert.match(appUpdates, /invoke<UpdateDownload>\("download_update"\)/);
  assert.match(appUpdates, /invoke\("install_downloaded_update"/);
  assert.match(app, /onRepairPluginMarketplace=\{handleRepairPluginMarketplace\}/);
  assert.match(app, /onRefresh=\{handleRefreshTraceLogStats\}/);
  assert.match(app, /onToggleDraftModel=\{toggleDraftModel\}/);
  assert.match(app, /onFetchCurrentModels=\{fetchCurrentModels\}/);
  assert.match(app, /onSetDefaultModel=\{setDefaultModel\}/);
  assert.match(app, /onSave=\{saveModelSelection\}/);
  assert.doesNotMatch(app, /handleFetchCurrentModels|handleSetDefaultModel/);
  for (const callback of [
    "fetchCurrentModels",
    "updateSubagentOptimization",
    "toggleDraftModel",
    "updateCustomModelInput",
    "addCustomModel",
    "deleteDraftThirdPartyModel",
    "applyModelSelection",
    "saveModelSelection",
    "deleteThirdPartyModel",
    "setDefaultModel",
  ]) {
    assert.match(
      modelSelection,
      new RegExp(`const ${callback} = useCallback\\(`),
    );
  }
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
  for (const component of [
    "ModelPickerDialog",
    "ConfirmationDialog",
    "CodexAppPathDialog",
  ]) {
    assert.match(dialogs, new RegExp(`export const ${component} = memo\\(`));
  }
  assert.match(trace, /export const TraceLogModule = memo\(/);
  assert.match(
    notificationCard,
    /export const NotificationChannelsCard = memo\(/,
  );
  assert.match(
    notificationDialog,
    /export const NotificationChannelDialog = memo\(/,
  );
  assert.match(notificationDialog, /notification-channel-select/);
  assert.match(notificationDialog, /reveal_notification_channel/);
  assert.match(notificationDialog, /notification-enabled-control/);
  assert.match(notificationDialog, /test_notification_channel/);
  assert.match(notificationDialog, /hasSuccessfulTest/);
  assert.match(notificationCard, /onRequestRemoveChannel/);
  assert.doesNotMatch(notificationCard, /待完成配置/);
  assert.doesNotMatch(notificationCard, /测试通知/);
  assert.match(app, /delete-notification-channel/);
  assert.match(feishuEditor, /export const FeishuChannelEditor = memo\(/);
  assert.match(telegramEditor, /export const TelegramChannelEditor = memo\(/);
  assert.match(channelRegistry, /feishu:\s*\{/);
  assert.match(channelRegistry, /telegram:\s*\{/);
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
