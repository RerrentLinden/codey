import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../public/renderer-inject.js", import.meta.url), "utf8");

class FakeElement {
  constructor(tagName = "div", { visible = true, right = 100, width = right, height = 46, top = 0 } = {}) {
    this.tagName = tagName.toUpperCase();
    this.children = [];
    this.dataset = {};
    this.id = "";
    this.parentElement = null;
    this.right = right;
    this.width = width;
    this.height = height;
    this.top = top;
    this.style = {};
    this.textContent = "";
    this.attributes = new Map();
    this.visible = visible;
    this.isConnected = false;
    this.rectReads = 0;
  }

  addEventListener() {}

  get nextElementSibling() {
    if (!this.parentElement) return null;
    const index = this.parentElement.children.indexOf(this);
    return index >= 0 ? this.parentElement.children[index + 1] || null : null;
  }

  appendChild(child) {
    child.remove();
    child.parentElement = this;
    child.isConnected = true;
    this.children.push(child);
    return child;
  }

  insertBefore(child, before) {
    child.remove();
    const index = this.children.indexOf(before);
    assert.notEqual(index, -1);
    child.parentElement = this;
    child.isConnected = true;
    this.children.splice(index, 0, child);
    return child;
  }

  closest() {
    return null;
  }

  getBoundingClientRect() {
    this.rectReads += 1;
    return this.visible
      ? {
          bottom: this.top + this.height,
          height: this.height,
          left: this.right - this.width,
          right: this.right,
          top: this.top,
          width: this.width,
        }
      : { bottom: 0, height: 0, left: 0, right: 0, top: 0, width: 0 };
  }

  getClientRects() {
    return this.visible ? [this.getBoundingClientRect()] : [];
  }

  querySelector() {
    return null;
  }

  querySelectorAll(selector) {
    if (selector !== "button, [role=button], a[href]") return [];
    const controls = [];
    const visit = (element) => {
      for (const child of element.children) {
        if (child.tagName === "BUTTON") controls.push(child);
        visit(child);
      }
    };
    visit(this);
    return controls;
  }

  matches(selector) {
    return selector
      .split(",")
      .some((part) => part.trim().toUpperCase() === this.tagName);
  }

  remove() {
    if (!this.parentElement) return;
    const index = this.parentElement.children.indexOf(this);
    if (index >= 0) this.parentElement.children.splice(index, 1);
    this.parentElement = null;
    this.isConnected = false;
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  removeAttribute(name) {
    this.attributes.delete(name);
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
    if (name === "id") this.id = String(value);
  }
}

test("moves the Codey button beside the visible header's trailing action region", () => {
  const hiddenHeader = new FakeElement("header", { visible: false });
  const visibleHeader = new FakeElement("header", { right: 1200 });
  const rightRegion = new FakeElement("div", { right: 1200, width: 70 });
  const actionRow = new FakeElement("div", { right: 1192, width: 62 });
  const controlWrapper = new FakeElement("span", { right: 1192, width: 28 });
  const nativeButton = new FakeElement("button", { right: 1192, width: 28 });
  const codeyButton = new FakeElement("button", { right: 200, width: 32 });
  codeyButton.id = "codey-settings-button";
  hiddenHeader.appendChild(codeyButton);
  visibleHeader.appendChild(rightRegion);
  rightRegion.appendChild(actionRow);
  actionRow.appendChild(controlWrapper);
  controlWrapper.appendChild(nativeButton);

  const placeholders = {
    "codey-injected-style": new FakeElement("style"),
    "codey-message-toolbar": new FakeElement(),
    "codey-settings-button": codeyButton,
  };
  const document = {
    body: new FakeElement("body"),
    documentElement: new FakeElement("html"),
    createElement: (tagName) => new FakeElement(tagName),
    getElementById: (id) => placeholders[id] || null,
    querySelector: () => null,
    querySelectorAll: (selector) => (selector === "header" ? [hiddenHeader, visibleHeader] : []),
  };
  const window = {
    addEventListener() {},
    clearTimeout() {},
    dispatchEvent() {},
    getComputedStyle: (element) => ({
      display: element.visible ? "flex" : "none",
      visibility: element.visible ? "visible" : "hidden",
    }),
    localStorage: { getItem: () => null, key: () => null, length: 0, setItem() {} },
    setTimeout: () => 1,
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

  assert.equal(codeyButton.parentElement, visibleHeader);
  assert.equal(codeyButton.dataset.codeyHeaderActions, "true");
  assert.equal(hiddenHeader.children.includes(codeyButton), false);
  assert.deepEqual(visibleHeader.children, [codeyButton, rightRegion]);
});

test("renders official account usage before the header settings action", async () => {
  const visibleHeader = new FakeElement("header", { right: 1200 });
  const sessionTitle = new FakeElement("div", { right: 700, width: 240 });
  sessionTitle.textContent = "当前会话";
  const rightRegion = new FakeElement("div", { right: 1200, width: 70 });
  const nativeButton = new FakeElement("button", { right: 1192, width: 28 });
  rightRegion.appendChild(nativeButton);
  visibleHeader.appendChild(sessionTitle);
  visibleHeader.appendChild(rightRegion);

  const documentElement = new FakeElement("html", {
    right: 1200,
    width: 1200,
    height: 800,
  });
  const findById = (id) => {
    let result = null;
    const visit = (element) => {
      if (result) return;
      if (element.id === id) {
        result = element;
        return;
      }
      element.children.forEach(visit);
    };
    visit(documentElement);
    visit(visibleHeader);
    return result;
  };
  const document = {
    body: new FakeElement("body"),
    documentElement,
    visibilityState: "visible",
    createElement: (tagName) => new FakeElement(tagName),
    getElementById: findById,
    querySelector: () => null,
    querySelectorAll: (selector) =>
      selector === "header" ? [visibleHeader] : [],
  };
  const todayResetAt = new Date();
  todayResetAt.setHours(23, 45, 0, 0);
  const tomorrowResetAt = new Date(todayResetAt);
  tomorrowResetAt.setDate(todayResetAt.getDate() + 1);
  let accountUsageResult = {
    status: "ok",
    planType: "pro",
    primary: {
      usedPercent: 15,
      windowMinutes: 300,
      resetsAt: Math.floor(todayResetAt.getTime() / 1000),
    },
    secondary: {
      usedPercent: 40,
      windowMinutes: 10080,
      resetsAt: Math.floor(tomorrowResetAt.getTime() / 1000),
    },
  };
  const window = {
    __codexSessionDeleteBridge: async (path) => {
      assert.equal(path, "/account/usage");
      return accountUsageResult;
    },
    addEventListener() {},
    alert() {},
    clearTimeout() {},
    dispatchEvent() {},
    getComputedStyle: () => ({ display: "flex", visibility: "visible" }),
    innerWidth: 1200,
    setTimeout: () => 1,
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

  await window.__codeyRefreshAccountUsage();

  const usage = findById("codey-account-usage");
  const settingsButton = findById("codey-settings-button");
  assert.ok(usage);
  assert.ok(settingsButton);
  assert.equal(usage.parentElement, visibleHeader);
  assert.equal(usage.nextElementSibling, settingsButton);
  assert.equal(sessionTitle.parentElement, visibleHeader);
  assert.equal(visibleHeader.children[0], sessionTitle);
  assert.equal(visibleHeader.getAttribute("data-codey-usage-host"), "true");
  assert.match(usage.innerHTML, /5 小时/);
  assert.match(usage.innerHTML, /85%/);
  assert.match(usage.innerHTML, /7 天/);
  assert.match(usage.innerHTML, /60%/);
  assert.match(usage.innerHTML, /今天 \d{2}:\d{2} 刷新/);
  assert.match(usage.innerHTML, /明天 \d{2}:\d{2} 刷新/);
  assert.match(usage.getAttribute("aria-label"), /5 小时额度剩余 85%/);

  accountUsageResult = { status: "unavailable", reason: "third_party" };
  await window.__codeyRefreshAccountUsage();
  assert.equal(findById("codey-account-usage"), null);
  assert.equal(visibleHeader.getAttribute("data-codey-usage-host"), null);
});

test("marks the Codey button when a silent update check finds a new version", async () => {
  const visibleHeader = new FakeElement("header", { right: 1200 });
  const documentElement = new FakeElement("html");
  const elementsById = new Map();
  let nextTimerId = 1;
  const timers = [];
  const events = [];
  const activeTimers = () => timers.filter((timer) => !timer.cleared);
  const visibleButton = () =>
    elementsById.get("codey-settings-button") || null;
  const document = {
    body: new FakeElement("body"),
    documentElement,
    createElement: (tagName) => {
      const element = new FakeElement(tagName);
      let id = element.id;
      Object.defineProperty(element, "id", {
        configurable: true,
        get: () => id,
        set: (value) => {
          id = String(value);
          if (id) elementsById.set(id, element);
        },
      });
      const originalSetAttribute = element.setAttribute.bind(element);
      element.setAttribute = (name, value) => {
        originalSetAttribute(name, value);
        if (name === "id") elementsById.set(String(value), element);
      };
      return element;
    },
    getElementById: (id) =>
      id === "codey-settings-button"
        ? visibleButton()
        : elementsById.get(id) || null,
    querySelector: () => null,
    querySelectorAll: (selector) =>
      selector === "header" ? [visibleHeader] : [],
  };
  const window = {
    __codexSessionDeleteBridge: async (path) => {
      assert.equal(path, "/api/check_for_updates");
      return {
        currentVersion: "0.3.9",
        latestVersion: "0.4.0",
        updateAvailable: true,
        selectedAsset: { fileName: "Codey-0.4.0.zip" },
      };
    },
    addEventListener() {},
    alert() {
      throw new Error("silent update check must not alert");
    },
    clearTimeout(id) {
      const timer = timers.find((entry) => entry.id === id);
      if (timer) timer.cleared = true;
    },
    dispatchEvent(event) {
      events.push(event);
      return true;
    },
    getComputedStyle: () => ({ display: "flex", visibility: "visible" }),
    innerWidth: 1200,
    localStorage: { getItem: () => null, key: () => null, length: 0, setItem() {} },
    setTimeout(callback, delay) {
      const timer = { id: nextTimerId, callback, delay, cleared: false };
      nextTimerId += 1;
      timers.push(timer);
      return timer.id;
    },
  };
  window.window = window;

  vm.runInNewContext(source, {
    console,
    CustomEvent: class {
      constructor(type, init = {}) {
        this.type = type;
        this.detail = init.detail;
      }
    },
    document,
    HTMLElement: FakeElement,
    location: { pathname: "/", search: "" },
    MutationObserver: class {
      observe() {}
    },
    URLSearchParams,
    window,
  });

  const initialTimer = activeTimers().find((timer) => timer.delay === 0);
  assert.equal(initialTimer?.delay, 0);
  initialTimer.cleared = true;
  initialTimer.callback();
  await new Promise((resolve) => setImmediate(resolve));
  const button = visibleButton();
  assert.ok(button);
  assert.equal(button.getAttribute("data-codey-update-available"), "true");
  assert.equal(button.getAttribute("aria-label"), "打开 Codey 配置，有可用更新");
  assert.equal(window.__codeyUpdateAvailability.latestVersion, "0.4.0");
  assert.equal(events.length, 1);
  assert.equal(events[0].type, "codey-update-availability-changed");
  assert.equal(
    activeTimers().some((timer) => timer.delay === 30 * 60 * 1000),
    false,
  );
});

test("ignores sidebar nav and main content until top chrome is available", () => {
  const sidebarNav = new FakeElement("nav", { right: 84, width: 84, height: 720 });
  const main = new FakeElement("main", { right: 1200, width: 1200, height: 640, top: 80 });
  const mainContent = new FakeElement("div", { right: 1080, width: 960, height: 640, top: 80 });
  const staleButton = new FakeElement("button", { right: 60, width: 28 });
  staleButton.id = "codey-settings-button";
  sidebarNav.appendChild(staleButton);
  main.appendChild(mainContent);

  let topNav = null;
  const placeholders = {
    "codey-core-injected-style": new FakeElement("style"),
    "codey-settings-button": staleButton,
  };
  const document = {
    body: new FakeElement("body"),
    documentElement: new FakeElement("html", { right: 1200, width: 1200, height: 800 }),
    createElement: (tagName) => new FakeElement(tagName),
    getElementById: (id) => placeholders[id] || null,
    querySelector: (selector) => (selector === "main" ? main : null),
    querySelectorAll: (selector) => {
      if (selector === "header") return [];
      if (selector === "nav") return topNav ? [sidebarNav, topNav] : [sidebarNav];
      return [];
    },
  };
  const window = {
    addEventListener() {},
    alert() {},
    clearTimeout() {},
    getComputedStyle: (element) => ({
      display: element.visible ? "flex" : "none",
      visibility: element.visible ? "visible" : "hidden",
    }),
    innerWidth: 1200,
    setTimeout: () => 1,
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

  assert.equal(staleButton.parentElement, null);
  assert.equal(sidebarNav.children.includes(staleButton), false);
  assert.equal(mainContent.children.length, 0);

  topNav = new FakeElement("nav", { right: 1200, width: 96, height: 46 });
  window.__codeyRendererScan();

  assert.equal(staleButton.parentElement, topNav);
  assert.deepEqual(topNav.children, [staleButton]);
});

test("repeated scans fast-path an already mounted button without layout reads", () => {
  const visibleHeader = new FakeElement("header", { right: 1200 });
  const rightRegion = new FakeElement("div", { right: 1200, width: 70 });
  const nativeButton = new FakeElement("button", { right: 1192, width: 28 });
  const codeyButton = new FakeElement("button", { right: 1120, width: 28 });
  codeyButton.id = "codey-settings-button";
  codeyButton.dataset.codeyHeaderActions = "true";
  codeyButton.isConnected = true;
  visibleHeader.appendChild(codeyButton);
  visibleHeader.appendChild(rightRegion);
  rightRegion.appendChild(nativeButton);

  const placeholders = {
    "codey-core-injected-style": new FakeElement("style"),
    "codey-settings-button": codeyButton,
  };
  let headerQueries = 0;
  const document = {
    body: new FakeElement("body"),
    documentElement: new FakeElement("html"),
    createElement: (tagName) => new FakeElement(tagName),
    getElementById: (id) => placeholders[id] || null,
    querySelector: () => null,
    querySelectorAll: (selector) => {
      if (selector === "header" || selector === "nav") headerQueries += 1;
      return selector === "header" ? [visibleHeader] : [];
    },
  };
  const window = {
    addEventListener() {},
    alert() {},
    clearTimeout() {},
    getComputedStyle: () => ({ display: "flex", visibility: "visible" }),
    setTimeout: () => 1,
  };
  window.window = window;
  let observerCallback = null;

  vm.runInNewContext(source, {
    console,
    document,
    HTMLElement: FakeElement,
    location: { pathname: "/", search: "" },
    MutationObserver: class {
      constructor(callback) {
        observerCallback = callback;
      }

      observe() {}
    },
    URLSearchParams,
    window,
  });

  headerQueries = 0;
  for (const element of [visibleHeader, rightRegion, nativeButton, codeyButton]) {
    element.rectReads = 0;
  }
  for (let scan = 0; scan < 10; scan += 1) {
    window.__codeyRendererScan();
  }
  assert.equal(headerQueries, 0);
  assert.equal(visibleHeader.rectReads, 0);
  assert.equal(rightRegion.rectReads, 0);
  assert.equal(nativeButton.rectReads, 0);
  assert.equal(codeyButton.rectReads, 0);
  assert.deepEqual(visibleHeader.children, [codeyButton, rightRegion]);

  const newRightRegion = new FakeElement("div", { right: 1200, width: 50 });
  const newRightButton = new FakeElement("button", { right: 1200, width: 28 });
  newRightRegion.appendChild(newRightButton);
  visibleHeader.appendChild(newRightRegion);
  observerCallback([{
    type: "childList",
    target: visibleHeader,
    addedNodes: [newRightRegion],
    removedNodes: [],
  }]);
  window.__codeyRendererScan();

  assert.ok(headerQueries > 0);
  assert.equal(codeyButton.__codeyHeaderAnchor, newRightRegion);
  assert.equal(codeyButton.dataset.codeyHeaderActions, "true");
  assert.deepEqual(visibleHeader.children, [rightRegion, codeyButton, newRightRegion]);
});
