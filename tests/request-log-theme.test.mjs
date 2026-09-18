import assert from "node:assert/strict";
import test from "node:test";
import ts from "typescript";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
import { readSource } from "./helpers/read-source.mjs";

const { installRequestLogTheme } = await loadTypeScriptModule(new URL("../src/requestLogTheme.ts", import.meta.url));

function fixture(search, systemDark) {
  const listeners = new Set();
  const preference = {
    matches: systemDark,
    addEventListener(type, callback) { assert.equal(type, "change"); listeners.add(callback); },
    removeEventListener(type, callback) { assert.equal(type, "change"); listeners.delete(callback); },
  };
  const document = {
    documentElement: { dataset: {}, style: {} },
    defaultView: {
      location: { search },
      matchMedia(query) { assert.equal(query, "(prefers-color-scheme: dark)"); return preference; },
    },
  };
  return {
    document,
    listeners,
    change(matches) {
      preference.matches = matches;
      for (const listener of listeners) listener();
    },
    expectTheme(theme) {
      assert.equal(document.documentElement.dataset.theme, theme);
      assert.equal(document.documentElement.style.colorScheme, theme);
    },
  };
}

test("request logs use the opening Codex theme even when the browser system theme differs", () => {
  for (const theme of ["light", "dark"]) {
    const f = fixture(`?theme=${theme}`, theme === "light");
    installRequestLogTheme(f.document);
    f.expectTheme(theme);
    f.change(theme === "dark");
    f.change(theme === "light");
    f.expectTheme(theme);
    assert.equal(f.listeners.size, 0);
  }
});

test("old and invalid log links follow system changes without locking to their first theme", () => {
  for (const search of ["", "?theme=system", "?theme=untrusted"]) {
    const f = fixture(search, true);
    const dispose = installRequestLogTheme(f.document);
    f.expectTheme("dark");
    f.change(false);
    f.expectTheme("light");
    f.change(true);
    f.expectTheme("dark");
    dispose();
    assert.equal(f.listeners.size, 0);
  }
});

test("removing the request log token from the URL preserves the theme on reload", async () => {
  const overlay = await readSource("src/overlay.tsx");
  const source = overlay.match(/function installBrowserBridge\(\) \{[\s\S]*?\n\}/)?.[0];
  assert.ok(source);
  const compiled = ts.transpileModule(source, {}).outputText;
  const tokenKey = "test-request-log-token";
  const values = new Map();
  const window = {
    location: { pathname: "/codey/request-logs", search: "?theme=dark", hash: "#test%2Dtoken" },
    sessionStorage: {
      setItem: (key, value) => values.set(key, value),
      getItem: (key) => values.get(key),
    },
    history: {
      replaceState(_state, _title, url) {
        assert.equal(url, "/codey/request-logs?theme=dark");
        window.location.hash = "";
      },
    },
  };
  const install = new Function("window", "REQUEST_LOG_TOKEN_KEY", `${compiled}; return installBrowserBridge;`)(window, tokenKey);
  install();
  assert.equal(values.get(tokenKey), "test-token");
  assert.equal(window.location.hash, "");
  install();
  assert.equal(window.location.search, "?theme=dark");
  assert.equal(typeof window.__codeyInvokeApi, "function");
});
