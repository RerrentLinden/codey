import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("settings panels keep stable handlers and skip unrelated parent renders", async () => {
  const [
    app,
    appUpdates,
    sections,
    dialogs,
    trace,
    notificationCard,
    feishuEditor,
    telegramEditor,
    channelRegistry,
  ] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/useAppUpdates.ts", root), "utf8"),
    Promise.all(
      [
        "OperationsPanel.tsx",
        "AppUpdateCard.tsx",
        "ModelSection.tsx",
        "ExperimentalFeaturesCard.tsx",
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
      new URL("src/notifications/FeishuChannelEditor.tsx", root),
      "utf8",
    ),
    readFile(
      new URL("src/notifications/TelegramChannelEditor.tsx", root),
      "utf8",
    ),
    readFile(new URL("src/notifications/channelRegistry.tsx", root), "utf8"),
  ]);

  assert.match(app, /function useStableEvent</);
  assert.match(app, /useAppUpdates\(\{/);
  assert.doesNotMatch(app, /async function checkForUpdates\(/);
  assert.match(appUpdates, /export function useAppUpdates/);
  assert.match(appUpdates, /invoke<UpdateCheck>\("check_for_updates"\)/);
  assert.match(appUpdates, /invoke<UpdateDownload>\("download_update"\)/);
  assert.match(appUpdates, /invoke\("install_downloaded_update"/);
  assert.match(app, /onRepairPluginMarketplace=\{handleRepairPluginMarketplace\}/);
  assert.match(app, /onRefresh=\{handleRefreshTraceLogStats\}/);
  assert.match(app, /onToggleDraftModel=\{handleToggleDraftModel\}/);
  assert.doesNotMatch(
    app,
    /onRepairPluginMarketplace=\{\(\) => void repairPluginMarketplace\(\)\}/,
  );

  for (const component of [
    "OperationsPanel",
    "AppUpdateCard",
    "ModelSection",
    "ExperimentalFeaturesCard",
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
  assert.match(feishuEditor, /export const FeishuChannelEditor = memo\(/);
  assert.match(telegramEditor, /export const TelegramChannelEditor = memo\(/);
  assert.match(channelRegistry, /feishu:\s*\{/);
  assert.match(channelRegistry, /telegram:\s*\{/);
});
