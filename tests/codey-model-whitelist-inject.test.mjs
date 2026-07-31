import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const MODEL_CONFIG_ID = "107580212";

async function loadPatch(
  catalogResponse,
  clients,
  { bridgeReady = true, queryClient = null } = {},
) {
  const source = await readFile(
    new URL("../public/model-whitelist-inject.js", import.meta.url),
    "utf8",
  );
  let nextTimer = 0;
  const timers = new Map();
  const windowListeners = new Map();
  const body = {};
  if (queryClient) {
    body.__reactFiber$codeyTest = {
      memoizedProps: {
        queryClient,
      },
    };
  }
  const document = {
    body,
    documentElement: {},
    getElementById() {
      return null;
    },
    querySelectorAll() {
      return [];
    },
    addEventListener() {},
    removeEventListener() {},
  };
  const bridge = async (path) => {
    assert.equal(path, "/codex-model-catalog");
    return typeof catalogResponse === "function"
      ? catalogResponse()
      : catalogResponse;
  };
  const window = {
    __STATSIG__: {
      firstInstance: clients[0],
      instances: Object.fromEntries(clients.slice(1).map((client, index) => [index, client])),
    },
    addEventListener(name, listener) {
      const listeners = windowListeners.get(name) || new Set();
      listeners.add(listener);
      windowListeners.set(name, listeners);
    },
    removeEventListener(name, listener) {
      windowListeners.get(name)?.delete(listener);
    },
    setTimeout(callback) {
      nextTimer += 1;
      timers.set(nextTimer, callback);
      return nextTimer;
    },
    clearTimeout(id) {
      timers.delete(id);
    },
  };
  if (bridgeReady) window.__codexSessionDeleteBridge = bridge;
  Function("window", "document", "globalThis", "console", source)(
    window,
    document,
    window,
    { warn() {} },
  );
  const patch = window.__codeyModelWhitelistPatch;
  if (bridgeReady) await patch.refresh();
  return {
    patch,
    connectBridge() {
      window.__codexSessionDeleteBridge = bridge;
    },
    dispatchWindowEvent(name, event) {
      for (const listener of windowListeners.get(name) || []) {
        listener(event);
      }
    },
    async runNextTimer() {
      const next = timers.entries().next().value;
      assert.ok(next, "a retry timer should be pending");
      const [id, callback] = next;
      timers.delete(id);
      callback();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    },
  };
}

function modelConfig(models, defaultModel) {
  return {
    value: {
      available_models: models,
      default_model: defaultModel,
      untouched: true,
    },
  };
}

function statsigClient(initialModels = ["gpt-5.6-sol", "gpt-5.3-codex"]) {
  const memo = modelConfig(initialModels, "gpt-5.4");
  const external = modelConfig(initialModels, "gpt-5.4");
  const internal = modelConfig(initialModels, "gpt-5.4");
  const events = [];
  return {
    memo,
    external,
    internal,
    events,
    _memoCache: {
      [`c|${MODEL_CONFIG_ID}`]: memo,
    },
    _store: {
      _valuesForExternalUse: {
        dynamic_configs: {
          [MODEL_CONFIG_ID]: external,
        },
      },
      _values: {
        _values: {
          dynamic_configs: {
            [MODEL_CONFIG_ID]: internal,
          },
        },
      },
    },
    getDynamicConfig(name) {
      return name === MODEL_CONFIG_ID
        ? modelConfig(initialModels, "gpt-5.4")
        : { value: { available_models: ["unrelated-model"] } };
    },
    $emt(event) {
      events.push(event);
    },
  };
}

function modelDescriptor(model, isDefault = false) {
  return {
    model,
    id: model,
    displayName: model,
    hidden: false,
    isDefault,
    defaultReasoningEffort: "medium",
    supportedReasoningEfforts: [{
      reasoningEffort: "medium",
      description: "medium effort",
    }],
  };
}

function activeModelQueryClient(initialModels) {
  const queryKey = ["models", "list", "local", "apikey", 100];
  const entries = new Map([[
    JSON.stringify(queryKey),
    {
      queryKey,
      data: {
        data: initialModels.map((model, index) => modelDescriptor(model, index === 0)),
        nextCursor: null,
      },
    },
  ]]);
  let invalidations = 0;
  return {
    get invalidations() {
      return invalidations;
    },
    getQueriesData({ queryKey: prefix }) {
      return [...entries.values()]
        .filter((entry) => prefix.every((value, index) => entry.queryKey[index] === value))
        .map((entry) => [entry.queryKey, entry.data]);
    },
    setQueryData(queryKeyValue, value) {
      const entry = entries.get(JSON.stringify(queryKeyValue));
      assert.ok(entry, "the active model query should exist");
      entry.data = typeof value === "function" ? value(entry.data) : value;
    },
    async invalidateQueries({ queryKey: prefix }) {
      assert.deepEqual(prefix, ["models", "list"]);
      invalidations += 1;
    },
    models() {
      return entries.get(JSON.stringify(queryKey)).data.data.map((model) => model.model);
    },
  };
}

test("runtime whitelist keeps Spark and removes unsupported channel models", async () => {
  const firstClient = statsigClient();
  const secondClient = statsigClient(["gpt-5.6-terra"]);
  const expected = [
    "gpt-5.6-sol",
    "gpt-5.4",
    "gpt-5.3-codex-spark",
    "provider-fast-coder",
  ];
  const { patch } = await loadPatch({
    status: "ok",
    models: expected,
    default_model: "gpt-5.3-codex-spark",
  }, [firstClient, secondClient]);

  assert.deepEqual(patch.snapshot(), {
    loaded: true,
    models: expected,
    defaultModel: "gpt-5.3-codex-spark",
  });
  for (const client of [firstClient, secondClient]) {
    assert.deepEqual(client.memo.value.available_models, expected);
    assert.deepEqual(client.external.value.available_models, expected);
    assert.deepEqual(client.internal.value.available_models, expected);
    assert.equal(client.external.value.default_model, "gpt-5.3-codex-spark");

    const futureConfig = client.getDynamicConfig(MODEL_CONFIG_ID);
    assert.deepEqual(futureConfig.value.available_models, expected);
    assert.equal(futureConfig.value.default_model, "gpt-5.3-codex-spark");
    assert.equal(futureConfig.value.untouched, true);
    assert.deepEqual(
      client.getDynamicConfig("another-config"),
      { value: { available_models: ["unrelated-model"] } },
    );
  }
  assert.equal(expected.includes("gpt-5.3-codex"), false);
  assert.equal(expected.includes("gpt-5.6-terra"), false);
  patch.dispose();
});

test("an explicit refresh hot updates the native model list and default", async () => {
  const client = statsigClient();
  const catalogResponse = {
    status: "ok",
    models: ["gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
  };
  const { patch } = await loadPatch(catalogResponse, [client]);

  catalogResponse.models = ["gpt-5.6-sol", "provider-hot-added"];
  catalogResponse.default_model = "provider-hot-added";
  await patch.refresh();

  assert.deepEqual(patch.snapshot(), {
    loaded: true,
    models: ["gpt-5.6-sol", "provider-hot-added"],
    defaultModel: "provider-hot-added",
  });
  assert.deepEqual(client.external.value.available_models, [
    "gpt-5.6-sol",
    "provider-hot-added",
  ]);
  assert.equal(client.external.value.default_model, "provider-hot-added");
  patch.dispose();
});

test("a backend-pushed catalog updates immediately without a nested bridge request", async () => {
  const client = statsigClient();
  const queryClient = activeModelQueryClient(["gpt-5.6-sol"]);
  const runtime = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
  }, [client], { queryClient });
  const { patch } = runtime;
  const eventsBeforePush = client.events.length;

  assert.equal(patch.version, "3");
  assert.equal(await patch.setCatalog({
    status: "ok",
    models: ["gpt-5.6-sol", "provider-hot-pushed"],
    default_model: "provider-hot-pushed",
  }), true);
  assert.deepEqual(patch.snapshot(), {
    loaded: true,
    models: ["gpt-5.6-sol", "provider-hot-pushed"],
    defaultModel: "provider-hot-pushed",
  });
  assert.deepEqual(client.external.value.available_models, [
    "gpt-5.6-sol",
    "provider-hot-pushed",
  ]);
  assert.equal(client.external.value.default_model, "provider-hot-pushed");
  assert.ok(client.events.length > eventsBeforePush);
  assert.equal(client.events.at(-1).name, "values_updated");
  assert.deepEqual(queryClient.models(), [
    "gpt-5.6-sol",
    "provider-hot-pushed",
  ]);
  assert.ok(queryClient.invalidations > 0);
  assert.deepEqual(patch.delivery(), {
    revision: 2,
    statsigClients: 1,
    notifiedClients: 1,
    queryClients: 1,
    queryEntries: 1,
    reactContainers: 0,
    responsePatchInstalled: true,
  });

  runtime.dispatchWindowEvent("codex-message-from-view", {
    detail: {
      type: "mcp-request",
      request: {
        id: 41,
        method: "model/list",
        params: {},
      },
    },
  });
  const response = {
    data: {
      type: "mcp-response",
      message: {
        id: 41,
        result: {
          data: [modelDescriptor("provider-stale")],
          nextCursor: null,
        },
      },
    },
  };
  runtime.dispatchWindowEvent("message", response);
  assert.deepEqual(
    response.data.message.result.data.map((model) => model.model),
    ["gpt-5.6-sol", "provider-hot-pushed"],
  );
  patch.dispose();
});

test("a stale bridge response cannot overwrite a backend-pushed catalog", async () => {
  const client = statsigClient();
  let resolveCatalog;
  const staleCatalog = new Promise((resolve) => {
    resolveCatalog = resolve;
  });
  const runtime = await loadPatch(() => staleCatalog, [client], {
    bridgeReady: false,
  });
  runtime.connectBridge();
  await Promise.resolve();
  await Promise.resolve();
  const staleRefresh = runtime.patch.refresh();

  assert.equal(await runtime.patch.setCatalog({
    status: "ok",
    models: ["provider-current"],
    default_model: "provider-current",
  }), true);
  resolveCatalog({
    status: "ok",
    models: ["provider-stale"],
    default_model: "provider-stale",
  });
  await staleRefresh;

  assert.deepEqual(runtime.patch.snapshot(), {
    loaded: true,
    models: ["provider-current"],
    defaultModel: "provider-current",
  });
  runtime.patch.dispose();
});

test("a synced channel with no supported models clears the native allowlist", async () => {
  const client = statsigClient();
  const { patch } = await loadPatch({
    status: "not_configured",
    models: [],
    default_model: "",
  }, [client]);

  assert.deepEqual(client.external.value.available_models, []);
  assert.equal(client.external.value.default_model, "");
  assert.deepEqual(
    client.getDynamicConfig(MODEL_CONFIG_ID).value.available_models,
    [],
  );
  patch.dispose();
});

test("the catalog load retries when the bridge appears after injection", async () => {
  const client = statsigClient();
  const runtime = await loadPatch({
    status: "ok",
    models: ["gpt-5.3-codex-spark"],
    default_model: "gpt-5.3-codex-spark",
  }, [client], { bridgeReady: false });

  assert.equal(runtime.patch.snapshot().loaded, false);
  runtime.connectBridge();
  await runtime.runNextTimer();

  assert.deepEqual(runtime.patch.snapshot(), {
    loaded: true,
    models: ["gpt-5.3-codex-spark"],
    defaultModel: "gpt-5.3-codex-spark",
  });
  assert.deepEqual(client.external.value.available_models, ["gpt-5.3-codex-spark"]);
  runtime.patch.dispose();
});

test("failed catalog responses preserve the native allowlist", async () => {
  const client = statsigClient();
  const { patch } = await loadPatch({
    status: "failed",
    message: "catalog unavailable",
  }, [client]);

  assert.equal(patch.snapshot().loaded, false);
  assert.deepEqual(
    client.external.value.available_models,
    ["gpt-5.6-sol", "gpt-5.3-codex"],
  );
  patch.dispose();
});

test("frozen Statsig results and Map memo caches receive patched copies", async () => {
  const frozenConfig = Object.freeze({
    value: Object.freeze({
      available_models: ["gpt-5.3-codex"],
      default_model: "gpt-5.3-codex",
    }),
  });
  const memoCache = new Map([[`c|${MODEL_CONFIG_ID}`, frozenConfig]]);
  const client = {
    _memoCache: memoCache,
    getDynamicConfig: () => frozenConfig,
  };
  const { patch } = await loadPatch({
    status: "ok",
    models: ["gpt-5.3-codex-spark"],
    default_model: "gpt-5.3-codex-spark",
  }, [client]);

  assert.notEqual(memoCache.get(`c|${MODEL_CONFIG_ID}`), frozenConfig);
  assert.deepEqual(
    memoCache.get(`c|${MODEL_CONFIG_ID}`).value.available_models,
    ["gpt-5.3-codex-spark"],
  );
  assert.deepEqual(
    client.getDynamicConfig(MODEL_CONFIG_ID).value.available_models,
    ["gpt-5.3-codex-spark"],
  );
  patch.dispose();
});
