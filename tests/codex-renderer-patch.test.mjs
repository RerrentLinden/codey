import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const normalizeLineEndings = (source) => source.replace(/\r\n/g, "\n");

async function loadStartupPatchExpression(
  disablePet = true,
  errorLoggerExecutable = null,
) {
  const template = normalizeLineEndings(
    await readFile(
      new URL("../backend/src/codex_startup_patch.js", import.meta.url),
      "utf8",
    ),
  );
  assert.ok(template);
  const expression = template.replaceAll(
    "__DISABLE_PET__",
    disablePet ? "true" : "false",
  );
  return errorLoggerExecutable == null
    ? expression
    : expression.replaceAll(
        '"__CODEY_ERROR_LOGGER_EXECUTABLE__"',
        JSON.stringify(errorLoggerExecutable),
      );
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
      this.backgroundThrottling = [];
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

    setBackgroundThrottling(enabled) {
      this.backgroundThrottling.push(enabled);
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
  const childProcess = process.getBuiltinModule("child_process");
  const nativeSpawn = childProcess.spawn;
  const nativeSpawnSync = childProcess.spawnSync;
  const asyncLogSpawns = [];
  const syncLogSpawns = [];
  childProcess.spawn = (command, args, options) => {
    const child = new FakeEmitter();
    const stdin = new FakeEmitter();
    const call = { command, args, options, input: null, encoding: null };
    stdin.end = (input, encoding) => {
      call.input = input;
      call.encoding = encoding;
      queueMicrotask(() => child.emit("exit", 0));
    };
    child.stdin = stdin;
    child.kill = () => true;
    child.unref = () => child;
    asyncLogSpawns.push(call);
    return child;
  };
  childProcess.spawnSync = (command, args, options) => {
    syncLogSpawns.push({ command, args, options });
    return { status: 0, stderr: "" };
  };
  const patchErrors = [];
  console.error = (...args) => {
    patchErrors.push(args);
  };

  try {
    assert.equal(
      (0, eval)(await loadStartupPatchExpression(true, "C:\\Codey\\codey.exe")),
      "codey-startup-patch-installed-v31",
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
      webPreferences: { backgroundThrottling: false },
    });
    assert.equal(avatarOverlayWindow.destroyed, false);
    assert.equal(
      avatarOverlayWindow.options.webPreferences.backgroundThrottling,
      true,
    );
    avatarOverlayWindow.emit("show");
    avatarOverlayWindow.emit("hide");
    assert.deepEqual(
      avatarOverlayWindow.webContents.backgroundThrottling,
      [false, true],
    );
    assert.equal(petSurface.options.webPreferences, undefined);
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
    assert.equal(syncLogSpawns.length, 0);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(asyncLogSpawns.length, 1);
    assert.deepEqual(
      JSON.parse(asyncLogSpawns[0].input).map(({ operation }) => operation),
      [
        "renderer_patch:model allowlist",
        "renderer_patch:model visibility",
      ],
    );

    const repeatedResponse = await installedHandler({
      url: "app://-/assets/app-initial-new-codex-build.js",
    });
    assert.match(await repeatedResponse.text(), /useHiddenModels:/);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(
      patchErrors.length,
      2,
      "the same incompatible source must not rerun failed renderer gates",
    );
    assert.equal(
      asyncLogSpawns.length,
      1,
      "the same incompatible source must not spawn another patch logger",
    );

    const currentRendererSource = [
      "const includeUltraReasoningEffort=!0,isServiceTierAllowed=!0;",
      "function currentModelFilter({additionalAvailableModels:e,authMethod:t,availableModels:n,isCustomModelProvider:r,model:i,useHiddenModels:a}){",
      "return e?.has(i.model)===!0||i.model!==`codex-auto-review`&&",
      "(a&&!r&&t!==`amazonBedrock`?n.has(i.model):!i.hidden)}",
      "function currentComposer(){",
      "let w=!1,F=!0,K=!1,xe=`fast`,M={availableOptions:[{value:`fast`}]};",
      "let Ee=!w&&F&&M.availableOptions.length>1;",
      "let Re=!w&&F&&!K&&xe!=null,ze={enabled:Re};",
      "OQ(`composer.toggleFastMode`,()=>{},ze);",
      "let de=!0,r=!1,pe=de&&!r,Se=`fast`,V=()=>{},H=()=>{},U=()=>{},z=!1,B={},te=[];",
      "let Ze=pe?{labelCandidates:te,onBlur:V,onPointerDown:H,onPointerLeave:U,",
      "selectedServiceTierIconKind:Se,showFastServiceTierIndicator:!0,tooltipOpen:z,triggerRef:B}:void 0;",
      "let view={modelPickerTriggerConfig:Ze,selectedServiceTierIconKind:Se};",
      "if(de&&Ze!=null)view.ready=!0;return {Ee,Re,pe,view}}",
      "`composer.intelligenceDropdown.model.title`;",
      "`composer.intelligenceDropdown.model.rowLabel`;",
    ].join("");
    electron.protocol.handle(
      "app",
      async () => new Response(currentRendererSource),
    );
    const currentRendererResponse = await installedHandler({
      url: "app://-/assets/app-initial-current-codex-build.js",
    });
    const patchedCurrentRendererSource = await currentRendererResponse.text();
    assert.match(
      patchedCurrentRendererSource,
      /Ee=!w&&M\.availableOptions\.length>1/,
    );
    assert.match(patchedCurrentRendererSource, /Re=!w&&!K&&xe!=null/);
    assert.match(patchedCurrentRendererSource, /pe=!r/);
    assert.match(patchedCurrentRendererSource, /if\(Ze!=null\)/);
    assert.equal(
      patchErrors.length,
      2,
      "native-compatible model access and current Fast controls must not log skips",
    );

    const hookStatsSource = [
      "const hookLabel=`assistantMessage.hookStats.label`;",
      "const hookTitle=`assistantMessage.hookStats.title`;",
      "function renderHookStats(r,l,d){",
      "return (0,R.jsx)(r,{tooltipContent:l,tooltipClassName:`px-3 py-2`,",
      "tooltipMaxWidth:`min(32rem, var(--radix-tooltip-content-available-width), calc(100vw - 16px))`,",
      "children:d})}",
    ].join("");
    electron.protocol.handle("app", async () => new Response(hookStatsSource));
    const hookStatsResponse = await installedHandler({
      url: "app://-/assets/subagent-activity-chip-group-current-build.js",
    });
    const patchedHookStatsSource = await hookStatsResponse.text();
    assert.match(
      patchedHookStatsSource,
      /\{interactive:!0,tooltipContent:l,tooltipClassName:`px-3 py-2`/,
    );
    assert.equal(
      patchErrors.length,
      2,
      "the compatible hook tooltip patch must not log a skipped renderer gate",
    );

    const historicalSubagentSource = [
      "class HistoricalSubagentTopology{",
      "constructor(store,requestClient,threads){",
      "this.params={threadStore:store,requestClient};this.threads=threads}",
      "async readLatestPaginatedDescendantTurn(e){return e}",
      "async listDescendantThreads(){let t=this.threads;",
      "let l=await Promise.all(t.map(async e=>{let t=e.id;",
      "if(e.status.type!==`notLoaded`||e.historyMode!==`paginated`||this.params.threadStore.getConversation(t)!=null)return e;",
      "try{return await this.readLatestPaginatedDescendantTurn(e)}catch{return e}})),u=!0;",
      "return{descendantThreads:l,isComplete:u}}",
      "discover(e,{reconcile:t=!0}={}){let n=this.listDescendantThreads(e);",
      "return n.then(n=>!t||!n.isComplete?n:{...n,descendantThreads:this.params.threadStore.reconcileSubagentDescendantSnapshot(e,n.descendantThreads)})}}",
      "function projectHistoricalAgents({cachedConversations:e,conversationTurns:t,getIndexedSubagentItems:n,getIndexedSubagentProgress:r,getThreadRuntimeStatusEvidence:i,parentConversationId:a,sourceLinkedThreads:o,threadSummaries:s=[]}){",
      "let projected=globalThis.__codeyTestProjectedAgents??[];return projected}",
      "const historicalMarkers=`thread/turns/list getThreadRuntimeStatusEvidence recordThreadRuntimeStatusEvidence reconcileSubagentDescendantSnapshot`;",
    ].join("");
    const originalHistoricalExports = Function(
      `${historicalSubagentSource};return {HistoricalSubagentTopology,projectHistoricalAgents}`,
    )();
    const OriginalHistoricalSubagentTopology =
      originalHistoricalExports.HistoricalSubagentTopology;
    electron.protocol.handle(
      "app",
      async () => new Response(historicalSubagentSource),
    );
    const historicalSubagentResponse = await installedHandler({
      url: "app://-/assets/app-initial-subagent-history-current-build.js",
    });
    const patchedHistoricalSubagentSource =
      await historicalSubagentResponse.text();
    assert.match(
      patchedHistoricalSubagentSource,
      /__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__/,
    );
    assert.match(
      patchedHistoricalSubagentSource,
      /\.then\(async \w+=>\{if\(!\w+\|\|!\w+\.isComplete\)return/,
      "verification must run after the native reconcile path has produced the final list",
    );
    assert.match(patchedHistoricalSubagentSource, /entries\.size>=256/);
    assert.match(patchedHistoricalSubagentSource, /activeRequests<2/);
    assert.match(patchedHistoricalSubagentSource, /queryCandidates\.slice\(0,8\)/);
    assert.match(patchedHistoricalSubagentSource, /inspected<32/);
    assert.match(patchedHistoricalSubagentSource, /context\.pending\.size>=32/);
    assert.match(patchedHistoricalSubagentSource, /requestQueue\.length>=32/);
    assert.match(patchedHistoricalSubagentSource, /contexts\.size>8/);
    assert.match(patchedHistoricalSubagentSource, /topologyCursor/);
    assert.match(patchedHistoricalSubagentSource, /projectionCursor/);
    assert.match(patchedHistoricalSubagentSource, /visitLimit=Math\.min\(length,128\)/);
    assert.match(
      patchedHistoricalSubagentSource,
      /context\.rows\.get\(String\(entry\.id\)\)/,
      "a projected request must revalidate the current topology row before publishing idle",
    );
    assert.match(
      patchedHistoricalSubagentSource,
      /\.observe\(a,o,projected\)/,
      "the central sidebar projection must expose its exact UI-active set to the verifier",
    );
    assert.match(patchedHistoricalSubagentSource, /itemsView:`notLoaded`/);
    assert.match(patchedHistoricalSubagentSource, /limit:1/);
    assert.match(patchedHistoricalSubagentSource, /expiresAt:Date\.now\(\)\+30000/);
    assert.doesNotMatch(
      patchedHistoricalSubagentSource,
      /__CODEY_SUBAGENT_STATUS_RECONCILER_V1__|\[0,200,800\]/,
      "the superseded full-discovery retry burst must be absent",
    );

    const historicalExports = Function(
      `${patchedHistoricalSubagentSource};return {HistoricalSubagentTopology,projectHistoricalAgents}`,
    )();
    const { HistoricalSubagentTopology, projectHistoricalAgents } =
      historicalExports;
    const thread = (id, type = "active") => ({
      id,
      status: { type },
      historyMode: "legacy",
      createdAt: 1,
      updatedAt: 2,
    });
    const createHistoricalStore = ({
      liveEvidence = new Map(),
      loadedStatuses = new Map(),
      loadedRuntimeStatuses = new Map(),
      passiveThreads = [],
    } = {}) => {
      const summaries = new Map();
      const threadsById = new Map(passiveThreads.map((current) => [current.id, current]));
      const recordedStatuses = [];
      const updatedConversations = [];
      const conversations = new Map();
      for (const [id, status] of loadedStatuses) {
        const threadRuntimeStatus = loadedRuntimeStatuses.get(id);
        conversations.set(id, {
          turns: [{ status }],
          ...(threadRuntimeStatus == null ? {} : { threadRuntimeStatus }),
        });
      }
      return {
        recordedStatuses,
        updatedConversations,
        getConversation(id) {
          return conversations.get(id) ?? null;
        },
        getThreadRuntimeStatusEvidence(id) {
          return liveEvidence.get(id) ?? null;
        },
        recordThreadRuntimeStatusEvidence(id, status) {
          recordedStatuses.push({ id, status });
          if (conversations.has(id)) liveEvidence.delete(id);
          else liveEvidence.set(id, status);
          const existing = threadsById.get(id);
          if (existing != null) threadsById.set(id, { ...existing, status });
          const summary = summaries.get(id);
          if (summary != null) {
            summaries.set(id, { ...summary, threadRuntimeStatus: status });
          }
        },
        updateConversationState(id, update) {
          const conversation = conversations.get(id);
          if (conversation == null) return;
          update(conversation);
          updatedConversations.push(id);
        },
        reconcileSubagentDescendantSnapshot(_parentId, threads) {
          const seen = new Set(threads.map(({ id }) => id));
          const reconciled = [
            ...threads,
            ...passiveThreads.filter(({ id }) => !seen.has(id)),
          ];
          return reconciled.map((current) => {
            threadsById.set(current.id, current);
            const status = liveEvidence.get(current.id) ?? current.status;
            summaries.set(current.id, { threadRuntimeStatus: status });
            return status === current.status
              ? current
              : { ...current, status };
          });
        },
        projectedStatus(snapshot, id) {
          return liveEvidence.get(id)?.type
            ?? conversations.get(id)?.threadRuntimeStatus?.type
            ?? summaries.get(id)?.threadRuntimeStatus?.type
            ?? snapshot.descendantThreads.find((current) => current.id === id)
              ?.status.type;
        },
      };
    };
    const createTurnsClient = ({
      statuses,
      liveEvidence,
      onResponse,
      sharedConcurrency,
      wrapped = false,
    }) => {
      const requests = [];
      let inFlight = 0;
      let maxInFlight = 0;
      return {
        requests,
        getMaxInFlight: () => maxInFlight,
        async sendRequest(method, params, options) {
          requests.push({ method, params, options });
          assert.equal(method, "thread/turns/list");
          inFlight += 1;
          maxInFlight = Math.max(maxInFlight, inFlight);
          if (sharedConcurrency) {
            sharedConcurrency.inFlight += 1;
            sharedConcurrency.maxInFlight = Math.max(
              sharedConcurrency.maxInFlight,
              sharedConcurrency.inFlight,
            );
          }
          await new Promise((resolve) => setImmediate(resolve));
          inFlight -= 1;
          if (sharedConcurrency) sharedConcurrency.inFlight -= 1;
          onResponse?.(params.threadId, liveEvidence);
          const status = statuses.get(params.threadId);
          if (status === "throw") throw new Error("latest turn unavailable");
          const response = { data: status == null ? [] : [{ status }] };
          return wrapped ? { response } : response;
        },
      };
    };

    // Reproduce the actual failure: the state-DB list itself contains no active
    // row, but native reconcile appends a cached active descendant. A verifier
    // placed before reconcile sees zero candidates while the final UI projection
    // remains active through the native summary.
    const baselineStore = createHistoricalStore({
      passiveThreads: [thread("child-baseline")],
    });
    const baselineClient = createTurnsClient({
      statuses: new Map([["child-baseline", "completed"]]),
    });
    const baselineSnapshot = await new OriginalHistoricalSubagentTopology(
      baselineStore,
      baselineClient,
      [thread("child-listed-idle", "idle")],
    ).discover("parent-baseline");
    assert.equal(
      baselineStore.projectedStatus(baselineSnapshot, "child-baseline"),
      "active",
    );
    assert.equal(baselineClient.requests.length, 0);

    delete globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__;
    const staleEvidence = { type: "active" };
    const liveEvidence = new Map([["child-stale-evidence", staleEvidence]]);
    const primaryThreads = [
      thread("child-failed"),
      thread("child-interrupted"),
      thread("child-running"),
      thread("child-error"),
      thread("child-stale-evidence"),
      thread("child-loaded-stale"),
      thread("child-loaded-running"),
      thread("child-became-live"),
      thread("child-idle", "idle"),
    ];
    const primaryStore = createHistoricalStore({
      liveEvidence,
      loadedStatuses: new Map([
        ["child-loaded-stale", "completed"],
        ["child-loaded-running", "inProgress"],
      ]),
      loadedRuntimeStatuses: new Map([
        ["child-loaded-stale", { type: "active" }],
      ]),
      passiveThreads: [thread("child-completed")],
    });
    const primaryClient = createTurnsClient({
      statuses: new Map([
        ["child-completed", "completed"],
        ["child-failed", "failed"],
        ["child-interrupted", "interrupted"],
        ["child-running", "inProgress"],
        ["child-error", "throw"],
        ["child-stale-evidence", "completed"],
        ["child-loaded-stale", "completed"],
        ["child-became-live", "completed"],
      ]),
      liveEvidence,
      onResponse(id, evidence) {
        if (id === "child-became-live") evidence.set(id, { type: "active" });
      },
      wrapped: true,
    });
    const primarySnapshot = await new HistoricalSubagentTopology(
      primaryStore,
      primaryClient,
      primaryThreads,
    ).discover("parent-primary");
    assert.equal(primaryClient.getMaxInFlight(), 2);
    assert.equal(primaryClient.requests.length, 8);
    assert.ok(primaryClient.requests.every(({ params, options }) =>
      params.limit === 1
      && params.sortDirection === "desc"
      && params.itemsView === "notLoaded"
      && options.priority === "background"
    ));
    assert.equal(primaryStore.projectedStatus(primarySnapshot, "child-completed"), "idle");
    assert.equal(primaryStore.projectedStatus(primarySnapshot, "child-failed"), "idle");
    assert.equal(primaryStore.projectedStatus(primarySnapshot, "child-interrupted"), "idle");
    assert.equal(primaryStore.projectedStatus(primarySnapshot, "child-running"), "active");
    assert.equal(primaryStore.projectedStatus(primarySnapshot, "child-error"), "active");
    assert.equal(
      primaryStore.projectedStatus(primarySnapshot, "child-stale-evidence"),
      "idle",
      "unchanged stale active evidence must not be mistaken for a live turn",
    );
    assert.equal(
      primaryStore.projectedStatus(primarySnapshot, "child-loaded-stale"),
      "idle",
      "a loaded conversation's stale runtime status must be corrected too",
    );
    assert.equal(primaryStore.projectedStatus(primarySnapshot, "child-loaded-running"), "active");
    assert.equal(primaryStore.projectedStatus(primarySnapshot, "child-became-live"), "active");
    assert.equal(primaryStore.projectedStatus(primarySnapshot, "child-idle"), "idle");
    assert.deepEqual(
      primaryStore.recordedStatuses.map(({ id }) => id).sort(),
      [
        "child-completed",
        "child-failed",
        "child-interrupted",
        "child-loaded-stale",
        "child-stale-evidence",
      ],
      "confirmed terminal rows must update Codex's native runtime status source",
    );
    assert.deepEqual(
      primaryStore.updatedConversations,
      ["child-loaded-stale"],
      "only a loaded stale conversation should require a conversation-state update",
    );
    assert.equal(
      primaryStore.getConversation("child-loaded-stale").threadRuntimeStatus.type,
      "idle",
    );
    assert.deepEqual(
      globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__.snapshot(),
      {
        version: 5,
        scans: 1,
        inspected: 8,
        candidates: 8,
        projectionScans: 0,
        projectionInspected: 0,
        projectionCandidates: 0,
        requests: 8,
        activeRequests: 0,
        queuedRequests: 0,
        cacheHits: 0,
        peakRequests: 2,
        queuePeak: 6,
        corrected: 5,
        skipped: 2,
        failures: 1,
        deferred: 0,
        contexts: 1,
      },
    );

    delete globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__;
    const cappedThreads = Array.from({ length: 9 }, (_value, index) => ({
      ...thread(`child-cap-${index + 1}`),
      updatedAt: index === 0 ? 100 : index,
    }));
    const cappedStore = createHistoricalStore();
    const cappedClient = createTurnsClient({
      statuses: new Map(cappedThreads.map(({ id }) => [id, "completed"])),
    });
    const cappedTopology = new HistoricalSubagentTopology(
      cappedStore,
      cappedClient,
      cappedThreads,
    );
    const firstCappedSnapshot = await cappedTopology.discover("parent-cap");
    assert.equal(cappedClient.requests.length, 8);
    assert.equal(cappedClient.getMaxInFlight(), 2);
    assert.deepEqual(
      cappedClient.requests.map(({ params }) => params.threadId),
      cappedThreads.slice(1).map(({ id }) => id),
      "the bounded first pass should prioritize older suspicious rows",
    );
    assert.equal(
      firstCappedSnapshot.descendantThreads.filter(({ status }) =>
        status.type === "idle"
      ).length,
      8,
    );
    const secondCappedSnapshot = await cappedTopology.discover("parent-cap");
    assert.equal(cappedClient.requests.length, 9);
    assert.equal(
      secondCappedSnapshot.descendantThreads.every(({ status }) =>
        status.type === "idle"
      ),
      true,
    );
    assert.equal(
      globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__.snapshot()
        .cacheHits,
      0,
      "publishing idle into the native store should avoid cache-only rechecks",
    );

    delete globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__;
    const rotatingTopologyThreads = Array.from(
      { length: 33 },
      (_value, index) => ({
        ...thread(`child-rotating-topology-${index + 1}`),
        updatedAt: index === 32 ? 0 : 100,
      }),
    );
    const rotatingTopologyStore = createHistoricalStore();
    const rotatingTopologyClient = createTurnsClient({
      statuses: new Map(rotatingTopologyThreads.map(({ id }, index) => [
        id,
        index === 32 ? "completed" : "inProgress",
      ])),
    });
    const rotatingTopology = new HistoricalSubagentTopology(
      rotatingTopologyStore,
      rotatingTopologyClient,
      rotatingTopologyThreads,
    );
    const firstRotatingTopology = await rotatingTopology.discover(
      "parent-rotating-topology",
    );
    assert.equal(
      firstRotatingTopology.descendantThreads[32].status.type,
      "active",
    );
    assert.equal(
      rotatingTopologyClient.requests.some(({ params }) =>
        params.threadId === "child-rotating-topology-33"
      ),
      false,
    );
    const secondRotatingTopology = await rotatingTopology.discover(
      "parent-rotating-topology",
    );
    assert.equal(
      secondRotatingTopology.descendantThreads[32].status.type,
      "idle",
      "the rotating cursor must reach a stale row after 32 long-running predecessors",
    );
    assert.equal(
      rotatingTopologyClient.requests.filter(({ params }) =>
        params.threadId === "child-rotating-topology-33"
      ).length,
      1,
    );

    // Reproduce the restart-only counterexample from the production renderer:
    // native topology reports every row as notLoaded/legacy while stale parent
    // collab state makes the central sidebar projection label 31 of them active.
    // Only those exact projected rows should receive a bounded latest-turn read.
    delete globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__;
    const projectedActiveIds = Array.from(
      { length: 31 },
      (_value, index) => `child-projected-active-${index + 1}`,
    );
    const projectedDoneIds = Array.from(
      { length: 9 },
      (_value, index) => `child-projected-done-${index + 1}`,
    );
    const projectedThreads = [
      ...projectedActiveIds,
      ...projectedDoneIds,
    ].map((id) => thread(id, "notLoaded"));
    const projectedStore = createHistoricalStore();
    const projectedClient = createTurnsClient({
      statuses: new Map(projectedThreads.map(({ id }) => [id, "completed"])),
    });
    await new HistoricalSubagentTopology(
      projectedStore,
      projectedClient,
      projectedThreads,
    ).discover("parent-projected");
    const projectionProps = {
      cachedConversations: [],
      conversationTurns: [],
      getIndexedSubagentItems: null,
      getIndexedSubagentProgress: null,
      getThreadRuntimeStatusEvidence: null,
      parentConversationId: "parent-projected",
      sourceLinkedThreads: projectedThreads,
      threadSummaries: [],
    };
    globalThis.__codeyTestProjectedAgents = [
      ...projectedActiveIds.map((conversationId) => ({
        conversationId,
        status: "active",
      })),
      ...projectedDoneIds.map((conversationId) => ({
        conversationId,
        status: "done",
      })),
    ];
    const firstProjection = projectHistoricalAgents(projectionProps);
    const duplicatePendingProjection = projectHistoricalAgents(projectionProps);
    assert.equal(
      firstProjection.filter(({ status }) => status === "active").length,
      31,
    );
    assert.equal(
      duplicatePendingProjection.filter(({ status }) => status === "active")
        .length,
      31,
      "selector re-entry before the microtask flush must not duplicate requests",
    );
    for (let attempt = 0; attempt < 100; attempt += 1) {
      const stats =
        globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__.snapshot();
      if (stats.corrected === projectedActiveIds.length) break;
      await new Promise((resolve) => setImmediate(resolve));
    }
    const projectedStats =
      globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__.snapshot();
    assert.equal(projectedStats.scans, 1);
    assert.equal(projectedStats.inspected, 0);
    assert.equal(projectedStats.candidates, 0);
    assert.equal(projectedStats.projectionCandidates, 31);
    assert.equal(projectedStats.requests, 31);
    assert.equal(projectedStats.corrected, 31);
    assert.equal(projectedStats.peakRequests, 2);
    assert.ok(projectedStats.queuePeak <= 32);
    assert.equal(projectedStats.contexts, 1);
    assert.equal(projectedClient.requests.length, 31);
    assert.equal(projectedClient.getMaxInFlight(), 2);
    assert.deepEqual(
      projectedClient.requests.map(({ params }) => params.threadId).sort(),
      projectedActiveIds.toSorted(),
      "already-done projected rows must not be queried",
    );
    assert.ok(projectedClient.requests.every(({ params, options }) =>
      params.limit === 1
      && params.sortDirection === "desc"
      && params.itemsView === "notLoaded"
      && options.priority === "background"
      && options.source === "collab_hydration"
    ));
    assert.deepEqual(
      projectedStore.recordedStatuses.map(({ id }) => id).sort(),
      projectedActiveIds.toSorted(),
    );
    assert.equal(
      projectHistoricalAgents(projectionProps).every(({ status }) =>
        status === "done"
      ),
      true,
      "terminal cache plus native idle evidence must correct the next projection",
    );
    delete globalThis.__codeyTestProjectedAgents;

    delete globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__;
    const rotatingProjectionThreads = Array.from(
      { length: 33 },
      (_value, index) => thread(`child-rotating-projection-${index + 1}`, "notLoaded"),
    );
    const rotatingProjectionStore = createHistoricalStore();
    const rotatingProjectionClient = createTurnsClient({
      statuses: new Map(rotatingProjectionThreads.map(({ id }, index) => [
        id,
        index === 32 ? "completed" : "inProgress",
      ])),
    });
    await new HistoricalSubagentTopology(
      rotatingProjectionStore,
      rotatingProjectionClient,
      rotatingProjectionThreads,
    ).discover("parent-rotating-projection");
    const rotatingProjectionProps = {
      ...projectionProps,
      parentConversationId: "parent-rotating-projection",
      sourceLinkedThreads: rotatingProjectionThreads,
    };
    globalThis.__codeyTestProjectedAgents = rotatingProjectionThreads.map(({ id }) => ({
      conversationId: id,
      status: "active",
    }));
    projectHistoricalAgents(rotatingProjectionProps);
    for (let attempt = 0; attempt < 100; attempt += 1) {
      const stats =
        globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__.snapshot();
      if (
        stats.requests === 32
        && stats.activeRequests === 0
        && stats.queuedRequests === 0
      ) break;
      await new Promise((resolve) => setImmediate(resolve));
    }
    assert.equal(rotatingProjectionClient.requests.length, 32);
    assert.equal(
      rotatingProjectionClient.requests.some(({ params }) =>
        params.threadId === "child-rotating-projection-33"
      ),
      false,
    );
    projectHistoricalAgents(rotatingProjectionProps);
    for (let attempt = 0; attempt < 100; attempt += 1) {
      const stats =
        globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__.snapshot();
      if (stats.corrected === 1) break;
      await new Promise((resolve) => setImmediate(resolve));
    }
    assert.equal(
      rotatingProjectionClient.requests.filter(({ params }) =>
        params.threadId === "child-rotating-projection-33"
      ).length,
      1,
    );
    assert.equal(
      projectHistoricalAgents(rotatingProjectionProps).at(-1).status,
      "done",
      "the projection cursor must reach a stale row after 32 cooldown entries",
    );
    delete globalThis.__codeyTestProjectedAgents;

    delete globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__;
    const supersededRow = thread("child-projection-superseded", "notLoaded");
    const supersededStore = createHistoricalStore();
    let resolveSupersededRequest;
    const supersededClient = {
      requests: [],
      sendRequest(method, params, options) {
        this.requests.push({ method, params, options });
        return new Promise((resolve) => {
          resolveSupersededRequest = resolve;
        });
      },
    };
    const supersededTopology = new HistoricalSubagentTopology(
      supersededStore,
      supersededClient,
      [supersededRow],
    );
    await supersededTopology.discover("parent-projection-superseded");
    let supersededSourceThreads = [supersededRow];
    const supersededProjectionProps = {
      ...projectionProps,
      parentConversationId: "parent-projection-superseded",
      sourceLinkedThreads: supersededSourceThreads,
    };
    globalThis.__codeyTestProjectedAgents = [{
      conversationId: supersededRow.id,
      status: "active",
    }];
    projectHistoricalAgents(supersededProjectionProps);
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(typeof resolveSupersededRequest, "function");
    const replacementRow = { ...supersededRow, updatedAt: 3 };
    supersededSourceThreads = [replacementRow];
    supersededTopology.threads = supersededSourceThreads;
    await supersededTopology.discover("parent-projection-superseded");
    resolveSupersededRequest({ data: [{ status: "completed" }] });
    for (let attempt = 0; attempt < 10; attempt += 1) {
      const stats =
        globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__.snapshot();
      if (stats.activeRequests === 0) break;
      await new Promise((resolve) => setImmediate(resolve));
    }
    assert.equal(supersededStore.recordedStatuses.length, 0);
    assert.equal(
      globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__.snapshot()
        .corrected,
      0,
      "a request for a replaced topology row must not publish stale idle evidence",
    );
    delete globalThis.__codeyTestProjectedAgents;

    delete globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__;
    const sharedConcurrency = { inFlight: 0, maxInFlight: 0 };
    const sharedTopologies = ["a", "b"].map((suffix) => {
      const threads = Array.from({ length: 4 }, (_value, index) =>
        thread(`child-shared-${suffix}-${index + 1}`)
      );
      return new HistoricalSubagentTopology(
        createHistoricalStore(),
        createTurnsClient({
          statuses: new Map(threads.map(({ id }) => [id, "completed"])),
          sharedConcurrency,
        }),
        threads,
      );
    });
    await Promise.all(sharedTopologies.map((topology, index) =>
      topology.discover(`parent-shared-${index}`)
    ));
    assert.equal(
      sharedConcurrency.maxInFlight,
      2,
      "all mounted loaders must share the same native request limit",
    );
    delete globalThis.__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__;
    assert.equal(
      patchErrors.length,
      2,
      "the compatible historical status patch must not log a skipped renderer gate",
    );

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
      /__CODEY_THREAD_OWNER_DISCOVERY_V2__/,
    );
    assert.match(
      patchedOwnerDiscoverySource,
      /setTimeout\(\(\)=>\{if\(settled\)return;settled=true;resolve\(null\)\},150\)/,
    );
    assert.doesNotMatch(patchedOwnerDiscoverySource, /expiresAt|\.cache/);
    assert.doesNotMatch(
      patchedOwnerDiscoverySource,
      /owner=await Bm\.clientCoordination\.findThreadOwner/,
    );
    delete globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V2__;
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
    let currentOwner = "existing-owner";
    const primaryCoordination = {
      async findThreadOwner() {
        ownerLookupCalls += 1;
        return currentOwner;
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

      // A settled positive answer must not be reused. The owner may have
      // disconnected before the next hydration attempt, and returning its
      // stale client ID would skip local thread hydration indefinitely.
      currentOwner = null;
      assert.equal(
        await maybeResume(
          { clientCoordination: primaryCoordination },
          "local",
          "thread-1",
          { followExistingOwner: true },
        ),
        null,
      );
      assert.equal(ownerLookupCalls, 2);
      assert.equal(scheduledOwnerTimers, 2);

      // A separate window/client never shares in-flight discovery state.
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
      assert.equal(scheduledOwnerTimers, 3);
    } finally {
      globalThis.setTimeout = ownerNativeSetTimeout;
      globalThis.clearTimeout = ownerNativeClearTimeout;
      delete globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V2__;
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
    delete globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V2__;

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
      delete globalThis.__CODEY_THREAD_OWNER_DISCOVERY_V2__;
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

    // Only the first (fully incompatible) bundle logged skips — two gates whose
    // anchors were present but whose shapes did not match. The interaction
    // bundle patched cleanly, so no further skips were logged.
    assert.equal(patchErrors.length, 2);

    const productionRendererAsset = process.env.CODEY_RENDERER_ASSET;
    if (productionRendererAsset) {
      const productionSource = await readFile(productionRendererAsset, "utf8");
      const previousErrorCount = patchErrors.length;
      electron.protocol.handle("app", async () => new Response(productionSource));
      const productionResponse = await installedHandler({
        url: "app://-/assets/app-initial-production-build.js",
      });
      const patchedProductionSource = await productionResponse.text();
      await new Promise((resolve) => setImmediate(resolve));
      assert.notEqual(
        patchedProductionSource,
        productionSource,
        "the production renderer asset should receive compatible Codey gates",
      );
      if (
        productionSource.includes("readLatestPaginatedDescendantTurn")
        && productionSource.includes("thread/turns/list")
        && productionSource.includes("getThreadRuntimeStatusEvidence")
        && productionSource.includes("reconcileSubagentDescendantSnapshot")
      ) {
        assert.match(
          patchedProductionSource,
          /__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__/,
          "the production descendant loader should verify the final reconciled active rows",
        );
        assert.match(
          patchedProductionSource,
          /__CODEY_SUBAGENT_HISTORICAL_ACTIVE_VERIFIER_V5__\?\.observe\(/,
          "the production sidebar projection should expose its exact UI-active legacy rows",
        );
      }
      const currentGateFailures = patchErrors
        .slice(previousErrorCount)
        .map(([message]) => String(message))
        .filter((message) =>
          /model allowlist|model visibility|model-aware service tier control|model-aware Fast toggle|fast model trigger availability|subagent historical active verification|subagent projected active observation/.test(
            message,
          ),
        );
      assert.deepEqual(
        currentGateFailures,
        [],
        "the current production renderer shapes must not log known compatibility failures",
      );
    }
  } finally {
    console.error = nativeConsoleError;
    childProcess.spawn = nativeSpawn;
    childProcess.spawnSync = nativeSpawnSync;
    Module._load = nativeLoad;
    Module._extensions[".js"] = nativeJsExtension;
  }
});
