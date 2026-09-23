import assert from "node:assert/strict";
import test from "node:test";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const { installOverlayTheme } = await loadTypeScriptModule(new URL("../src/overlayTheme.ts", import.meta.url));

function fixture(systemDark = false) {
  const element = () => ({
    attributes: {}, dataset: {}, style: {}, scheme: "normal",
    getAttribute(name) { return this.attributes[name] ?? null; },
    classList: { values: new Set(), contains(value) { return this.values.has(value); } },
  });
  const preference = {
    matches: systemDark, listeners: new Set(),
    addEventListener(type, callback) { assert.equal(type, "change"); this.listeners.add(callback); },
    removeEventListener(type, callback) { assert.equal(type, "change"); this.listeners.delete(callback); },
    change(matches) { this.matches = matches; for (const callback of this.listeners) callback(); },
  };
  let observer;
  const document = {
    documentElement: element(), body: element(), root: element(),
    getElementById(id) { assert.equal(id, "root"); return this.root; },
    defaultView: {
      matchMedia(query) { assert.equal(query, "(prefers-color-scheme: dark)"); return preference; },
      getComputedStyle(node) { return { colorScheme: node.scheme }; },
      MutationObserver: class {
        targets = new Map();
        constructor(callback) { this.callback = callback; observer = this; }
        observe(target, options) { this.targets.set(target, options); }
        disconnect() { this.targets.clear(); }
      },
    },
  };
  const containers = [element(), element()];
  const start = () => installOverlayTheme(containers, document);
  const mutate = (target, attributeName = "class", type = "attributes") => {
    const options = observer.targets.get(target);
    if (options && (type === "childList" ? options.childList : options.attributeFilter.includes(attributeName))) {
      observer.callback([{ target, type, attributeName }]);
    }
  };
  const expectTheme = (theme) => {
    for (const container of containers) {
      assert.equal(container.dataset.theme, theme);
      assert.equal(container.style.colorScheme, theme);
    }
  };
  return { document, preference, containers, element, start, mutate, expectTheme, observer: () => observer };
}

test("initial theme follows Codex markers before render and overrides the system preference", () => {
  const f = fixture(true);
  f.document.documentElement.attributes["data-theme"] = "light";
  f.start();
  f.expectTheme("light");
  f.document.documentElement.attributes["data-theme"] = "dark";
  f.mutate(f.document.documentElement, "data-theme");
  f.expectTheme("dark");
  f.preference.change(false);
  f.expectTheme("dark");
});

test("host classes and resolved color scheme synchronize both containers", () => {
  const f = fixture();
  f.document.body.classList.values.add("dark");
  f.start();
  f.expectTheme("dark");
  f.document.body.classList.values.clear();
  f.document.root.classList.values.add("light");
  f.mutate(f.document.root);
  f.expectTheme("light");
  f.document.root.classList.values.clear();
  f.document.documentElement.scheme = "only dark";
  f.mutate(f.document.documentElement, "style");
  f.expectTheme("dark");
  f.document.documentElement.scheme = "normal";
  f.document.body.scheme = "light";
  f.mutate(f.document.body, "style");
  f.expectTheme("light");
});

test("system changes apply only when the host has no resolved theme", () => {
  const f = fixture();
  f.document.documentElement.attributes["data-theme"] = "system";
  f.document.documentElement.scheme = "light dark";
  const controller = f.start();
  f.expectTheme("light");
  f.preference.change(true);
  f.expectTheme("dark");
  f.document.documentElement.scheme = "light";
  f.preference.change(false);
  f.preference.change(true);
  f.expectTheme("light");
  controller.dispose();
  assert.equal(f.preference.listeners.size, 0);
  assert.equal(f.observer().targets.size, 0);
});

test("child updates that keep the same theme roots do not recompute the theme", () => {
  const f = fixture();
  f.document.documentElement.scheme = "dark";
  let reads = 0;
  const original = f.document.defaultView.getComputedStyle;
  f.document.defaultView.getComputedStyle = (node) => {
    reads += 1;
    return original(node);
  };
  f.start();
  assert.ok(reads > 0);
  const afterStart = reads;
  f.mutate(f.document.body, undefined, "childList");
  f.mutate(f.document.documentElement, undefined, "childList");
  assert.equal(reads, afterStart);
  f.expectTheme("dark");
});

test("reopening refreshes immediately and a replaced host root remains observed", () => {
  const f = fixture();
  const controller = f.start();
  f.document.documentElement.attributes["data-theme"] = "dark";
  // 模拟关闭期间改变主题后立即重开，MutationObserver 尚未派发。
  controller.sync();
  f.expectTheme("dark");
  delete f.document.documentElement.attributes["data-theme"];
  const oldRoot = f.document.root;
  f.document.root = f.element();
  f.document.root.classList.values.add("dark");
  f.mutate(f.document.body, undefined, "childList");
  assert.equal(f.observer().targets.has(oldRoot), false);
  f.document.root.classList.values.delete("dark");
  f.document.root.classList.values.add("light");
  f.mutate(f.document.root);
  f.expectTheme("light");
  for (const options of f.observer().targets.values()) assert.notEqual(options.subtree, true);
});
