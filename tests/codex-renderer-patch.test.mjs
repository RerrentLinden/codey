import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const normalizeLineEndings = (source) => source.replace(/\r\n/g, "\n");

async function loadStartupPatchExpression(disablePet = true) {
  const template = normalizeLineEndings(
    await readFile(
      new URL("../backend/src/codex_startup_patch.js", import.meta.url),
      "utf8",
    ),
  );
  assert.ok(template);
  return template
    .replaceAll("__DISABLE_PET__", disablePet ? "true" : "false")
    .replaceAll("__FAST_CODEX_STARTUP__", "true");
}

test("an incompatible optional renderer patch never blocks the Codex module response", async () => {
  const Module = process.getBuiltinModule("module");
  const nativeLoad = Module._load;
  const nativeJsExtension = Module._extensions[".js"];
  let installedHandler = null;
  class FakeEmitter {
    constructor() {
      this.listeners = new Map();
    }

    on(name, listener) {
      const listeners = this.listeners.get(name) || [];
      listeners.push({ listener, once: false });
      this.listeners.set(name, listeners);
      return this;
    }

    once(name, listener) {
      const listeners = this.listeners.get(name) || [];
      listeners.push({ listener, once: true });
      this.listeners.set(name, listeners);
      return this;
    }

    removeListener(name, listener) {
      const listeners = this.listeners.get(name) || [];
      this.listeners.set(
        name,
        listeners.filter((entry) => entry.listener !== listener),
      );
      return this;
    }

    emit(name, ...args) {
      const listeners = [...(this.listeners.get(name) || [])];
      this.listeners.set(
        name,
        listeners.filter((entry) => !entry.once),
      );
      listeners.forEach((entry) => entry.listener(...args));
    }
  }
  class FakeWebContents extends FakeEmitter {
    constructor() {
      super();
      this.currentUrl = "";
      this.loadedUrls = [];
      this.destroyed = false;
    }

    getURL() {
      return this.currentUrl;
    }

    loadURL(url) {
      this.currentUrl = url;
      this.loadedUrls.push(url);
      this.emit("did-start-navigation", {}, url);
      return Promise.resolve();
    }
  }
  class FakeBrowserWindow extends FakeEmitter {
    constructor(options = {}) {
      super();
      this.options = options;
      this.webContents = new FakeWebContents();
      this.destroyed = false;
      this.destroyCalls = 0;
    }

    destroy() {
      if (this.destroyed) return;
      this.destroyed = true;
      this.destroyCalls += 1;
      this.webContents.destroyed = true;
      this.webContents.emit("destroyed");
      this.emit("closed");
    }

    isDestroyed() {
      return this.destroyed;
    }

    loadURL(url) {
      return this.webContents.loadURL(url);
    }
  }
  const fakeElectron = {
    BrowserWindow: FakeBrowserWindow,
    protocol: {
      handle(scheme, handler) {
        assert.equal(scheme, "app");
        installedHandler = handler;
      },
    },
  };
  const fakeAvatarOverlayNative = { createController: () => ({}) };
  Module._load = function testElectronLoader(request) {
    if (request === "electron") return fakeElectron;
    if (request === "C:\\Codex\\avatar_overlay.node") {
      return fakeAvatarOverlayNative;
    }
    return Reflect.apply(nativeLoad, this, arguments);
  };

  const nativeConsoleError = console.error;
  const patchErrors = [];
  console.error = (...args) => {
    patchErrors.push(args);
  };

  try {
    assert.equal(
      (0, eval)(await loadStartupPatchExpression()),
      "codey-startup-patch-installed-v21",
    );
    const electron = Module._load("electron", undefined, false);
    const petSurface = new electron.BrowserWindow({ title: "Pet Surface test" });
    assert.equal(petSurface.destroyed, false);
    const avatarOverlayWindow = new electron.BrowserWindow({
      width: 356,
      height: 320,
      alwaysOnTop: true,
      transparent: true,
      focusable: false,
      show: false,
      frame: false,
      skipTaskbar: true,
    });
    assert.equal(avatarOverlayWindow.destroyed, false);
    assert.equal(
      Module._load("C:\\Codex\\avatar_overlay.node", undefined, false),
      fakeAvatarOverlayNative,
    );
    const routeWindow = new electron.BrowserWindow({ title: "Codex" });
    await routeWindow.webContents.loadURL(
      "app://-/index.html?initialRoute=%2Favatar-overlay",
    );
    assert.equal(routeWindow.destroyed, false);
    assert.deepEqual(routeWindow.webContents.loadedUrls, [
      "app://-/index.html?initialRoute=%2Favatar-overlay",
    ]);
    const nativeAvatarManagerSource = [
      "const avatarStateKey=`electron-avatar-overlay-open`;",
      "class AvatarOverlayManager{",
      "constructor(){this.window=null;this.openingWindowPromise=null;",
      "this.isAppQuitting=false;this.windowVisibilitySequence=1;",
      "this.ensureWindowCalls=0;",
      "this.compositionHost={tuck(){}}}",
      "async ensureWindow(){this.ensureWindowCalls+=1;return {}}",
      "positionWindow(){}",
      "async prewarm(e){",
      "if(this.window!=null||this.openingWindowPromise!=null||this.isAppQuitting)return;",
      "let t=this.windowVisibilitySequence,n=await this.ensureWindow(t);",
      "n==null||t!==this.windowVisibilitySequence||",
      "(this.compositionHost.tuck(),this.positionWindow(n,e))}",
      "async prepareRealtimePresentation(){return this.ensureWindow()}",
      "}",
    ].join("");
    const patchedAvatarManagerSource =
      globalThis.__CODEY_PATCH_CODEX_AVATAR_OVERLAY_PREWARM__(
        nativeAvatarManagerSource,
      );
    assert.match(
      patchedAvatarManagerSource,
      /async prewarm\(e\)\{return;if\(this\.window!=null/,
    );
    const AvatarOverlayManager = Function(
      `${patchedAvatarManagerSource};return AvatarOverlayManager`,
    )();
    const avatarOverlayManager = new AvatarOverlayManager();
    await avatarOverlayManager.prewarm({ x: 0, y: 0 });
    assert.equal(avatarOverlayManager.ensureWindowCalls, 0);
    await avatarOverlayManager.prepareRealtimePresentation();
    assert.equal(avatarOverlayManager.ensureWindowCalls, 1);
    assert.equal(globalThis.__CODEY_CODEX_STARTUP_PATCH__.disablePet, true);
    assert.equal(
      Object.hasOwn(globalThis.__CODEY_CODEX_STARTUP_PATCH__, "petManagerSourceRemoved"),
      false,
    );
    const upstreamHandler = async () =>
      new Response(
        [
          "useHiddenModels:",
          "availableModels:",
          "includeUltraReasoningEffort",
          "amazonBedrock",
        ].join(" "),
      );
    electron.protocol.handle("app", upstreamHandler);
    assert.equal(typeof installedHandler, "function");

    const response = await installedHandler({
      url: "app://-/assets/app-initial-new-codex-build.js",
    });
    assert.equal(response.ok, true);
    assert.match(await response.text(), /useHiddenModels:/);
    // Each incompatible gate is skipped independently (and logged) instead of one
    // throw discarding every gate on the asset. The response is never blocked and
    // the source is returned unchanged when nothing matched.
    assert.ok(patchErrors.length >= 1);
    for (const [message] of patchErrors) {
      assert.match(String(message), /incompatible Codex renderer patch/);
    }

    const petSettingsSource = [
      "import{AvatarPreview as P,builtInPets as L}",
      "from\"./codex-avatar-BpKnWN_W.js\";",
      "const petSettingsId=`settings.appearance.pets.title`;",
      "function renderPetSettings(){return [P(),L.map(()=>1),petSettingsId]}",
    ].join("");
    electron.protocol.handle("app", async () => new Response(petSettingsSource));
    const petSettingsResponse = await installedHandler({
      url: "app://-/assets/general-settings-current-build.js",
    });
    const patchedPetSettingsSource = await petSettingsResponse.text();
    assert.doesNotMatch(patchedPetSettingsSource, /codex-avatar-/);
    assert.match(
      patchedPetSettingsSource,
      /const P=\(\(\)=>\{const target=function\(\)\{return null\}/,
    );
    const renderPetSettings = Function(
      `${patchedPetSettingsSource};return renderPetSettings`,
    )();
    assert.deepEqual(renderPetSettings(), [
      null,
      [],
      "settings.appearance.pets.title",
    ]);

    const sideEffectPetSettingsSource = [
      "import\"./codex-avatar-next-build.js\";",
      "const petSettingsId=`settings.pets.title`;",
    ].join("");
    electron.protocol.handle(
      "app",
      async () => new Response(sideEffectPetSettingsSource),
    );
    const sideEffectPetSettingsResponse = await installedHandler({
      url: "app://-/assets/pet-settings-next-build.js",
    });
    const patchedSideEffectPetSettingsSource =
      await sideEffectPetSettingsResponse.text();
    assert.doesNotMatch(patchedSideEffectPetSettingsSource, /codex-avatar-/);
    assert.match(
      patchedSideEffectPetSettingsSource,
      /const petSettingsId=`settings\.pets\.title`/,
    );

    const statsigSource = [
      "function Ftu(e){",
      "let m=async e=>{",
      "let t=await Ueu(e);",
      "try{let client=new Vtu.StatsigClient(c,t.user,config);return client}",
      "catch(error){throw new Jeu(",
      "mb.CODEX_POST_LOGIN_STATSIG_BOOTSTRAP_FAILURE_TYPE_CLIENT_INITIALIZATION_FAILED,",
      "error)}};",
      "return m}",
      "`Statsig: error while bootstrapping post-login client ",
      "CodexStatsigProvider.sync`;",
      "`useStatsigInternalClientFactoryAsync`;`_getInstance`;",
      "function runAsyncStatsigGate(i,n,o){",
      "return i.loadingStatus!==`Ready`&&",
      "i.initializeAsync().catch(n.Log.error).finally(()=>o(!1))}",
    ].join("");
    electron.protocol.handle("app", async () => new Response(statsigSource));
    const statsigResponse = await installedHandler({
      url: "app://-/assets/app-initial-BHB6SClA.js",
    });
    const patchedStatsigSource = await statsigResponse.text();
    assert.match(
      patchedStatsigSource,
      /let t=await Promise\.race\(\[Ueu\(e\),new Promise/,
    );
    assert.match(patchedStatsigSource, /Codey Statsig bootstrap timeout/);
    assert.doesNotMatch(patchedStatsigSource, /let t=await Ueu\(e\);/);
    assert.match(
      patchedStatsigSource,
      /Promise\.race\(\[i\.initializeAsync\(\),new Promise/,
    );
    assert.match(
      patchedStatsigSource,
      /Codey Statsig async initialization timeout/,
    );
    assert.doesNotMatch(patchedStatsigSource, /i\.initializeAsync\(\)\.catch/);

    const localeSource = [
      "function resolveLocale(a,bp,Au){",
      "const dynamicConfigId=`72216192`,enableI18n=`enable_i18n`;",
      "let o=a?.get(enableI18n,!1);",
      "let s=o,c=a?.get(`locale_source`,`IDE`),l=bp(Au.localeOverride);",
      "return {enabled:s,source:c,locale:l}}",
    ].join("");
    electron.protocol.handle("app", async () => new Response(localeSource));
    const localeResponse = await installedHandler({
      url: "app://-/assets/app-initial-BHB6SClA.js",
    });
    const patchedLocaleSource = await localeResponse.text();
    assert.match(
      patchedLocaleSource,
      /__CODEY_DEFAULT_CHINESE_LOCALE_RENDERER_PATCH__=!0/,
    );
    assert.doesNotMatch(
      patchedLocaleSource,
      /let s=o,c=a\?\.get\(`locale_source`,`IDE`\),l=bp\(Au\.localeOverride\)/,
    );
    delete globalThis.__CODEY_DEFAULT_CHINESE_LOCALE_RENDERER_PATCH__;
    const resolveLocale = Function(`${patchedLocaleSource};return resolveLocale`)();
    assert.deepEqual(
      resolveLocale(
        { get: () => false },
        () => "en-US",
        { localeOverride: {} },
      ),
      { enabled: true, source: "SYSTEM", locale: "zh-CN" },
    );
    assert.equal(
      globalThis.__CODEY_DEFAULT_CHINESE_LOCALE_RENDERER_PATCH__,
      true,
    );
    delete globalThis.__CODEY_DEFAULT_CHINESE_LOCALE_RENDERER_PATCH__;

    const ownerDiscoverySource = [
      "async function maybeResume(Bm,f,n,t){",
      "if(t.followExistingOwner===!0&&f===`local`&&Bm?.clientCoordination!=null){",
      "let owner=null;",
      "try{owner=await Bm.clientCoordination.findThreadOwner({hostId:f,conversationId:n})}",
      "catch(error){console.warn(`maybe_resume_owner_discovery_failed`,error)}",
      "return owner}",
      "return null}",
    ].join("");
    electron.protocol.handle(
      "app",
      async () => new Response(ownerDiscoverySource),
    );
    const ownerDiscoveryResponse = await installedHandler({
      url: "app://-/assets/app-initial-BHB6SClA.js",
    });
    const patchedOwnerDiscoverySource = await ownerDiscoveryResponse.text();
    assert.match(
      patchedOwnerDiscoverySource,
      /__CODEY_THREAD_OWNER_DISCOVERY_V1__/,
    );
    assert.match(
      patchedOwnerDiscoverySource,
      /setTimeout\(\(\)=>\{if\(settled\)return;settled=true;resolve\(null\)\},150\)/,
    );
    assert.match(patchedOwnerDiscoverySource, /expiresAt:now\+5000/);
    assert.match(patchedOwnerDiscoverySource, /state\.cache\.size>=64/);
    assert.doesNotMatch(
      patchedOwnerDiscoverySource,
      /owner=await Bm\.clientCoordination\.findThreadOwner/,
    );
    delete globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V1__;
    const maybeResume = Function(
      `${patchedOwnerDiscoverySource};return maybeResume`,
    )();
    const ownerNativeSetTimeout = globalThis.setTimeout;
    const ownerNativeClearTimeout = globalThis.clearTimeout;
    let scheduledOwnerTimers = 0;
    globalThis.setTimeout = (callback, delay, ...args) => {
      scheduledOwnerTimers += 1;
      return ownerNativeSetTimeout(callback, delay, ...args);
    };
    globalThis.clearTimeout = (timer) => ownerNativeClearTimeout(timer);
    let ownerLookupCalls = 0;
    const primaryCoordination = {
      async findThreadOwner() {
        ownerLookupCalls += 1;
        return "existing-owner";
      },
    };
    try {
      assert.equal(
        await maybeResume(
          { clientCoordination: primaryCoordination },
          "local",
          "thread-1",
          { followExistingOwner: true },
        ),
        "existing-owner",
      );
      assert.equal(ownerLookupCalls, 1);
      assert.equal(scheduledOwnerTimers, 1);

      // A positive answer in the same renderer/coordination generation is a
      // cache hit: no IPC lookup and no fallback timer.
      assert.equal(
        await maybeResume(
          { clientCoordination: primaryCoordination },
          "local",
          "thread-1",
          { followExistingOwner: true },
        ),
        "existing-owner",
      );
      assert.equal(ownerLookupCalls, 1);
      assert.equal(scheduledOwnerTimers, 1);

      // A separate window/client never shares the positive owner cache.
      let overlayLookupCalls = 0;
      assert.equal(
        await maybeResume(
          {
            clientCoordination: {
              async findThreadOwner() {
                overlayLookupCalls += 1;
                return "overlay-owner";
              },
            },
          },
          "local",
          "thread-1",
          { followExistingOwner: true },
        ),
        "overlay-owner",
      );
      assert.equal(overlayLookupCalls, 1);
      assert.equal(scheduledOwnerTimers, 2);
    } finally {
      globalThis.setTimeout = ownerNativeSetTimeout;
      globalThis.clearTimeout = ownerNativeClearTimeout;
      delete globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V1__;
    }

    // Concurrent hydration attempts in the same renderer share one discovery.
    let resolveSharedOwner;
    let sharedLookupCalls = 0;
    const sharedOwner = new Promise((resolve) => {
      resolveSharedOwner = resolve;
    });
    const sharedCoordination = {
      findThreadOwner() {
        sharedLookupCalls += 1;
        return sharedOwner;
      },
    };
    const sharedOwnerFirst = maybeResume(
      { clientCoordination: sharedCoordination },
      "local",
      "thread-shared",
      { followExistingOwner: true },
    );
    const sharedOwnerSecond = maybeResume(
      { clientCoordination: sharedCoordination },
      "local",
      "thread-shared",
      { followExistingOwner: true },
    );
    await Promise.resolve();
    assert.equal(sharedLookupCalls, 1);
    resolveSharedOwner("shared-owner");
    assert.deepEqual(
      await Promise.all([sharedOwnerFirst, sharedOwnerSecond]),
      ["shared-owner", "shared-owner"],
    );
    delete globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V1__;

    // A timeout is uncertainty, not a negative cache entry. The next attempt
    // must retry discovery and can immediately observe a newly available owner.
    const timeoutCallbacks = [];
    let timeoutLookupCalls = 0;
    globalThis.setTimeout = (callback, delay) => {
      const timer = { callback, delay, cleared: false };
      timeoutCallbacks.push(timer);
      return timer;
    };
    globalThis.clearTimeout = (timer) => {
      timer.cleared = true;
    };
    const timeoutCoordination = {
      findThreadOwner() {
        timeoutLookupCalls += 1;
        if (timeoutLookupCalls === 1) return new Promise(() => {});
        return Promise.resolve("owner-after-timeout");
      },
    };
    try {
      const timedOutOwner = maybeResume(
        { clientCoordination: timeoutCoordination },
        "local",
        "thread-timeout",
        { followExistingOwner: true },
      );
      assert.equal(timeoutCallbacks.length, 1);
      assert.equal(timeoutCallbacks[0].delay, 150);
      timeoutCallbacks[0].callback();
      assert.equal(await timedOutOwner, null);

      assert.equal(
        await maybeResume(
          { clientCoordination: timeoutCoordination },
          "local",
          "thread-timeout",
          { followExistingOwner: true },
        ),
        "owner-after-timeout",
      );
      assert.equal(timeoutLookupCalls, 2);
      assert.equal(timeoutCallbacks.length, 2);
      assert.equal(timeoutCallbacks[1].cleared, true);
    } finally {
      globalThis.setTimeout = ownerNativeSetTimeout;
      globalThis.clearTimeout = ownerNativeClearTimeout;
      delete globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V1__;
    }

    const interactionPerformanceSource = [
      "Hcn=class{activeInteractions=new Map;beginCpuSampling;",
      "start(e,n,u){let d={activeKey:e,",
      "cpuSampling:u===`dropped`||n.backfilled===!0?null:this.beginCpuSampling(),",
      "name:e};return this.activeInteractions.set(e,d),this.ensureHeartbeat(),d}",
      "ensureHeartbeat(){this.heartbeatTimer??=setInterval(()=>{",
      "let e=this.now(),t=this.wallNow();",
      "for(let n of this.activeInteractions.values())",
      "this.recordHeartbeat(n,e,t)},Vcn)}",
      "recordHeartbeat(e,t,n){return [e,t,n]}};",
      "const rendererProcessCpuPercentAvg=true;",
      "function unrelated(){return beginCpuSampling()}",
    ].join("");
    electron.protocol.handle(
      "app",
      async () => new Response(interactionPerformanceSource),
    );
    const interactionPerformanceResponse = await installedHandler({
      url: "app://-/assets/app-initial-BHB6SClA.js",
    });
    const patchedInteractionPerformance =
      await interactionPerformanceResponse.text();
    assert.match(patchedInteractionPerformance, /cpuSampling:null/);
    assert.match(patchedInteractionPerformance, /ensureHeartbeat\(\)\{\}/);
    assert.doesNotMatch(
      patchedInteractionPerformance,
      /heartbeatTimer\?\?=setInterval/,
    );
    assert.doesNotMatch(
      patchedInteractionPerformance,
      /cpuSampling:[^,}]*this\.beginCpuSampling\(\)/,
    );
    assert.match(
      patchedInteractionPerformance,
      /function unrelated\(\)\{return beginCpuSampling\(\)\}/,
    );

    let rejectBootstrap;
    const neverCompletesBootstrap = new Promise((_, reject) => {
      rejectBootstrap = reject;
    });
    const createStatsigSync = Function(
      "Ueu",
      "Vtu",
      "Jeu",
      "mb",
      "c",
      "config",
      `${patchedStatsigSource};return Ftu()`,
    );
    const syncStatsig = createStatsigSync(
      () => neverCompletesBootstrap,
      {
        StatsigClient() {
          assert.fail(
            "StatsigClient must not be constructed after bootstrap timeout",
          );
        },
      },
      class FakeStatsigInitializationError extends Error {},
      {
        CODEX_POST_LOGIN_STATSIG_BOOTSTRAP_FAILURE_TYPE_CLIENT_INITIALIZATION_FAILED:
          "client-initialization-failed",
      },
      {},
      {},
    );
    const unhandledRejections = [];
    const onUnhandledRejection = (reason) => {
      unhandledRejections.push(reason);
    };
    const nativeDateNow = Date.now;
    const nativeSetTimeout = globalThis.setTimeout;
    const nativeClearTimeout = globalThis.clearTimeout;
    let now = 10_000;
    const scheduledTimers = [];
    const runNextTimer = () => {
      const timer = scheduledTimers.find((candidate) => !candidate.cleared);
      assert.ok(timer, "expected a scheduled timeout");
      timer.cleared = true;
      timer.callback(...timer.args);
      return timer.delay;
    };

    delete globalThis.__CODEY_STATSIG_STARTUP_DEADLINE_MS__;
    process.on("unhandledRejection", onUnhandledRejection);
    let releaseAsyncInitialization;
    const neverCompletesAsyncInitialization = new Promise((_, reject) => {
      releaseAsyncInitialization = reject;
    });
    const asyncInitializationErrors = [];
    const asyncGateFinished = [];
    const runAsyncStatsigGate = Function(
      `${patchedStatsigSource};return runAsyncStatsigGate`,
    )();
    try {
      Date.now = () => now;
      globalThis.setTimeout = (callback, delay = 0, ...args) => {
        const timer = { callback, delay, args, cleared: false };
        scheduledTimers.push(timer);
        return timer;
      };
      globalThis.clearTimeout = (timer) => {
        if (timer) timer.cleared = true;
      };

      const syncStatsigPromise = syncStatsig("input");
      const syncStatsigAssertion = assert.rejects(
        syncStatsigPromise,
        (error) => {
          assert.match(String(error), /Codey Statsig bootstrap timeout/);
          return true;
        },
      );
      assert.equal(runNextTimer(), 1500);
      now += 1500;
      await syncStatsigAssertion;
      const asyncGatePromise = runAsyncStatsigGate(
        {
          loadingStatus: "Loading",
          initializeAsync: () => neverCompletesAsyncInitialization,
        },
        {
          Log: {
            error: (error) => {
              asyncInitializationErrors.push(error);
            },
          },
        },
        (loading) => {
          asyncGateFinished.push(loading);
        },
      );
      assert.equal(runNextTimer(), 0);
      await asyncGatePromise;
      assert.equal(asyncInitializationErrors.length, 1);
      assert.match(
        String(asyncInitializationErrors[0]),
        /Codey Statsig async initialization timeout/,
      );
      assert.deepEqual(asyncGateFinished, [false]);
      rejectBootstrap(new Error("late bootstrap failure"));
      releaseAsyncInitialization(
        new Error("late async initialization failure"),
      );
      await new Promise((resolve) => setImmediate(resolve));
      assert.deepEqual(unhandledRejections, []);
    } finally {
      delete globalThis.__CODEY_STATSIG_STARTUP_DEADLINE_MS__;
      Date.now = nativeDateNow;
      globalThis.setTimeout = nativeSetTimeout;
      globalThis.clearTimeout = nativeClearTimeout;
      process.off("unhandledRejection", onUnhandledRejection);
    }

    // Only the first (fully incompatible) bundle logged skips — two gates whose
    // anchors were present but whose shapes did not match. The statsig and
    // interaction bundles patched cleanly, so no further skips were logged.
    assert.equal(patchErrors.length, 2);
  } finally {
    console.error = nativeConsoleError;
    Module._load = nativeLoad;
    Module._extensions[".js"] = nativeJsExtension;
  }
});
