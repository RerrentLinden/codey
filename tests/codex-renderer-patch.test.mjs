import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const normalizeLineEndings = (source) => source.replace(/\r\n/g, "\n");

async function loadStartupPatchExpression(
  experimentalFeatureOverrides = {},
  disablePet = true,
) {
  const source = normalizeLineEndings(
    await readFile(
      new URL("../backend/src/codex_startup_patch.rs", import.meta.url),
      "utf8",
    ),
  );
  const template = source.match(
    /const STARTUP_PATCH_TEMPLATE: &str = r#"\n([\s\S]*?)\n"#;/,
  )?.[1];
  assert.ok(template);
  return template
    .replaceAll("__DISABLE_PET__", disablePet ? "true" : "false")
    .replaceAll("__DISABLE_VOICE__", "false")
    .replaceAll("__FAST_CODEX_STARTUP__", "true")
    .replaceAll(
      "__EXPERIMENTAL_FEATURE_OVERRIDES__",
      JSON.stringify(experimentalFeatureOverrides),
    );
}

test("an incompatible optional renderer patch never blocks the Codex module response", async () => {
  const Module = process.getBuiltinModule("module");
  const nativeLoad = Module._load;
  const nativeJsExtension = Module._extensions[".js"];
  let installedHandler = null;
  class FakeBrowserWindow {}
  const fakeElectron = {
    BrowserWindow: FakeBrowserWindow,
    protocol: {
      handle(scheme, handler) {
        assert.equal(scheme, "app");
        installedHandler = handler;
      },
    },
  };
  Module._load = function testElectronLoader(request) {
    if (request === "electron") return fakeElectron;
    return Reflect.apply(nativeLoad, this, arguments);
  };

  const nativeConsoleError = console.error;
  const patchErrors = [];
  console.error = (...args) => {
    patchErrors.push(args);
  };

  try {
    assert.equal(
      (0, eval)(
        await loadStartupPatchExpression({
          unified_exec: true,
          remote_compaction_v2: false,
        }),
      ),
      "codey-startup-patch-installed-v20",
    );
    const electron = Module._load("electron", undefined, false);
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

    const featureSource = [
      "function Lln(e){",
      "let t=zln(e),n=Bln(e),r={...t,...n,[Fnn]:mnt(e,`2380644311`)};",
      "return Hf.info(`Concurrent reasoning summaries feature override resolved`,{}),r}",
      "const feature_overrides=`feature_overrides`,gate=`2508143457`;",
    ].join("");
    delete globalThis.__CODEY_EXPERIMENTAL_FEATURE_RUNTIME__;
    electron.protocol.handle("app", async () => new Response(featureSource));
    const featureResponse = await installedHandler({
      url: "app://-/assets/app-initial-BHB6SClA.js",
    });
    const patchedFeatureSource = await featureResponse.text();
    const resolveFeatures = Function(
      "zln",
      "Bln",
      "Fnn",
      "mnt",
      "Hf",
      `${patchedFeatureSource};return Lln`,
    )(
      () => ({ unified_exec: false, remote_compaction_v2: true }),
      () => ({ shell_snapshot: true }),
      "concurrent_reasoning_summaries",
      () => true,
      { info() {} },
    );
    assert.deepEqual(resolveFeatures({}), {
      unified_exec: true,
      remote_compaction_v2: false,
      shell_snapshot: true,
      concurrent_reasoning_summaries: true,
    });
    assert.deepEqual(
      globalThis.__CODEY_EXPERIMENTAL_FEATURE_RUNTIME__.configuredFeatures,
      {
        unifiedExec: true,
        remoteCompactionV2: false,
      },
    );
    assert.deepEqual(
      globalThis.__CODEY_EXPERIMENTAL_FEATURE_RUNTIME__.officialFeatures,
      {
        unifiedExec: false,
        remoteCompactionV2: true,
        shellSnapshot: true,
        concurrentReasoningSummaries: true,
      },
    );
    assert.deepEqual(
      globalThis.__CODEY_EXPERIMENTAL_FEATURE_RUNTIME__.effectiveFeatures,
      {
        unifiedExec: true,
        remoteCompactionV2: false,
        shellSnapshot: true,
        concurrentReasoningSummaries: true,
      },
    );
    delete globalThis.__CODEY_EXPERIMENTAL_FEATURE_RUNTIME__;

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
