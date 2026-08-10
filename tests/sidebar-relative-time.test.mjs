import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../public/codey-inject.js", import.meta.url), "utf8");
const vendorSource = readFileSync(
  new URL("../vendor/CodeyRuntime/assets/inject/renderer-inject.js", import.meta.url),
  "utf8",
);

class FakeElement {
  constructor(tagName = "div") {
    this.attributes = new Map();
    this.children = [];
    this.className = "";
    this.parentElement = null;
    this.tagName = String(tagName).toUpperCase();
    this.textContent = "";
    this.title = "";
    this.attributeWrites = 0;
    const styleValues = new Map();
    this.style = {
      getPropertyValue: (name) => styleValues.get(name) || "",
      removeProperty: (name) => styleValues.delete(name),
      setProperty: (name, value) => styleValues.set(name, String(value)),
    };
  }

  appendChild(child) {
    if (child.parentElement) {
      child.parentElement.children = child.parentElement.children.filter(
        (candidate) => candidate !== child,
      );
    }
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  insertBefore(child, reference) {
    if (child.parentElement) {
      child.parentElement.children = child.parentElement.children.filter(
        (candidate) => candidate !== child,
      );
    }
    const referenceIndex = this.children.indexOf(reference);
    if (referenceIndex < 0) return this.appendChild(child);
    child.parentElement = this;
    this.children.splice(referenceIndex, 0, child);
    return child;
  }

  getAttribute(name) {
    return this.attributes.get(name) ?? null;
  }

  hasAttribute(name) {
    return this.attributes.has(name);
  }

  matches(selector) {
    if (/^[a-z]+$/i.test(selector)) return this.tagName.toLowerCase() === selector.toLowerCase();
    const classContains = selector.match(/^\[class\*=(['"]?)([^\]'"]+)\1\]$/)?.[2];
    if (classContains) return String(this.className || "").includes(classContains);
    const attribute = selector.match(/^\[([^\]]+)\]$/)?.[1];
    return attribute ? this.hasAttribute(attribute) : false;
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }

  querySelectorAll(selector) {
    const selectors = selector.split(",").map((candidate) => candidate.trim());
    const matches = [];
    const visit = (node) => {
      node.children.forEach((child) => {
        if (selectors.some((candidate) => child.matches(candidate))) matches.push(child);
        visit(child);
      });
    };
    visit(this);
    return matches;
  }

  closest(selector) {
    const selectors = selector.split(",").map((candidate) => candidate.trim());
    let current = this;
    while (current) {
      if (selectors.some((candidate) => current.matches(candidate))) return current;
      current = current.parentElement;
    }
    return null;
  }

  remove() {
    if (!this.parentElement) return;
    this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
    this.parentElement = null;
  }

  removeAttribute(name) {
    this.attributes.delete(name);
  }

  setAttribute(name, value) {
    this.attributeWrites += 1;
    this.attributes.set(name, String(value));
  }
}

function loadInjection({
  bridgeHandler,
  now,
  rows = [],
  signalDispatcher,
} = {}) {
  const placeholder = new FakeElement();
  const intervalCallbacks = [];
  let mutationCallback = null;
  let nowMs = Number.isFinite(now) ? now : Date.now();
  class FakeDate extends Date {
    constructor(...args) {
      super(...(args.length ? args : [nowMs]));
    }

    static now() {
      return nowMs;
    }
  }
  const document = {
    body: new FakeElement("body"),
    documentElement: new FakeElement("html"),
    visibilityState: "visible",
    addEventListener() {},
    createElement: (tagName) => new FakeElement(tagName),
    getElementById: () => placeholder,
    querySelector: () => null,
    threadRowQueries: 0,
    querySelectorAll(selector) {
      if (selector !== "[data-app-action-sidebar-thread-row]") return [];
      this.threadRowQueries += 1;
      return rows;
    },
  };
  const window = {
    __codexSessionDeleteBridge: bridgeHandler,
    __codeyCodexSignalDispatcher: signalDispatcher,
    addEventListener() {},
    clearTimeout() {},
    dispatchEvent() {},
    localStorage: { length: 0, key: () => null, getItem: () => null, setItem() {} },
    setInterval: (callback) => {
      intervalCallbacks.push(callback);
      return intervalCallbacks.length;
    },
    setTimeout: (callback) => {
      queueMicrotask(callback);
      return 1;
    },
  };
  window.window = window;
  const context = {
    console,
    document,
    HTMLElement: FakeElement,
    location: { pathname: "/", search: "" },
    MutationObserver: class {
      constructor(callback) {
        mutationCallback = callback;
      }
      observe() {}
    },
    URLSearchParams,
    window,
  };
  if (Number.isFinite(now)) context.Date = FakeDate;
  vm.runInNewContext(source, context);
  return {
    advanceTime: (milliseconds) => {
      nowMs += milliseconds;
    },
    document,
    notifyMutations: (mutations) => mutationCallback?.(mutations),
    runIntervals: () => intervalCallbacks.forEach((callback) => callback()),
    window,
  };
}

function sidebarThreadEntry({ running = false, sessionId = "" } = {}) {
  const list = new FakeElement();
  list.setAttribute("role", "list");
  const item = new FakeElement();
  item.setAttribute("role", "listitem");
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  if (sessionId) {
    row.setAttribute("data-app-action-sidebar-thread-id", sessionId);
    row.setAttribute("data-app-action-sidebar-thread-title", sessionId);
  }
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  const titleRegion = new FakeElement();
  titleRegion.className = "flex min-w-0 flex-1 items-center gap-2";
  const nativeStatusRail = new FakeElement();
  nativeStatusRail.className = "ml-[3px] flex items-center justify-end gap-1";
  const spinner = new FakeElement();
  spinner.className = "animate-spin rounded-full";
  if (running) nativeStatusRail.appendChild(spinner);
  content.appendChild(titleRegion);
  content.appendChild(nativeStatusRail);
  row.appendChild(content);
  item.appendChild(row);
  list.appendChild(item);
  return {
    content,
    item,
    list,
    nativeStatusRail,
    row,
    spinner,
  };
}

test("formats compact relative times for the sidebar", () => {
  const { window } = loadInjection();
  const now = Date.UTC(2026, 6, 21, 12);
  const format = window.__codeyFormatRelativeThreadTime;

  assert.equal(format(now - 59_000, now), "刚刚");
  assert.equal(format(now - 3 * 60_000, now), "3 分");
  assert.equal(format(now - 3 * 60 * 60_000, now), "3 小时");
  assert.equal(format(now - 2 * 24 * 60 * 60_000, now), "2 天");
  assert.equal(format(now - 14 * 24 * 60 * 60_000, now), "2 周");
  assert.equal(format(now - 45 * 24 * 60 * 60_000, now), "1 月");
  assert.equal(format(now - 360 * 24 * 60 * 60_000, now), "12 月");
  assert.equal(format(now - 400 * 24 * 60 * 60_000, now), "1 年");
});

test("normalizes Codex timestamp payload variants to milliseconds", () => {
  const { window } = loadInjection();
  const timestampFrom = window.__codeyThreadTimestampMsFromPayload;

  assert.equal(timestampFrom({ recency_at_ms: 222_333, updated_at_ms: 123_456 }), 222_333);
  assert.equal(timestampFrom({ recency_at: 123, updated_at_ms: 456_789 }), 123_000);
  assert.equal(timestampFrom({ createdAtMs: 456_789 }), 456_789);
  assert.equal(timestampFrom({ updatedAt: 123, createdAt: 45 }), 123_000);
  assert.equal(
    timestampFrom({ id: "019f948c-dba4-73c0-83e3-804e6ad6a5be" }),
    1_784_903_687_076,
  );
  assert.equal(timestampFrom({ updated_at: 123 }), 123_000);
});

test("renders an accessible time element in the thread row content", () => {
  const { window } = loadInjection();
  const row = new FakeElement();
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  const titleRegion = new FakeElement();
  titleRegion.className = "flex min-w-0 flex-1 items-center gap-2";
  const nativeStatusRail = new FakeElement();
  nativeStatusRail.className = "ml-[3px] flex items-center justify-end gap-1";
  const nativeActionSpacer = new FakeElement();
  nativeActionSpacer.className = "shrink-0";
  content.appendChild(titleRegion);
  content.appendChild(nativeStatusRail);
  content.appendChild(nativeActionSpacer);
  row.appendChild(content);
  const timestamp = Date.now() - 2 * 24 * 60 * 60_000;

  window.__codeyUpdateThreadUpdatedAt(row, timestamp);

  const label = content.querySelector("[data-codey-thread-updated-at]");
  assert.ok(label);
  assert.equal(label.textContent, "2 天");
  assert.equal(label.getAttribute("data-codey-thread-updated-at-ms"), String(timestamp));
  assert.match(label.getAttribute("datetime"), /^\d{4}-\d{2}-\d{2}T/);
  assert.match(label.getAttribute("aria-label"), /^最后消息：2 天/);
  assert.match(label.title, /^最后消息：/);
  assert.deepEqual(
    content.children,
    [titleRegion, label, nativeStatusRail, nativeActionSpacer],
  );

  const attributeWrites = label.attributeWrites;
  window.__codeyUpdateThreadUpdatedAt(row, timestamp);
  assert.equal(label.attributeWrites, attributeWrites);

  window.__codeyUpdateThreadUpdatedAt(row, 0);
  assert.equal(content.querySelector("[data-codey-thread-updated-at]"), null);
});

test("hides thread time while the native status rail is occupied", () => {
  const { window } = loadInjection();
  const row = new FakeElement();
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  const titleRegion = new FakeElement();
  titleRegion.className = "flex min-w-0 flex-1 items-center gap-2";
  const nativeStatusRail = new FakeElement();
  nativeStatusRail.className = "ml-[3px] flex items-center justify-end gap-1";
  const runningStatus = new FakeElement();
  runningStatus.className = "animate-spin rounded-full";
  const nativeActionSpacer = new FakeElement();
  nativeActionSpacer.className = "shrink-0";
  content.appendChild(titleRegion);
  nativeStatusRail.appendChild(runningStatus);
  content.appendChild(nativeStatusRail);
  content.appendChild(nativeActionSpacer);
  row.appendChild(content);
  const timestamp = Date.now() - 5 * 60_000;

  window.__codeyUpdateThreadUpdatedAt(row, timestamp);

  assert.equal(content.querySelector("[data-codey-thread-updated-at]"), null);

  runningStatus.remove();
  window.__codeyUpdateThreadUpdatedAt(row, timestamp);

  const label = content.querySelector("[data-codey-thread-updated-at]");
  assert.ok(label);
  assert.equal(label.textContent, "5 分");
  assert.deepEqual(
    content.children,
    [titleRegion, label, nativeStatusRail, nativeActionSpacer],
  );
});

test("hides thread time from Codex React loading and unread status state", () => {
  const { window } = loadInjection();
  const row = new FakeElement();
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  const titleRegion = new FakeElement();
  titleRegion.className = "flex min-w-0 flex-1 items-center gap-2";
  const nativeStatusRail = new FakeElement();
  nativeStatusRail.className = "ml-[3px] flex items-center justify-end gap-1";
  content.appendChild(titleRegion);
  content.appendChild(nativeStatusRail);
  row.appendChild(content);
  const timestamp = Date.now() - 6 * 60_000;
  const statusFiber = {
    memoizedProps: { statusState: { type: "loading", unread: false } },
    return: null,
  };
  row.__reactFiber$test = { memoizedProps: {}, return: statusFiber };

  window.__codeyUpdateThreadUpdatedAt(row, timestamp);
  assert.equal(content.querySelector("[data-codey-thread-updated-at]"), null);

  statusFiber.memoizedProps.statusState = { type: undefined, unread: true };
  window.__codeyUpdateThreadUpdatedAt(row, timestamp);
  assert.equal(content.querySelector("[data-codey-thread-updated-at]"), null);

  statusFiber.memoizedProps.statusState = { type: undefined, unread: false };
  window.__codeyUpdateThreadUpdatedAt(row, timestamp);
  assert.equal(content.querySelector("[data-codey-thread-updated-at]")?.textContent, "6 分");
});

test("leaves Codex-owned sidebar order untouched", async () => {
  const olderIdle = sidebarThreadEntry({
    sessionId: "thread-older",
  });
  const newerIdle = sidebarThreadEntry({ sessionId: "thread-newer" });
  const list = olderIdle.list;
  list.appendChild(newerIdle.item);
  loadInjection({
    rows: [olderIdle.row, newerIdle.row],
    signalDispatcher: async (signal, request) => {
      assert.equal(signal, "send-cli-request-for-host");
      assert.equal(request.method, "thread/list");
      return {
        data: [
        {
            id: "thread-older",
            recencyAt: 60,
        },
        {
            id: "thread-newer",
            recencyAt: 180,
        },
        ],
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(olderIdle.item.style.getPropertyValue("--codey-thread-sort-order"), "");
  assert.equal(newerIdle.item.style.getPropertyValue("--codey-thread-sort-order"), "");
  assert.equal(olderIdle.item.getAttribute("data-codey-thread-sort-order"), null);
  assert.equal(newerIdle.item.getAttribute("data-codey-thread-sort-order"), null);
  assert.deepEqual(list.children, [olderIdle.item, newerIdle.item]);
});

test("refreshes the official timestamp when a running thread completes", async () => {
  const running = sidebarThreadEntry({
    running: true,
    sessionId: "thread-running",
  });
  let dispatcherCalls = 0;
  const firstTimestamp = Date.now() - 60 * 60_000;
  const completedTimestamp = Date.now() - 2 * 60_000;
  const { window } = loadInjection({
    rows: [running.row],
    signalDispatcher: async () => {
      dispatcherCalls += 1;
      return {
        data: [{
          id: "thread-running",
          recencyAt: (
            dispatcherCalls === 1 ? firstTimestamp : completedTimestamp
          ) / 1_000,
        }],
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(dispatcherCalls, 1);
  assert.equal(running.content.querySelector("[data-codey-thread-updated-at]"), null);

  running.spinner.remove();
  window.__codeyInstallThreadUpdatedTimes(running.row);
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(dispatcherCalls, 2);
  assert.equal(
    running.content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "2 分",
  );
  assert.equal(running.item.getAttribute("data-codey-thread-sort-order"), null);
});

test("coalesces sidebar mutations before recomputing React status", async () => {
  const entry = sidebarThreadEntry({ sessionId: "thread-1" });
  let fiberReads = 0;
  let dispatcherCalls = 0;
  Object.defineProperty(entry.row, "__reactFiber$test", {
    configurable: true,
    enumerable: true,
    get() {
      fiberReads += 1;
      return { memoizedProps: {}, return: null };
    },
  });
  const { notifyMutations } = loadInjection({
    rows: [entry.row],
    signalDispatcher: async () => {
      dispatcherCalls += 1;
      return {
        data: [{ id: "thread-1", updatedAt: 60 }],
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(dispatcherCalls, 1);
  fiberReads = 0;

  entry.nativeStatusRail.appendChild(entry.spinner);
  notifyMutations(Array.from({ length: 100 }, () => ({
    type: "childList",
    target: entry.nativeStatusRail,
    addedNodes: [entry.spinner],
    removedNodes: [],
  })));

  assert.equal(fiberReads, 0);

  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(dispatcherCalls, 1);
  assert.equal(entry.item.getAttribute("data-codey-thread-sort-order"), null);
  assert.ok(
    fiberReads <= 2,
    `100 mutations should converge in one scan, observed ${fiberReads} React fiber reads`,
  );
});

test("keeps an existing thread time when a native completion marker appears", async () => {
  const timestamp = Date.now() - 2 * 60_000;
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "local:thread-1");
  row.setAttribute("data-app-action-sidebar-thread-title", "增加文本清洗 key 配置");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  const titleRegion = new FakeElement();
  titleRegion.className = "flex min-w-0 flex-1 items-center gap-2";
  const nativeStatusRail = new FakeElement();
  nativeStatusRail.className = "ml-[3px] flex items-center justify-end gap-1";
  const nativeActionSpacer = new FakeElement();
  nativeActionSpacer.className = "shrink-0";
  content.appendChild(titleRegion);
  content.appendChild(nativeStatusRail);
  content.appendChild(nativeActionSpacer);
  row.appendChild(content);

  const { window } = loadInjection({
    rows: [row],
    signalDispatcher: async () => ({
      data: [{ id: "thread-1", recencyAt: timestamp / 1_000 }],
      nextCursor: null,
    }),
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(content.querySelector("[data-codey-thread-updated-at]")?.textContent, "2 分");

  const completedStatus = new FakeElement();
  completedStatus.className = "rounded-full bg-blue-500";
  completedStatus.setAttribute("aria-label", "Completed");
  nativeStatusRail.appendChild(completedStatus);
  window.__codeyInstallThreadUpdatedTimes(row);

  assert.equal(content.querySelector("[data-codey-thread-updated-at]")?.textContent, "2 分");

  const runningStatus = new FakeElement();
  runningStatus.className = "animate-spin rounded-full";
  nativeStatusRail.appendChild(runningStatus);
  window.__codeyInstallThreadUpdatedTimes(row);
  assert.equal(content.querySelector("[data-codey-thread-updated-at]"), null);

  runningStatus.remove();
  window.__codeyInstallThreadUpdatedTimes(row);
  assert.equal(content.querySelector("[data-codey-thread-updated-at]")?.textContent, "2 分");
});

test("does not treat trailing action icons as native thread status", () => {
  const { window } = loadInjection();
  const row = new FakeElement();
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  const titleRegion = new FakeElement();
  titleRegion.className = "flex min-w-0 flex-1 items-center gap-2";
  const nativeStatusRail = new FakeElement();
  nativeStatusRail.className = "ml-[3px] flex items-center justify-end gap-1";
  const nativeActionSpacer = new FakeElement();
  nativeActionSpacer.className = "shrink-0";
  const actionButton = new FakeElement("button");
  const actionIcon = new FakeElement("svg");
  actionButton.appendChild(actionIcon);
  nativeActionSpacer.appendChild(actionButton);
  content.appendChild(titleRegion);
  content.appendChild(nativeStatusRail);
  content.appendChild(nativeActionSpacer);
  row.appendChild(content);
  const timestamp = Date.now() - 9 * 60_000;

  window.__codeyUpdateThreadUpdatedAt(row, timestamp);

  assert.equal(content.querySelector("[data-codey-thread-updated-at]")?.textContent, "9 分");
});

test("moves a previously appended time before the native trailing rail", () => {
  const { window } = loadInjection();
  const row = new FakeElement();
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center text-sm leading-4";
  const titleRegion = new FakeElement();
  titleRegion.className = "flex min-w-0 flex-1 items-center gap-2";
  const nativeStatusRail = new FakeElement();
  nativeStatusRail.className = "ml-[3px] flex items-center justify-end gap-1";
  const nativeActionSpacer = new FakeElement();
  nativeActionSpacer.className = "shrink-0";
  const misplacedLabel = new FakeElement("time");
  misplacedLabel.setAttribute("data-codey-thread-updated-at", "true");
  const duplicateLabel = new FakeElement("time");
  duplicateLabel.setAttribute("data-codey-thread-updated-at", "true");
  content.appendChild(titleRegion);
  content.appendChild(nativeStatusRail);
  content.appendChild(nativeActionSpacer);
  content.appendChild(misplacedLabel);
  content.appendChild(duplicateLabel);
  row.appendChild(content);

  window.__codeyUpdateThreadUpdatedAt(row, Date.now() - 12 * 60_000);

  assert.deepEqual(
    content.children,
    [titleRegion, misplacedLabel, nativeStatusRail, nativeActionSpacer],
  );
  assert.equal(
    content.querySelectorAll("[data-codey-thread-updated-at]").length,
    1,
  );
  assert.equal(duplicateLabel.parentElement, null);
});

test("loads visible thread timestamps through the official app-server list", async () => {
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "local:thread-1");
  row.setAttribute("data-app-action-sidebar-thread-title", "发布计划");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  row.appendChild(content);
  const calls = [];
  const timestamp = Date.now() - 3 * 60 * 60_000;

  const { document } = loadInjection({
    rows: [row],
    signalDispatcher: async (signal, request) => {
      calls.push({ signal, request });
      return {
        data: [{
          id: "thread-1",
          recencyAt: timestamp / 1_000,
          updatedAt: Date.now() / 1_000,
        }],
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(calls.length, 1);
  assert.equal(calls[0].signal, "send-cli-request-for-host");
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].request)), {
    hostId: "local",
    method: "thread/list",
    params: {
      archived: false,
      cursor: null,
      limit: 100,
      modelProviders: null,
      useStateDbOnly: true,
    },
    priority: "background",
    source: "thread_list",
  });
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "3 小时",
  );
  assert.equal(document.threadRowQueries, 1);
});

test("reads a remote task timestamp from the official React row without a local request", async () => {
  const now = Date.UTC(2026, 7, 10, 12);
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-host-id", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "remote:task-1");
  row.setAttribute("data-app-action-sidebar-thread-kind", "remote");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  row.appendChild(content);
  row.__reactFiber$test = {
    memoizedProps: {
      task: {
        created_at: (now - 60 * 60_000) / 1_000,
        id: "task-1",
        updated_at: (now - 6 * 60_000) / 1_000,
      },
    },
    pendingProps: null,
    return: null,
  };
  let dispatcherCalls = 0;

  loadInjection({
    now,
    rows: [row],
    signalDispatcher: async () => {
      dispatcherCalls += 1;
      throw new Error("remote tasks must not use the local app-server");
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(dispatcherCalls, 0);
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "6 分",
  );
});

test("refreshes official metadata on the visible one-minute tick", async () => {
  const now = Date.UTC(2026, 7, 10, 12);
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "thread-refresh");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  row.appendChild(content);
  let timestamp = now - 10 * 60_000;
  let listCalls = 0;
  const fixture = loadInjection({
    now,
    rows: [row],
    signalDispatcher: async () => {
      listCalls += 1;
      return {
        data: [{ id: "thread-refresh", recencyAt: timestamp / 1_000 }],
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(listCalls, 1);

  fixture.advanceTime(60_000);
  timestamp = now + 60_000 - 2 * 60_000;
  fixture.runIntervals();
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(listCalls, 2);
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "2 分",
  );
});

test("follows official thread list cursors until a visible thread is found", async () => {
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "thread-older");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  row.appendChild(content);
  const cursors = [];
  const timestamp = Date.now() - 4 * 60 * 60_000;

  loadInjection({
    rows: [row],
    signalDispatcher: async (_signal, request) => {
      cursors.push(request.params.cursor);
      if (request.params.cursor == null) {
        return {
          data: [{ id: "thread-newer", recencyAt: Date.now() / 1_000 }],
          nextCursor: "page-2",
        };
      }
      return {
        data: [{ id: "thread-older", recencyAt: timestamp / 1_000 }],
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(cursors, [null, "page-2"]);
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "4 小时",
  );
});

test("bounds thread list pagination and falls back to an exact thread read", async () => {
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "thread-manual");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  row.appendChild(content);
  const timestamp = Date.now() - 5 * 60 * 60_000;
  let listCalls = 0;
  let readCalls = 0;

  loadInjection({
    rows: [row],
    signalDispatcher: async (_signal, request) => {
      if (request.method === "thread/read") {
        readCalls += 1;
        return {
          thread: {
            id: "thread-manual",
            recencyAt: timestamp / 1_000,
          },
        };
      }
      listCalls += 1;
      return {
        data: [{ id: `unrelated-${listCalls}`, recencyAt: Date.now() / 1_000 }],
        nextCursor: `page-${listCalls + 1}`,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(listCalls, 5);
  assert.equal(readCalls, 1);
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "5 小时",
  );
});

test("drains exact timestamp reads beyond the 32-item batch size", async () => {
  const now = Date.UTC(2026, 7, 10, 12);
  const entries = Array.from({ length: 40 }, (_, index) => {
    const row = new FakeElement();
    row.setAttribute("data-app-action-sidebar-thread-row", "");
    row.setAttribute("data-app-action-sidebar-thread-id", `thread-old-${index}`);
    const content = new FakeElement();
    content.className = "flex h-full w-full items-center";
    row.appendChild(content);
    return { content, row };
  });
  let listCalls = 0;
  let readCalls = 0;

  loadInjection({
    now,
    rows: entries.map(({ row }) => row),
    signalDispatcher: async (_signal, request) => {
      if (request.method === "thread/read") {
        readCalls += 1;
        return {
          thread: {
            id: request.params.threadId,
            recencyAt: (now - 9 * 60_000) / 1_000,
          },
        };
      }
      listCalls += 1;
      return {
        data: [{ id: `unrelated-${listCalls}`, recencyAt: now / 1_000 }],
        nextCursor: `page-${listCalls + 1}`,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(listCalls, 5);
  assert.equal(readCalls, 40);
  entries.forEach(({ content }) => {
    assert.equal(
      content.querySelector("[data-codey-thread-updated-at]")?.textContent,
      "9 分",
    );
  });
});

test("continues timestamp work after the first 200 visible refs", async () => {
  const now = Date.UTC(2026, 7, 10, 12);
  const entries = Array.from({ length: 201 }, (_, index) => {
    const row = new FakeElement();
    row.setAttribute("data-app-action-sidebar-thread-row", "");
    row.setAttribute("data-app-action-sidebar-thread-id", `thread-${index}`);
    const content = new FakeElement();
    content.className = "flex h-full w-full items-center";
    row.appendChild(content);
    return { content, row };
  });
  let listCalls = 0;

  loadInjection({
    now,
    rows: entries.map(({ row }) => row),
    signalDispatcher: async () => {
      listCalls += 1;
      return {
        data: entries.map((_, index) => ({
          id: `thread-${index}`,
          recencyAt: (now - 11 * 60_000) / 1_000,
        })),
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(listCalls, 2);
  entries.forEach(({ content }) => {
    assert.equal(
      content.querySelector("[data-codey-thread-updated-at]")?.textContent,
      "11 分",
    );
  });
});

test("retries only a failed exact timestamp read", async () => {
  const now = Date.UTC(2026, 7, 10, 12);
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "thread-read-retry");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  row.appendChild(content);
  let listCalls = 0;
  let readCalls = 0;

  loadInjection({
    now,
    rows: [row],
    signalDispatcher: async (_signal, request) => {
      if (request.method === "thread/read") {
        readCalls += 1;
        if (readCalls === 1) throw new Error("temporary exact-read failure");
        return {
          thread: {
            id: "thread-read-retry",
            recencyAt: (now - 13 * 60_000) / 1_000,
          },
        };
      }
      listCalls += 1;
      return {
        data: [],
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(listCalls, 1);
  assert.equal(readCalls, 2);
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "13 分",
  );
});

test("retries a transient official timestamp request failure", async () => {
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "thread-retry");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  row.appendChild(content);
  const timestamp = Date.now() - 7 * 60_000;
  let calls = 0;

  loadInjection({
    rows: [row],
    signalDispatcher: async () => {
      calls += 1;
      if (calls === 1) throw new Error("temporary failure");
      return {
        data: [{ id: "thread-retry", recencyAt: timestamp / 1_000 }],
        nextCursor: null,
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(calls, 2);
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "7 分",
  );
});

test("clears a cached timestamp when official metadata no longer has one", async () => {
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
  row.setAttribute("data-app-action-sidebar-thread-id", "thread-cleared");
  const content = new FakeElement();
  content.className = "flex h-full w-full items-center";
  row.appendChild(content);
  const timestamp = Date.now() - 8 * 60_000;
  let includeTimestamp = true;

  const { window } = loadInjection({
    rows: [row],
    signalDispatcher: async () => ({
      data: [{
        id: "thread-cleared",
        recencyAt: includeTimestamp ? timestamp / 1_000 : null,
        updatedAt: null,
        createdAt: null,
      }],
      nextCursor: null,
    }),
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "8 分",
  );

  includeTimestamp = false;
  window.__codeyInstallThreadUpdatedTimes(row, true);
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(content.querySelector("[data-codey-thread-updated-at]"), null);
});

test("accepts only a unique direct app-server request wrapper", () => {
  const { window } = loadInjection();
  const direct = function request(action, payload, options) {
    return options == null
      ? signalClient.sendRequest(action, payload)
      : signalClient.sendRequest(action, payload, options);
  };
  const instanceMethod = function request(action, payload, options) {
    return this.requestClient.sendRequest(action, payload, options);
  };
  const secondDirect = function request(action, payload) {
    return otherSignalClient.sendRequest(action, payload);
  };

  assert.equal(
    window.__codeySignalDispatcherFromModule({ arbitrary: direct }, false),
    direct,
  );
  assert.equal(
    window.__codeySignalDispatcherFromModule({ O: instanceMethod }, true),
    null,
  );
  assert.equal(
    window.__codeySignalDispatcherFromModule({
      arbitrary: direct,
      another: secondDirect,
    }, false),
    null,
  );
  assert.equal(
    window.__codeySignalDispatcherFromModule({
      O: direct,
      another: secondDirect,
    }, false),
    null,
  );
});

test("injects time styles that coexist with native statuses and yield to sidebar actions", () => {
  assert.match(source, /threadUpdatedAtAttribute = "data-codey-thread-updated-at"/);
  assert.doesNotMatch(source, /threadSortOrderAttribute|data-codey-thread-sort-order/);
  assert.doesNotMatch(source, /--codey-thread-sort-order|thread-sort-keys/);
  assert.doesNotMatch(source, /data-codey-thread-running/);
  assert.doesNotMatch(source, /sortKey:\s*"updated_at"/);
  assert.match(source, /threadTimestampRefreshIntervalMs = 60_000/);
  assert.match(source, /threadTimestampReadBatchSize = 32/);
  assert.match(source, /dispatcher\("send-cli-request-for-host"/);
  assert.match(source, /method: "thread\/list"/);
  assert.match(source, /refreshThreadUpdatedTimes\(false\)/);
  assert.match(source, /"data-app-action-sidebar-thread-kind"/);
  assert.match(source, /isDeletedSidebarSession\(sessionId\) \|\| !timestamp/);
  assert.match(source, /font-variant-numeric: tabular-nums/);
  assert.match(source, /placeThreadUpdatedAt\(row, label\)/);
  assert.match(source, /mount\.insertBefore\(label, before\)/);
  assert.match(source, /"disabled",\s*"class",/);
  assert.doesNotMatch(source, /"class",\s*"style",/);
  assert.match(source, /sidebar-thread-row\]:hover \[\$\{threadUpdatedAtAttribute\}\].*opacity: 0/s);
});

test("vendor project moves preserve Codex-owned thread ordering", () => {
  assert.doesNotMatch(
    vendorSource,
    /prioritizeRunning|rowHasRunningStatus|ProjectMovePrioritizeRunning/,
  );
  assert.doesNotMatch(
    vendorSource,
    /thread-sort-key|sortMs|codexProjectMoveSortMs|ChatsSortTimer/,
  );
  assert.doesNotMatch(
    vendorSource,
    /const ordered = \[\.\.\.running, \.\.\.idle\]/,
  );
  assert.doesNotMatch(
    vendorSource,
    /codexProjectMoveTimestampMs|timestampTrusted|timestampStateFromMoveResult/,
  );
  assert.match(vendorSource, /function insertProjectedRowItem\(list, item\)/);
  assert.match(vendorSource, /item\.parentElement !== list/);
  assert.match(vendorSource, /list\.insertBefore\(item, firstNonThreadItem\)/);
});
