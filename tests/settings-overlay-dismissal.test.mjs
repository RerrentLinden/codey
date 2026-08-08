import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("settings Semi modal dismissal restores unsaved config", async () => {
  const [appSource, overlaySource, overlayStyles] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/overlay.tsx", root), "utf8"),
    readFile(new URL("src/overlay.css", root), "utf8"),
  ]);

  assert.match(
    appSource,
    /const persistedConfigRef = useRef<Config \| null>\(null\)/,
  );
  assert.match(
    appSource,
    /function closeSettings\(\) \{[\s\S]*setConfig\(persistedConfigRef\.current\)[\s\S]*setDirty\(false\)[\s\S]*onClose\?\.\(\)/,
  );
  assert.match(
    appSource,
    /import SemiModal from "@douyinfe\/semi-ui\/lib\/es\/modal"/,
  );
  assert.match(
    appSource,
    /<SemiModal[\s\S]*closeOnEsc=\{false\}[\s\S]*closable=\{header === undefined\}[\s\S]*maskClosable=\{false\}[\s\S]*onCancel=\{onCancel\}/,
  );
  assert.match(appSource, /onCancel=\{handleCloseSettings\}/);
  assert.match(
    appSource,
    /header=\{\([\s\S]*codey-settings-modal-header[\s\S]*configHeaderContent/,
  );
  assert.match(appSource, /aria-label="关闭配置"/);
  assert.match(appSource, /className="title-restart-button"/);
  assert.match(appSource, /onClick=\{handleRestartCodex\}/);
  assert.match(appSource, /showRestartAction=\{!embedded\}/);
  assert.match(
    appSource,
    /\{!embedded && \(\s*<header className="config-header">\{configHeaderContent\}<\/header>/,
  );
  assert.match(appSource, /\{!embedded && \(/);

  assert.doesNotMatch(overlaySource, /codey-overlay-backdrop/);
  assert.doesNotMatch(overlaySource, /codey-overlay-dialog/);
  assert.match(overlaySource, /modalVisible=\{visible\}/);
  assert.match(overlaySource, /onClose=\{close\}/);
  assert.match(overlaySource, /SETTINGS_OPENED_EVENT \} from "\.\/useRuntimeStatus"/);
  assert.match(overlaySource, /toggle: open/);
  assert.match(appSource, /const SETTINGS_OVERLAY_Z_INDEX = 2147483647/);
  assert.match(appSource, /zIndex=\{SETTINGS_OVERLAY_Z_INDEX\}/);
  assert.match(overlaySource, /const SETTINGS_OVERLAY_Z_INDEX = "2147483647"/);
  assert.match(
    overlaySource,
    /host\.style\.setProperty\("z-index", SETTINGS_OVERLAY_Z_INDEX, "important"\)/,
  );
  assert.match(
    overlaySource,
    /document\.documentElement\.appendChild\(host\);[\s\S]*host\.style\.display = "block"/,
  );
  assert.match(
    overlayStyles,
    /:host \{[\s\S]*z-index:\s*2147483647 !important;/,
  );
});

test("all editable feature controls are locked while an operation is active", async () => {
  const source = await readFile(
    new URL("src/FeaturePolicyCard.tsx", root),
    "utf8",
  );

  assert.match(
    source,
    /className="gpu-mode-fieldset"\s*disabled=\{isMacClient \|\| isBusy\}/,
  );
  for (const setting of [
    "slimCodexPet",
    "fastCodexStartup",
    "fastContextTools",
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
    readFile(new URL("src/styles.css", root), "utf8"),
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
