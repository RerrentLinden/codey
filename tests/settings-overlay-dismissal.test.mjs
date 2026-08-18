import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { readAppStyles } from "./helpers/read-app-styles.mjs";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const root = new URL("../", import.meta.url);

test("settings Semi modal dismissal restores unsaved config", async () => {
  const [appSource, shellSource, overlaySource, overlayStyles] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/SettingsModalShell.tsx", root), "utf8"),
    readFile(new URL("src/overlay.tsx", root), "utf8"),
    readFile(new URL("src/overlay.css", root), "utf8"),
  ]);
  const { SETTINGS_OVERLAY_Z_INDEX, SETTINGS_OVERLAY_Z_INDEX_CSS } =
    await loadTypeScriptModule(
      new URL("../src/overlay.constants.ts", import.meta.url),
    );

  assert.match(
    appSource,
    /function closeSettings\(\) \{[\s\S]*setConfig\(persistedConfigRef\.current\)[\s\S]*setDirty\(false\)[\s\S]*onClose\?\.\(\)/,
  );
  assert.match(
    shellSource,
    /import SemiModal from "@douyinfe\/semi-ui\/lib\/es\/modal"/,
  );
  assert.match(
    shellSource,
    /<SemiModal[\s\S]*closeOnEsc=\{false\}[\s\S]*closable=\{header === undefined\}[\s\S]*maskClosable=\{false\}[\s\S]*onCancel=\{onCancel\}/,
  );
  assert.match(appSource, /onCancel=\{handleCloseSettings\}/);

  assert.doesNotMatch(overlaySource, /codey-overlay-backdrop/);
  assert.doesNotMatch(overlaySource, /codey-overlay-dialog/);
  assert.match(overlaySource, /modalVisible=\{visible\}/);
  assert.match(overlaySource, /onClose=\{close\}/);
  assert.match(overlaySource, /SETTINGS_OPENED_EVENT \} from "\.\/useRuntimeStatus"/);
  assert.match(overlaySource, /toggle: open/);
  assert.equal(SETTINGS_OVERLAY_Z_INDEX, 2_147_483_647);
  assert.equal(SETTINGS_OVERLAY_Z_INDEX_CSS, "2147483647");
  assert.match(
    shellSource,
    /import \{ SETTINGS_OVERLAY_Z_INDEX \} from "\.\/overlay\.constants"/,
  );
  assert.match(shellSource, /zIndex=\{SETTINGS_OVERLAY_Z_INDEX\}/);
  assert.match(
    overlaySource,
    /"z-index",\s*SETTINGS_OVERLAY_Z_INDEX_CSS,\s*"important"/,
  );
  assert.match(
    overlaySource,
    /document\.documentElement\.appendChild\(host\);[\s\S]*host\.style\.display = "block"/,
  );
  assert.match(
    overlayStyles,
    /:host \{[\s\S]*z-index:\s*var\(--codey-settings-overlay-z-index\) !important;/,
  );
});

test("all editable feature controls are locked while an operation is active", async () => {
  const source = await readFile(
    new URL("src/FeaturePolicyCard.tsx", root),
    "utf8",
  );

  assert.match(
    source,
    /className="gpu-mode-fieldset"\s*disabled=\{isBusy\}/,
  );
  assert.match(source, /\{isWindowsClient && \(/);
  for (const setting of [
    "slimCodexPet",
    "fastCodexStartup",
    "disableTraceLogWrites",
    "protectCrashpadPending",
    "hideFullAccessWarning",
  ]) {
    assert.match(
      source,
      new RegExp(
        `checked=\\{config\\.${setting}\\}[\\s\\S]{0,80}disabled=\\{isBusy\\}`,
      ),
    );
  }
});

test("settings notice toast is scoped to the settings page", async () => {
  const [appSource, noticeSource, stylesSource] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/useAppNotice.tsx", root), "utf8"),
    readAppStyles(root),
  ]);

  assert.match(appSource, /<main[\s\S]*className=\{`app-shell/);
  assert.match(appSource, /<NoticeToast[\s\S]*controller=\{noticeController\}/);
  assert.match(noticeSource, /className=\{`notice-toast \$\{notice\.tone\}`\}/);
  assert.match(noticeSource, /const NOTICE_AUTO_DISMISS_MS = 5_000/);
  assert.match(
    noticeSource,
    /const \[autoDismissPaused, setAutoDismissPaused\] = useState\(false\)/,
  );
  assert.match(
    noticeSource,
    /if \(!autoDismissEnabled \|\| !notice\.text \|\| autoDismissPaused\)/,
  );
  assert.match(
    noticeSource,
    /window\.setTimeout\(\(\) => \{[\s\S]*controller\.setNotice\(\(current\) =>[\s\S]*NOTICE_AUTO_DISMISS_MS/,
  );
  assert.match(noticeSource, /window\.clearTimeout\(timeout\)/);
  assert.match(noticeSource, /onMouseEnter=\{\(\) => setAutoDismissPaused\(true\)\}/);
  assert.match(noticeSource, /onMouseLeave=\{\(\) => setAutoDismissPaused\(false\)\}/);
  assert.match(noticeSource, /onFocus=\{\(\) => setAutoDismissPaused\(true\)\}/);
  assert.match(noticeSource, /onBlur=\{\(\) => setAutoDismissPaused\(false\)\}/);
  assert.match(noticeSource, /aria-label="关闭提示"/);
  assert.match(
    noticeSource,
    /onClick=\{\(\) => \{[\s\S]*setAutoDismissPaused\(false\);[\s\S]*controller\.setNotice\(\{ tone: "info", text: "" \}\);[\s\S]*\}\}/,
  );
  assert.match(
    stylesSource,
    /\.notice-toast \{\s*position: absolute;\s*right: 24px;\s*bottom: 24px;/,
  );
  assert.doesNotMatch(
    stylesSource,
    /\.notice-toast \{\s*position: fixed;/,
  );
});

test("operations tooltips stay inside the settings overlay", async () => {
  const appSectionsSource = await readFile(
    new URL("src/OperationsPanel.tsx", root),
    "utf8",
  );
  assert.match(
    appSectionsSource,
    /const operationsHubRef = useRef<HTMLElement>\(null\)/,
  );
  assert.match(
    appSectionsSource,
    /operationsHubRef\.current\?\s*\.closest<HTMLElement>\("\.app-shell"\)\s*\?\?\s*document\.body/,
  );
  assert.match(appSectionsSource, /ref=\{operationsHubRef\}/);
  assert.match(appSectionsSource, /getPopupContainer=\{getTooltipContainer\}/);
});

test("settings select popups stay inside the settings overlay", async () => {
  const [
    appSource,
    featurePolicySource,
    promptOptimizationSource,
    notificationCardSource,
    notificationDialogSource,
  ] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
    readFile(new URL("src/PromptOptimizationCard.tsx", root), "utf8"),
    readFile(new URL("src/notifications/NotificationChannelsCard.tsx", root), "utf8"),
    readFile(new URL("src/notifications/NotificationChannelDialog.tsx", root), "utf8"),
  ]);

  assert.match(
    appSource,
    /const popupContainer = modalContainer \?\? null/,
  );
  assert.match(
    appSource,
    /<NotificationChannelsCard[\s\S]*popupContainer=\{popupContainer\}/,
  );
  assert.match(
    appSource,
    /<FeaturePolicyCard[\s\S]*popupContainer=\{popupContainer\}/,
  );
  assert.match(
    appSource,
    /<PromptOptimizationCard[\s\S]*popupContainer=\{popupContainer\}/,
  );
  assert.match(
    notificationCardSource,
    /<NotificationChannelDialog[\s\S]*popupContainer=\{popupContainer\}/,
  );

  for (const source of [
    featurePolicySource,
    promptOptimizationSource,
    notificationDialogSource,
  ]) {
    assert.match(
      source,
      /getPopupContainer=\{\(\) => popupContainer \?\? document\.body\}/,
    );
    assert.doesNotMatch(
      source,
      /getPopupContainer=\{\(\) => document\.body\}/,
    );
  }
});
