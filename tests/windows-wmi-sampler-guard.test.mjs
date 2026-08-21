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
  statusTransport = "return",
  sampler = {
    version: 4,
    enabled: true,
    installed: true,
    workerWrapperPatched: true,
    selfTestPassed: true,
    blocked: 0,
    observationMs: 1_000,
  },
} = {}) {
  let currentSampler = sampler;
  const events = [];
  const requests = [];
  const messageListeners = new Set();
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
    addEventListener(type, listener) {
      if (type === "message") messageListeners.add(listener);
    },
    clearTimeout,
    electronBridge: {
      sendMessageFromView(message) {
        requests.push(message);
        const response = {
          status: "ok",
          sampler: { ...currentSampler },
        };
        if (statusTransport === "event") {
          window.dispatchEvent({
            type: "message",
            data: {
              type: "codey-windows-wmi-sampler-status-response",
              requestId: message.requestId,
              ...response,
            },
          });
          return Promise.resolve(undefined);
        }
        if (statusTransport === "none") return Promise.resolve(undefined);
        return Promise.resolve(response);
      },
    },
    removeEventListener(type, listener) {
      if (type === "message") messageListeners.delete(listener);
    },
    setTimeout,
    dispatchEvent(event) {
      events.push(event);
      if (event.type === "message") {
        for (const listener of [...messageListeners]) listener(event);
      }
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
  assert.match(cdpSource, /snapshot\.selfTestConfirmed === true/);
  assert.doesNotMatch(source, /setInterval/);
});

test("WMI sampler guard confirms a complete self-test and reports actual blocks separately", async () => {
  const runtime = createRuntime();
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  assert.equal(entry.status, "effective");
  assert.match(entry.detail, /完整自检通过/);
  assert.match(entry.detail, /尚未触发实际 WMI 采样/);
  assert.equal(runtime.requests[0].type, "codey-windows-wmi-sampler-status");
  assert.equal(
    runtime.window.__codeyWindowsWmiSamplerGuard.snapshot().confirmed,
    true,
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

test("WMI sampler guard does not trust a legacy self-test as complete confirmation", async () => {
  const runtime = createRuntime({
    sampler: {
      version: 3,
      enabled: true,
      installed: true,
      workerWrapperPatched: true,
      selfTestPassed: true,
      blocked: 0,
      observationMs: 1_000,
    },
  });
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  const snapshot = runtime.window.__codeyWindowsWmiSamplerGuard.snapshot();
  assert.equal(entry.status, "executed");
  assert.match(entry.detail, /旧版/);
  assert.equal(snapshot.selfTestConfirmed, false);
  assert.equal(snapshot.confirmed, false);
});

test("WMI sampler guard verifies a no-return preload through a renderer event", async () => {
  const runtime = createRuntime({
    statusTransport: "event",
    sampler: {
      enabled: true,
      installed: true,
      workerWrapperPatched: true,
      blocked: 3,
      observationMs: 31_000,
    },
  });
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  const snapshot = runtime.window.__codeyWindowsWmiSamplerGuard.snapshot();
  assert.equal(entry.status, "effective");
  assert.match(entry.detail, /已阻止 3 次/);
  assert.equal(snapshot.installed, true);
  assert.equal(snapshot.confirmed, true);
  assert.equal(snapshot.probeTransport, "renderer-event");
  assert.equal(runtime.requests.length, 1);
});

test("WMI sampler guard keeps an unmatched observation window unverified", async () => {
  const runtime = createRuntime({
    sampler: {
      enabled: true,
      installed: true,
      workerWrapperPatched: true,
      selfTestPassed: false,
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

test("WMI sampler complete self-test is not downgraded by an unmatched observation", async () => {
  const runtime = createRuntime({
    sampler: {
      version: 4,
      enabled: true,
      installed: true,
      workerWrapperPatched: true,
      selfTestPassed: true,
      blocked: 0,
      observationMs: 46_000,
      workersObserved: 1,
      sourceInspections: 1,
      sourceSignatureMisses: 1,
      lastObservedWorkerName: "worker.js",
      lastObservedThreadName: "",
      lastObservedSourceSignals: ["workerMessaging"],
    },
  });
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  assert.equal(entry.status, "effective");
  assert.match(entry.detail, /完整自检通过/);
  assert.match(entry.detail, /已检查 1 个 Worker/);
  assert.match(entry.detail, /尚未观察到实际 WMI 采样/);
  assert.equal(
    runtime.window.__codeyWindowsWmiSamplerGuard.snapshot().confirmed,
    true,
  );
});

test("WMI sampler complete self-test keeps source-read diagnostics without losing confirmation", async () => {
  const runtime = createRuntime({
    sampler: {
      version: 4,
      enabled: true,
      installed: true,
      workerWrapperPatched: true,
      selfTestPassed: true,
      blocked: 0,
      observationMs: 60_000,
      sourceReadFailures: 1,
    },
  });
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  assert.equal(entry.status, "effective");
  assert.match(entry.detail, /完整自检通过/);
  assert.match(entry.detail, /1 个 Worker 源码无法检查/);
  assert.equal(
    runtime.window.__codeyWindowsWmiSamplerGuard.snapshot().confirmed,
    true,
  );
});

test("WMI sampler guard surfaces a failed one-shot self-test", async () => {
  const runtime = createRuntime({
    sampler: {
      enabled: true,
      installed: true,
      workerWrapperPatched: true,
      selfTestPassed: false,
      selfTestError: "probe was not intercepted",
      blocked: 0,
      observationMs: 1_000,
    },
  });
  await runtime.flush();

  const entry =
    runtime.window.__codeyInjectionStatus["windows-wmi-sampler"];
  assert.equal(entry.status, "failed");
  assert.match(entry.detail, /自检失败/);
  assert.match(entry.detail, /probe was not intercepted/);
  assert.equal(
    runtime.window.__codeyWindowsWmiSamplerGuard.snapshot().confirmed,
    false,
  );
});
