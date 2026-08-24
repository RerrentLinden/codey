import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { FakeElementCore } from "./helpers/fake-element.mjs";

const MODEL_CONFIG_ID = "107580212";

async function loadPatch(
  catalogResponse,
  clients,
  { bridgeReady = true, queryClient = null, documentBody = null } = {},
) {
  const [bridgeSource, source] = await Promise.all([
    readFile(new URL("../public/codey-bridge.js", import.meta.url), "utf8"),
    readFile(new URL("../public/model-whitelist-inject.js", import.meta.url), "utf8"),
  ]);
  let nextTimer = 0;
  const timers = new Map();
  const windowListeners = new Map();
  const documentListeners = new Map();
  let wildcardScanCount = 0;
  const body = documentBody || {};
  if (queryClient) {
    body.__reactFiber$codeyTest = {
      memoizedProps: {
        queryClient,
      },
    };
  }
  const head = documentBody ? new FakeElementCore("head") : null;
  const documentElement = documentBody ? new FakeElementCore("html") : {};
  const allDocumentRoots = () => [head, body, documentElement].filter(Boolean);
  const document = {
    body,
    documentElement,
    head,
    createElement(tagName) {
      return documentBody ? new FakeElementCore(tagName) : null;
    },
    getElementById() {
      return allDocumentRoots()
        .map((root) => root.querySelector?.("[id]"))
        .find(Boolean) || null;
    },
    querySelectorAll(selector) {
      if (selector === "*") wildcardScanCount += 1;
      return documentBody ? body.querySelectorAll(selector) : [];
    },
    addEventListener(name, listener) {
      const listeners = documentListeners.get(name) || new Set();
      listeners.add(listener);
      documentListeners.set(name, listeners);
    },
    removeEventListener(name, listener) {
      documentListeners.get(name)?.delete(listener);
    },
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
    dispatchEvent(event) {
      for (const listener of windowListeners.get(event?.type) || []) {
        listener(event);
      }
      return true;
    },
  };
  if (bridgeReady) window.__codexSessionDeleteBridge = bridge;
  Function("window", "document", "globalThis", "console", bridgeSource)(
    window,
    document,
    window,
    { warn() {} },
  );
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
      window.dispatchEvent({ ...event, type: name });
    },
    dispatchDocumentEvent(name, event = {}) {
      for (const listener of documentListeners.get(name) || []) {
        listener({ ...event, type: name });
      }
    },
    wildcardScanCount() {
      return wildcardScanCount;
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
    model(modelName) {
      return entries
        .get(JSON.stringify(queryKey))
        .data
        .data
        .find((model) => model.model === modelName);
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

  assert.equal(patch.version, "14");
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

test("stale thread and turn models are repaired before app-server dispatch", async () => {
  const runtime = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol", "gpt-5.6-terra"],
    default_model: "gpt-5.6-sol",
  }, [statsigClient()]);

  for (const method of ["thread/start", "thread/resume", "turn/start"]) {
    const event = {
      detail: {
        type: "mcp-request",
        request: {
          id: method,
          method,
          params: {
            threadId: "stale-thread",
            model: "claude-opus-4-8",
          },
        },
      },
    };
    runtime.dispatchWindowEvent("codex-message-from-view", event);
    assert.equal(event.detail.request.params.model, "gpt-5.6-sol");
  }
  runtime.patch.dispose();
});

test("route aliases display clearly and dispatch to the selected provider", async () => {
  const queryClient = activeModelQueryClient(["stale-model"]);
  const routeCatalog = {
    status: "ok",
    models: ["route-a/shared-model", "route-b/shared-model"],
    default_model: "route-a/shared-model",
    model_metadata: [
      {
        model: "route-a/shared-model",
        display_name: "主线路 / shared-model",
        route_name: "主线路",
        provider_id: "route-a",
        source_model: "shared-model",
      },
      {
        model: "route-b/shared-model",
        display_name: "备用线路 / shared-model",
        route_name: "备用线路",
        provider_id: "route-b",
        source_model: "shared-model",
      },
    ],
  };
  const runtime = await loadPatch(routeCatalog, [statsigClient()], { queryClient });

  assert.equal(
    queryClient.model("route-a/shared-model").displayName,
    "主线路 / shared-model",
  );
  assert.equal(queryClient.model("route-a/shared-model").routeName, "主线路");
  assert.equal(queryClient.model("route-a/shared-model").codeyModelName, "shared-model");
  assert.equal(
    queryClient.model("route-b/shared-model").displayName,
    "备用线路 / shared-model",
  );

  const direct = {
    detail: {
      type: "mcp-request",
      request: {
        id: "route-direct",
        method: "turn/start",
        params: { model: "route-b/shared-model" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", direct);
  assert.deepEqual(direct.detail.request.params, {
    model: "shared-model",
    model_provider: "route-b",
  });

  const wrapped = {
    detail: {
      type: "mcp-request",
      request: {
        id: "route-wrapped",
        method: "send-cli-request-for-host",
        params: {
          method: "thread/start",
          params: { model: "route-a/shared-model" },
        },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", wrapped);
  assert.deepEqual(wrapped.detail.request.params.params, {
    model: "shared-model",
    model_provider: "route-a",
  });

  const resumed = {
    detail: {
      type: "mcp-request",
      request: {
        id: "route-resumed",
        method: "thread/resume",
        params: { model: "shared-model", model_provider: "route-b" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", resumed);
  assert.deepEqual(resumed.detail.request.params, {
    model: "shared-model",
    model_provider: "route-b",
  });

  await runtime.patch.setCatalog({
    ...routeCatalog,
    model_metadata: routeCatalog.model_metadata.map((metadata) =>
      metadata.provider_id === "route-b"
        ? { ...metadata, display_name: "灾备线路 / shared-model" }
        : metadata,
    ),
  });
  assert.equal(
    queryClient.model("route-b/shared-model").displayName,
    "灾备线路 / shared-model",
  );

  await runtime.patch.setCatalog({
    status: "ok",
    models: ["route-a/shared-model"],
    default_model: "route-a/shared-model",
    model_metadata: [routeCatalog.model_metadata[0]],
  });
  const deletedRouteRequest = {
    detail: {
      type: "mcp-request",
      request: {
        id: "deleted-route",
        method: "turn/start",
        params: {
          model: "route-b/shared-model",
          model_provider: "route-b",
        },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", deletedRouteRequest);
  assert.deepEqual(deletedRouteRequest.detail.request.params, {
    model: "shared-model",
    model_provider: "route-a",
  });
  runtime.patch.dispose();
});

test("local-router aliases stay scoped when selected from an official route", async () => {
  const runtime = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol", "relay/shared-model"],
    default_model: "gpt-5.6-sol",
    model_metadata: [
      {
        model: "gpt-5.6-sol",
        display_name: "官方线路 / gpt-5.6-sol",
        route_name: "官方线路",
        provider_id: "openai",
        source_model: "gpt-5.6-sol",
      },
      {
        model: "relay/shared-model",
        display_name: "中转线路 / shared-model",
        route_name: "中转线路",
        provider_id: "codey_router",
        source_model: "relay/shared-model",
        route_provider_id: "relay",
        upstream_model: "shared-model",
      },
    ],
  }, [statsigClient()]);

  const selected = {
    detail: {
      type: "mcp-request",
      request: {
        id: "select-local-router-model",
        method: "turn/start",
        params: { model: "relay/shared-model", model_provider: "openai" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", selected);
  assert.deepEqual(selected.detail.request.params, {
    model: "relay/shared-model",
    model_provider: "codey_router",
  });

  const staleDirectProvider = {
    detail: {
      type: "mcp-request",
      request: {
        id: "repair-old-direct-provider",
        method: "thread/resume",
        params: { model: "relay/shared-model", model_provider: "relay" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", staleDirectProvider);
  assert.deepEqual(staleDirectProvider.detail.request.params, {
    model: "relay/shared-model",
    model_provider: "codey_router",
  });
  runtime.patch.dispose();
});

test("model picker menu groups models under route headings without changing model ids", async () => {
  const body = new FakeElementCore("body", { connected: true });
  const menu = body.appendChild(new FakeElementCore("div", {
    attributes: { role: "menu" },
  }));
  const officialItem = menu.appendChild(new FakeElementCore("div", {
    attributes: { role: "menuitemradio" },
  }));
  officialItem.textContent = "官方线路 / gpt-5.6-sol";
  const relayItem = menu.appendChild(new FakeElementCore("div", {
    attributes: { role: "menuitemradio" },
  }));
  relayItem.textContent = "中转线路 / gpt-5.6-sol";

  const runtime = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol", "relay/gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
    model_metadata: [
      {
        model: "gpt-5.6-sol",
        display_name: "官方线路 / gpt-5.6-sol",
        route_name: "官方线路",
        provider_id: "openai",
        source_model: "gpt-5.6-sol",
      },
      {
        model: "relay/gpt-5.6-sol",
        display_name: "中转线路 / gpt-5.6-sol",
        route_name: "中转线路",
        provider_id: "relay",
        source_model: "gpt-5.6-sol",
      },
    ],
  }, [statsigClient()], { documentBody: body });

  runtime.patch.enhanceModelMenus();

  assert.equal(menu.children[0].textContent, "官方线路");
  assert.equal(menu.children[1], officialItem);
  assert.equal(officialItem.textContent, "gpt-5.6-sol");
  assert.equal(officialItem.dataset.codeyRouteModel, "gpt-5.6-sol");
  assert.equal(officialItem.getAttribute("aria-label"), "官方线路 / gpt-5.6-sol");
  assert.equal(menu.children[2].textContent, "中转线路");
  assert.equal(menu.children[3], relayItem);
  assert.equal(relayItem.textContent, "gpt-5.6-sol");
  assert.equal(relayItem.dataset.codeyRouteModel, "relay/gpt-5.6-sol");
  assert.equal(relayItem.getAttribute("aria-label"), "中转线路 / gpt-5.6-sol");

  const originalHeadings = [menu.children[0], menu.children[2]];
  runtime.patch.enhanceModelMenus();
  assert.equal(menu.children.length, 4);
  assert.equal(menu.children[0], originalHeadings[0]);
  assert.equal(menu.children[2], originalHeadings[1]);

  const request = {
    detail: {
      type: "mcp-request",
      request: {
        id: "grouped-menu-selected-relay",
        method: "turn/start",
        params: { model: "relay/gpt-5.6-sol" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", request);
  assert.deepEqual(request.detail.request.params, {
    model: "gpt-5.6-sol",
    model_provider: "relay",
  });
  runtime.patch.dispose();
});

test("official account route models keep raw ids and dispatch to the OpenAI provider", async () => {
  const queryClient = activeModelQueryClient(["stale-model"]);
  const runtime = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
    model_metadata: [{
      model: "gpt-5.6-sol",
      display_name: "OpenAI 官方直登 / gpt-5.6-sol",
      route_name: "OpenAI 官方直登",
      provider_id: "openai",
      source_model: "gpt-5.6-sol",
    }],
  }, [statsigClient()], { queryClient });

  assert.equal(
    queryClient.model("gpt-5.6-sol").displayName,
    "OpenAI 官方直登 / gpt-5.6-sol",
  );
  assert.equal(queryClient.model("gpt-5.6-sol").routeName, "OpenAI 官方直登");

  const request = {
    detail: {
      type: "mcp-request",
      request: {
        id: "official-raw-model",
        method: "turn/start",
        params: { model: "gpt-5.6-sol" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", request);

  assert.deepEqual(request.detail.request.params, {
    model: "gpt-5.6-sol",
    model_provider: "openai",
  });
  runtime.patch.dispose();
});

test("official OpenAI route aliases dispatch raw model ids through the OpenAI provider from a relay default", async () => {
  const runtime = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol", "relay/gpt-5.6-sol"],
    default_model: "relay/gpt-5.6-sol",
    model_metadata: [
      {
        model: "gpt-5.6-sol",
        display_name: "官方线路 / gpt-5.6-sol",
        provider_id: "openai",
        source_model: "gpt-5.6-sol",
      },
      {
        model: "relay/gpt-5.6-sol",
        display_name: "中转线路 / gpt-5.6-sol",
        provider_id: "relay",
        source_model: "gpt-5.6-sol",
      },
    ],
  }, [statsigClient()]);

  const official = {
    detail: {
      type: "mcp-request",
      request: {
        id: "official-route",
        method: "turn/start",
        params: {
          model: "openai/gpt-5.6-sol",
          model_provider: "openai",
        },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", official);
  assert.deepEqual(official.detail.request.params, {
    model: "gpt-5.6-sol",
    model_provider: "openai",
  });

  const currentOfficial = {
    detail: {
      type: "mcp-request",
      request: {
        id: "official-current",
        method: "turn/start",
        params: {
          model: "gpt-5.6-sol",
          model_provider: "openai",
        },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", currentOfficial);
  assert.deepEqual(currentOfficial.detail.request.params, {
    model: "gpt-5.6-sol",
    model_provider: "openai",
  });

  const relay = {
    detail: {
      type: "mcp-request",
      request: {
        id: "relay-route",
        method: "turn/start",
        params: { model: "relay/gpt-5.6-sol" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", relay);
  assert.deepEqual(relay.detail.request.params, {
    model: "gpt-5.6-sol",
    model_provider: "relay",
  });
  runtime.patch.dispose();
});

test("official route selection does not inherit an active third party provider", async () => {
  const runtime = await loadPatch({
    status: "ok",
    model: "relay/gpt-5.6-sol",
    default_model: "relay/gpt-5.6-sol",
    model_provider: "relay",
    models: ["gpt-5.6-terra", "relay/gpt-5.6-sol"],
    model_metadata: [
      {
        model: "gpt-5.6-terra",
        display_name: "OpenAI 官方直登 / gpt-5.6-terra",
        route_name: "OpenAI 官方直登",
        provider_id: "openai",
        source_model: "gpt-5.6-terra",
      },
      {
        model: "relay/gpt-5.6-sol",
        display_name: "第三方线路 / gpt-5.6-sol",
        route_name: "第三方线路",
        provider_id: "relay",
        source_model: "gpt-5.6-sol",
      },
    ],
  }, [statsigClient()]);

  const request = {
    detail: {
      type: "mcp-request",
      request: {
        id: "official-from-third-party-runtime",
        method: "turn/start",
        params: { model: "gpt-5.6-terra" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", request);

  assert.deepEqual(request.detail.request.params, {
    model: "gpt-5.6-terra",
    model_provider: "openai",
  });

  const staleProviderRequest = {
    detail: {
      type: "mcp-request",
      request: {
        id: "official-from-stale-third-party-provider",
        method: "turn/start",
        params: { model: "gpt-5.6-terra", model_provider: "relay" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", staleProviderRequest);

  assert.deepEqual(staleProviderRequest.detail.request.params, {
    model: "gpt-5.6-terra",
    model_provider: "openai",
  });
  runtime.patch.dispose();
});

test("model IDs dedupe and match without case drift", async () => {
  const runtime = await loadPatch({
    status: "ok",
    models: ["Provider-Coder", " provider-coder ", "Provider-Reasoner"],
    default_model: "provider-coder",
  }, [statsigClient()]);

  assert.deepEqual(runtime.patch.snapshot(), {
    loaded: true,
    models: ["Provider-Coder", "Provider-Reasoner"],
    defaultModel: "Provider-Coder",
  });
  const event = {
    detail: {
      type: "mcp-request",
      request: {
        id: "case-insensitive-model",
        method: "turn/start",
        params: { model: "PROVIDER-REASONER" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", event);
  assert.equal(event.detail.request.params.model, "Provider-Reasoner");
  runtime.patch.dispose();
});

test("unchanged catalog retries and interactions do not repeat full React discovery", async () => {
  const catalog = {
    status: "ok",
    models: ["gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
  };
  const runtime = await loadPatch(catalog, [statsigClient()]);

  assert.equal(runtime.wildcardScanCount(), 1);
  runtime.dispatchDocumentEvent("pointerdown");
  runtime.dispatchDocumentEvent("focusin");
  await Promise.resolve();
  assert.equal(runtime.wildcardScanCount(), 1);

  await runtime.patch.setCatalog(catalog);
  assert.equal(runtime.wildcardScanCount(), 1);
  assert.equal(runtime.patch.delivery().revision, 1);

  await runtime.runNextTimer();
  await runtime.runNextTimer();
  assert.equal(runtime.wildcardScanCount(), 1);

  await runtime.patch.setCatalog({
    ...catalog,
    models: ["gpt-5.6-sol", "provider-new"],
  });
  assert.equal(runtime.wildcardScanCount(), 2);
  assert.equal(runtime.patch.delivery().revision, 2);
  runtime.patch.dispose();
});

test("configured third-party models survive direct and wrapped requests", async () => {
  const runtime = await loadPatch({
    status: "ok",
    models: ["claude-opus-4-8", "deepseek-reasoner"],
    default_model: "claude-opus-4-8",
  }, [statsigClient()]);
  const direct = {
    detail: {
      type: "mcp-request",
      request: {
        method: "turn/start",
        params: { threadId: "valid-thread", model: "deepseek-reasoner" },
      },
    },
  };
  const wrapped = {
    detail: {
      type: "mcp-request",
      request: {
        method: "send-cli-request-for-host",
        params: {
          hostId: "local",
          method: "turn/start",
          params: { threadId: "valid-thread", model: "deepseek-reasoner" },
        },
      },
    },
  };

  runtime.dispatchWindowEvent("codex-message-from-view", direct);
  runtime.dispatchWindowEvent("codex-message-from-view", wrapped);

  assert.equal(direct.detail.request.params.model, "deepseek-reasoner");
  assert.equal(
    wrapped.detail.request.params.params.model,
    "deepseek-reasoner",
  );
  runtime.patch.dispose();
});

test("valid current-route models survive direct and wrapped requests", async () => {
  const runtime = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol", "gpt-5.6-terra"],
    default_model: "gpt-5.6-sol",
  }, [statsigClient()]);
  const direct = {
    detail: {
      type: "mcp-request",
      request: {
        method: "turn/start",
        params: { threadId: "valid-thread", model: "gpt-5.6-terra" },
      },
    },
  };
  const wrapped = {
    detail: {
      type: "mcp-request",
      request: {
        method: "send-cli-request-for-host",
        params: {
          hostId: "local",
          method: "turn/start",
          params: { threadId: "stale-thread", model: "claude-opus-4-8" },
        },
      },
    },
  };

  runtime.dispatchWindowEvent("codex-message-from-view", direct);
  runtime.dispatchWindowEvent("codex-message-from-view", wrapped);

  assert.equal(direct.detail.request.params.model, "gpt-5.6-terra");
  assert.equal(
    wrapped.detail.request.params.params.model,
    "gpt-5.6-sol",
  );
  runtime.patch.dispose();
});

test("missing turn model receives the current route default", async () => {
  const runtime = await loadPatch({
    status: "ok",
    models: ["provider-current"],
    default_model: "provider-current",
  }, [statsigClient()]);
  const event = {
    detail: {
      type: "mcp-request",
      request: {
        method: "turn/start",
        params: { threadId: "legacy-thread" },
      },
    },
  };
  runtime.dispatchWindowEvent("codex-message-from-view", event);

  assert.equal(event.detail.request.params.model, "provider-current");
  runtime.patch.dispose();
});

test("an unchanged model list repairs missing reasoning effort options", async () => {
  const client = statsigClient();
  const queryClient = activeModelQueryClient(["gpt-5.6-sol"]);
  const existing = queryClient.model("gpt-5.6-sol");
  existing.supportedReasoningEfforts = [];
  delete existing.defaultReasoningEffort;

  const { patch } = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
  }, [client], { queryClient });

  const repaired = queryClient.model("gpt-5.6-sol");
  assert.deepEqual(
    repaired.supportedReasoningEfforts.map((effort) => effort.reasoningEffort),
    ["minimal", "low", "medium", "high", "xhigh"],
  );
  assert.equal(repaired.defaultReasoningEffort, "medium");
  patch.dispose();
});

test("an unchanged model list repairs missing native Fast tiers", async () => {
  const client = statsigClient();
  const queryClient = activeModelQueryClient(["gpt-5.6-sol"]);
  const existing = queryClient.model("gpt-5.6-sol");
  existing.serviceTiers = [{
    id: "standard",
    name: "Standard",
    description: "Default speed",
  }];
  existing.additionalSpeedTiers = ["standard"];
  delete existing.defaultServiceTier;

  const { patch } = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
  }, [client], { queryClient });

  const repaired = queryClient.model("gpt-5.6-sol");
  assert.deepEqual(repaired.serviceTiers, [
    {
      id: "standard",
      name: "Standard",
      description: "Default speed",
    },
    {
      id: "priority",
      name: "Fast",
      description: "1.5x speed, increased usage",
    },
  ]);
  assert.deepEqual(repaired.additionalSpeedTiers, ["standard", "fast"]);
  assert.equal(repaired.defaultServiceTier, null);
  patch.dispose();
});

test("catalog model metadata overrides stale native reasoning efforts", async () => {
  const client = statsigClient();
  const queryClient = activeModelQueryClient(["gpt-5.6-sol"]);

  const { patch } = await loadPatch({
    status: "ok",
    models: ["gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
    model_metadata: [{
      model: "gpt-5.6-sol",
      supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
      default_reasoning_effort: "low",
    }],
  }, [client], { queryClient });

  const repaired = queryClient.model("gpt-5.6-sol");
  assert.deepEqual(
    repaired.supportedReasoningEfforts.map((effort) => effort.reasoningEffort),
    ["low", "medium", "high", "xhigh", "max", "ultra"],
  );
  assert.equal(repaired.defaultReasoningEffort, "low");
  patch.dispose();
});

test("third-party metadata replaces a stale high-only native descriptor", async () => {
  const client = statsigClient();
  const queryClient = activeModelQueryClient(["provider-fast-coder"]);
  const stale = queryClient.model("provider-fast-coder");
  stale.defaultReasoningEffort = "high";
  stale.supportedReasoningEfforts = [{
    reasoningEffort: "high",
    description: "high effort",
  }];

  const { patch } = await loadPatch({
    status: "ok",
    models: ["provider-fast-coder"],
    default_model: "provider-fast-coder",
    model_metadata: [{
      model: "provider-fast-coder",
      supported_reasoning_efforts: ["low", "medium", "high", "xhigh"],
      default_reasoning_effort: "low",
    }],
  }, [client], { queryClient });

  const repaired = queryClient.model("provider-fast-coder");
  assert.deepEqual(
    repaired.supportedReasoningEfforts.map((effort) => effort.reasoningEffort),
    ["low", "medium", "high", "xhigh"],
  );
  assert.equal(repaired.defaultReasoningEffort, "low");
  patch.dispose();
});

test("a refresh applies changed reasoning metadata when model ids stay unchanged", async () => {
  const client = statsigClient();
  const queryClient = activeModelQueryClient(["gpt-5.6-sol"]);
  const catalogResponse = {
    status: "ok",
    models: ["gpt-5.6-sol"],
    default_model: "gpt-5.6-sol",
    model_metadata: [{
      model: "gpt-5.6-sol",
      supported_reasoning_efforts: ["low", "medium"],
      default_reasoning_effort: "low",
    }],
  };
  const { patch } = await loadPatch(catalogResponse, [client], { queryClient });

  catalogResponse.model_metadata[0] = {
    model: "gpt-5.6-sol",
    supported_reasoning_efforts: ["low", "medium", "high", "xhigh", "max", "ultra"],
    default_reasoning_effort: "high",
  };
  await patch.refresh();

  const refreshed = queryClient.model("gpt-5.6-sol");
  assert.deepEqual(
    refreshed.supportedReasoningEfforts.map((effort) => effort.reasoningEffort),
    ["low", "medium", "high", "xhigh", "max", "ultra"],
  );
  assert.equal(refreshed.defaultReasoningEffort, "high");
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
