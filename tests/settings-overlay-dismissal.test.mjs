import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("settings Semi modal dismissal restores unsaved config", async () => {
  const [appSource, overlaySource] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/overlay.tsx", root), "utf8"),
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
    /<SemiModal[\s\S]*closeOnEsc[\s\S]*closable[\s\S]*maskClosable[\s\S]*onCancel=\{onCancel\}/,
  );
  assert.match(appSource, /onCancel=\{handleCloseSettings\}/);
  assert.match(
    appSource,
    /header=\{\([\s\S]*codey-settings-modal-header[\s\S]*configHeaderContent/,
  );
  assert.match(appSource, /aria-label="关闭配置"/);
  assert.match(
    appSource,
    /\{!embedded && \(\s*<header className="config-header">\{configHeaderContent\}<\/header>/,
  );
  assert.match(appSource, /\{!embedded && \(/);

  assert.doesNotMatch(overlaySource, /codey-overlay-backdrop/);
  assert.doesNotMatch(overlaySource, /codey-overlay-dialog/);
  assert.match(overlaySource, /modalVisible=\{visible\}/);
  assert.match(overlaySource, /onClose=\{close\}/);
  assert.match(overlaySource, /codey-settings-opened/);
  assert.match(overlaySource, /toggle: open/);
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
