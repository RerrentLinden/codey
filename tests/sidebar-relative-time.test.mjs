import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../public/codey-inject.js", import.meta.url), "utf8");

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

function loadInjection({ rows = [], bridgeHandler } = {}) {
  const placeholder = new FakeElement();
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
    addEventListener() {},
    clearTimeout() {},
    dispatchEvent() {},
    localStorage: { length: 0, key: () => null, getItem: () => null, setItem() {} },
    setInterval: () => 1,
    setTimeout: (callback) => {
      queueMicrotask(callback);
      return 1;
    },
  };
  window.window = window;
  vm.runInNewContext(source, {
    console,
    document,
    HTMLElement: FakeElement,
    location: { pathname: "/", search: "" },
    MutationObserver: class {
      observe() {}
    },
    URLSearchParams,
    window,
  });
  return { document, window };
}

function sidebarThreadEntry({ running = false, statusState = null } = {}) {
  const list = new FakeElement();
  list.setAttribute("role", "list");
  const item = new FakeElement();
  item.setAttribute("role", "listitem");
  const row = new FakeElement();
  row.setAttribute("data-app-action-sidebar-thread-row", "");
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
  if (statusState) {
    row.__reactFiber$test = {
      memoizedProps: {},
      return: { memoizedProps: { statusState }, return: null },
    };
  }
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
  assert.equal(
    timestampFrom({ session_id: "019f948c-dba4-73c0-83e3-804e6ad6a5be", updated_at_ms: 999_999 }),
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

test("marks running sidebar items for visual priority and clears the mark on completion", () => {
  const idle = sidebarThreadEntry();
  const running = sidebarThreadEntry({ running: true });
  const list = idle.list;
  list.appendChild(running.item);
  const { window } = loadInjection({ rows: [idle.row, running.row] });

  assert.equal(idle.item.getAttribute("data-codey-thread-running"), null);
  assert.equal(running.item.getAttribute("data-codey-thread-running"), "true");
  assert.deepEqual(list.children, [idle.item, running.item]);

  running.spinner.remove();
  window.__codeySyncSidebarThreadRunningOrder(running.row);

  assert.equal(running.item.getAttribute("data-codey-thread-running"), null);
  assert.deepEqual(list.children, [idle.item, running.item]);
});

test("prioritizes active React status without treating unread as running", () => {
  const entry = sidebarThreadEntry({
    statusState: { type: undefined, unread: true },
  });
  const { window } = loadInjection({ rows: [entry.row] });
  const statusFiber = entry.row.__reactFiber$test.return;

  assert.equal(entry.item.getAttribute("data-codey-thread-running"), null);

  statusFiber.memoizedProps.statusState = { type: "processing", unread: false };
  window.__codeySyncSidebarThreadRunningOrder(entry.row);
  assert.equal(entry.item.getAttribute("data-codey-thread-running"), "true");

  statusFiber.memoizedProps.statusState = { type: undefined, unread: false };
  window.__codeySyncSidebarThreadRunningOrder(entry.row);
  assert.equal(entry.item.getAttribute("data-codey-thread-running"), null);
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
    bridgeHandler: async () => ({
      status: "ok",
      sort_keys: [{ session_id: "thread-1", recency_at_ms: timestamp }],
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

test("batches visible thread timestamps through the bridge and renders the result", async () => {
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
    bridgeHandler: async (path, payload) => {
      calls.push({ path, payload });
      return {
        status: "ok",
        sort_keys: [{ session_id: "thread-1", recency_at_ms: timestamp, updated_at_ms: Date.now() }],
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(calls.length, 1);
  assert.equal(calls[0].path, "/thread-sort-keys");
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].payload)), {
    sessions: [{ session_id: "thread-1", title: "发布计划" }],
  });
  assert.equal(
    content.querySelector("[data-codey-thread-updated-at]")?.textContent,
    "3 小时",
  );
  assert.equal(document.threadRowQueries, 1);
});

test("injects time styles that coexist with native statuses and yield to sidebar actions", () => {
  assert.match(source, /threadUpdatedAtAttribute = "data-codey-thread-updated-at"/);
  assert.match(source, /threadRunningAttribute = "data-codey-thread-running"/);
  assert.match(source, /\[\$\{threadRunningAttribute\}="true"\] \{ order: -1 !important; \}/);
  assert.match(source, /font-variant-numeric: tabular-nums/);
  assert.match(source, /placeThreadUpdatedAt\(row, label\)/);
  assert.match(source, /mount\.insertBefore\(label, before\)/);
  assert.match(source, /"disabled",\s*"class",/);
  assert.doesNotMatch(source, /"class",\s*"style",/);
  assert.match(source, /sidebar-thread-row\]:hover \[\$\{threadUpdatedAtAttribute\}\].*opacity: 0/s);
});
