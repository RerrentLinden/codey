import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

const source = await readFile(
  new URL("../public/windows-wmi-sampler-guard.js", import.meta.url),
  "utf8",
);
const cdpSource = await readFile(
  new URL("../backend/src/cdp.rs", import.meta.url),
  "utf8",
);

function createRuntime({
  platform = "Win32",
  sampler = {
    enabled: true,
    installed: true,
    workerWrapperPatched: true,
    blocked: 0,
    observationMs: 1_000,
  },
} = {}) {
  let currentSampler = sampler;
  const events = [];
  const requests = [];
  const window = {
    navigator: {
      platform,
      userAgent: platform === "Win32" ? "Windows" : "Macintosh",
    },
    __codeyInjectionStatus: {
      "windows-wmi-sampler": {
        status: "pending",
        detail: null,
        error: null,
      },
    },
    electronBridge: {
      sendMessageFromView(message) {
        requests.push(message);
        return Promise.resolve({
          status: "ok",
          sampler: { ...currentSampler },
        });
      },
    },
    dispatchEvent(event) {
      events.push(event);
      return true;
    },
  };
  window.window = window;
  const context = {
    CustomEvent: class CustomEvent {
      constructor(type, init = {}) {
        this.type = type;
        this.detail = init.detail;
      }
    },
    console,
    window,
  };
  vm.runInNewContext(source, context);
  return {
    events,
    requests,
    setSampler(next) {
      currentSampler = next;
    },
    async flush() {
      await Promise.resolve();
      await Promise.resolve();
      await new Promise((resolve) => setImmediate(resolve));
      await Promise.resolve();
      await Promise.resolve();
    },
    window,
  };
}

test("WMI sampler guard is registered as an independently probed CDP script", () => {
  assert.match(
    cdpSource,
    /include_str!\("\.\.\/\.\.\/dist-overlay\/inject\/windows-wmi-sampler-guard\.js"\)/,
  );
  assert.match(
    cdpSource,
    /"windows-wmi-sampler",\s*"Windows WMI 周期采样保护",\s*WINDOWS_WMI_SAMPLER_GUARD_SCRIPT/,
  );
  assert.match(cdpSource, /window\.__codeyWindowsWmiSamplerGuard/);
  assert.doesNotMatch(source, /setInterval/);
});

test("WMI sampler guard distinguishes installation from an actual blocked sample", async () => {
  const runtime = createRuntime();
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  assert.equal(entry.status, "executed");
  assert.match(entry.detail, /已安装，等待首次采样确认/);
  assert.equal(runtime.requests[0].type, "codey-windows-wmi-sampler-status");
  assert.equal(
    runtime.window.__codeyWindowsWmiSamplerGuard.snapshot().confirmed,
    false,
  );

  runtime.setSampler({
    enabled: true,
    installed: true,
    workerWrapperPatched: true,
    blocked: 3,
    observationMs: 31_000,
    lastMatchReason: "source-signature",
  });
  runtime.window.__codeyWindowsWmiSamplerGuard.requestProbe();
  await runtime.flush();

  assert.equal(entry.status, "effective");
  assert.match(entry.detail, /已阻止 3 次/);
  assert.match(entry.detail, /源码特征识别/);
  assert.equal(
    runtime.window.__codeyWindowsWmiSamplerGuard.snapshot().confirmed,
    true,
  );
});

test("WMI sampler guard keeps an unmatched observation window unverified", async () => {
  const runtime = createRuntime({
    sampler: {
      enabled: true,
      installed: true,
      workerWrapperPatched: true,
      blocked: 0,
      observationMs: 46_000,
      sourceInspections: 2,
      sourceSignatureMisses: 2,
    },
  });
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  assert.equal(entry.status, "executed");
  assert.match(entry.detail, /已检查 2 个 Worker/);
  assert.match(entry.detail, /当前来源尚未被识别/);
  assert.doesNotMatch(entry.detail, /已修复/);
  assert.equal(
    runtime.window.__codeyWindowsWmiSamplerGuard.snapshot().confirmed,
    false,
  );
});

test("WMI sampler guard stays unverified when a Worker source could not be read", async () => {
  const runtime = createRuntime({
    sampler: {
      enabled: true,
      installed: true,
      workerWrapperPatched: true,
      blocked: 0,
      observationMs: 60_000,
      sourceReadFailures: 1,
    },
  });
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  assert.equal(entry.status, "executed");
  assert.match(entry.detail, /1 个 Worker 源码无法检查/);
  assert.equal(
    runtime.window.__codeyWindowsWmiSamplerGuard.snapshot().confirmed,
    false,
  );
});
