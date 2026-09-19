(() => {
  if (window.__codeyPluginMarketplaceFixInstalled) {
    if (typeof window.__codeyEnsurePluginBridge === "function") {
      window.__codeyEnsurePluginBridge();
      return;
    }
    window.__codeyPluginMarketplaceFixInstalled = false;
  }
  window.__codeyPluginMarketplaceFixInstalled = true;
  const bridge = (path, payload = {}) => {
    const call = window.__codeyCall || window.__codeyBridge;
    return typeof call === "function" ? call(path, payload) : Promise.resolve({ status: "failed" });
  };
  window.__codeyLocalPlugins = [];
  let pluginRefreshPromise = null;
  let pluginRefreshQueued = false;
  const refreshLocalPlugins = (queueAfterInflight = false) => {
    if (pluginRefreshPromise) {
      if (queueAfterInflight) pluginRefreshQueued = true;
      return pluginRefreshPromise;
    }
    pluginRefreshPromise = Promise.resolve()
      .then(() => bridge("/plugins/list", {}))
      .then((result) => {
        if (result?.status === "failed") return;
        window.__codeyLocalPlugins = Array.isArray(result?.plugins) ? result.plugins : [];
      })
      .catch(() => {})
      .finally(() => {
        pluginRefreshPromise = null;
        if (pluginRefreshQueued) {
          pluginRefreshQueued = false;
          void refreshLocalPlugins();
        }
      });
    return pluginRefreshPromise;
  };
  const waitForLocalPlugins = () => {
    const refresh = refreshLocalPlugins();
    return new Promise((resolve) => {
      let settled = false;
      const finish = () => {
        if (settled) return;
        settled = true;
        window.clearTimeout(timer);
        resolve();
      };
      const timer = window.setTimeout(finish, 2_000);
      Promise.resolve(refresh).then(finish, finish);
    });
  };
  const pluginLike = (value) => value && typeof value === "object" && ("name" in value || "id" in value) && ("marketplace" in value || "marketplaceName" in value || "marketplacePath" in value || "hidden" in value);
  const normalizePlugin = (plugin) => {
    if (!pluginLike(plugin)) return plugin;
    const output = { ...plugin };
    if (output.hidden === true) output.hidden = false;
    if (!output.marketplaceName) output.marketplaceName = output.marketplace || output.remoteName || "openai-curated";
    if (!output.marketplacePath) output.marketplacePath = output.path || output.localPath || output.marketplaceName;
    return output;
  };
  const mergePlugins = (
    value,
    depth = 0,
    seen = new WeakMap(),
    budget = { remaining: 512 },
  ) => {
    if (!value || typeof value !== "object") return value;
    if (seen.has(value)) return seen.get(value);
    if (depth >= 12 || budget.remaining <= 0) return value;
    if (Array.isArray(value)) {
      const current = [];
      seen.set(value, current);
      for (const child of value) {
        if (budget.remaining <= 0) {
          current.push(child);
          continue;
        }
        budget.remaining -= 1;
        current.push(mergePlugins(child, depth + 1, seen, budget));
      }
      const existing = new Set(current.filter(pluginLike).map((plugin) => plugin.id || `${plugin.name}@${plugin.marketplaceName || ""}`));
      for (const plugin of window.__codeyLocalPlugins || []) {
        const normalized = normalizePlugin(plugin);
        const key = normalized.id || `${normalized.name}@${normalized.marketplaceName || ""}`;
        if (!existing.has(key)) current.push(normalized);
      }
      return current;
    }
    const output = normalizePlugin(value);
    seen.set(value, output);
    for (const [key, child] of Object.entries(output)) {
      if (!child || typeof child !== "object") continue;
      if (budget.remaining <= 0) break;
      budget.remaining -= 1;
      output[key] = mergePlugins(child, depth + 1, seen, budget);
    }
    return output;
  };
  const patchResponse = (value) => mergePlugins(value);
  window.__codeyPatchPluginResponse = patchResponse;
  const normalizeRequest = (
    value,
    depth = 0,
    seen = new WeakMap(),
    budget = { remaining: 128 },
  ) => {
    if (!value || typeof value !== "object" || depth >= 8 || budget.remaining <= 0) return value;
    if (seen.has(value)) return seen.get(value);
    let entries;
    try {
      entries = Object.entries(value);
    } catch {
      return value;
    }
    const output = Array.isArray(value) ? [] : {};
    seen.set(value, output);
    for (const [key, child] of entries) {
      if (budget.remaining <= 0) {
        output[key] = child;
        continue;
      }
      budget.remaining -= 1;
      if (key === "includeHidden" || key === "includeRemote") {
        output[key] = true;
      } else {
        output[key] = normalizeRequest(child, depth + 1, seen, budget);
      }
    }
    return output;
  };
  const normalizeRequestArg = (value) => {
    if (typeof value !== "string") {
      try { return normalizeRequest(value); } catch { return value; }
    }
    try { return JSON.stringify(normalizeRequest(JSON.parse(value))); } catch { return value; }
  };

  const directRequestKeys = ["channel", "command", "method", "action", "type", "path", "topic", "url"];
  const envelopeKeys = ["payload", "request", "body"];
  const envelopeCommands = new Set(["invoke", "request", "rpc", "mcp-request"]);
  const requestPath = (value) => value.trim().toLowerCase()
    .split(/[?#]/, 1)[0].replace(/^(?:[a-z][a-z\d+.-]*:)?\/\/[^/]+/, "");
  const pluginRoute = (value) => typeof value === "string"
    && /(?:^|\/)(?:plugins?|marketplace)(?:\/|$)/.test(requestPath(value));
  const pluginCommand = (value, route = false) => {
    if (typeof value !== "string") return null;
    const name = requestPath(value);
    const legacy = /^(?:\/?[a-z\d_-]+[/:.])*(?:list-plugins|install-plugin|uninstall-plugin)$/.test(name.replace(/^\//, ""));
    const namespaced = route ? pluginRoute(value)
      : /^[a-z\d_-]+(?:[/:.][a-z\d_-]+)*$/.test(name.replace(/^\//, ""))
        && /(?:^|[/:.])(?:plugins?|marketplace)(?:[/:.]|$)/.test(name);
    if (!legacy && !namespaced) return null;
    return {
      list: /(?:^|[/:.])list-plugins$/.test(name) || /(?:^|[/:.])(?:plugins?|marketplace)[/:.]list$/.test(name),
      mutation: /(?:^|[/:.])(?:install-plugin|uninstall-plugin)$/.test(name)
        || /(?:^|[/:.])(?:plugins?|marketplace)[/:.](?:install|uninstall)$/.test(name),
    };
  };
  const isStructuredRequestBody = (value) => {
    if (!value || typeof value !== "object") return false;
    if (Array.isArray(value)) return false;
    try {
      return Object.prototype.toString.call(value) === "[object Object]";
    } catch {
      return true;
    }
  };
  const pluginRequest = (
    value,
    depth = 0,
    seen = new WeakSet(),
    budget = { remaining: 24 },
  ) => {
    if (typeof value === "string") {
      if (!/plugin|marketplace/i.test(value)) return null;
      try { value = JSON.parse(value); } catch { return null; }
    }
    if (!isStructuredRequestBody(value) || depth >= 4 || seen.has(value) || budget.remaining <= 0) {
      return null;
    }
    seen.add(value);
    let hasCommand = false;
    let isEnvelope = false;
    let hasApplicationMethod = false;
    for (const key of directRequestKeys) {
      let marker;
      try {
        marker = value[key];
      } catch {
        continue;
      }
      const command = pluginCommand(marker, key === "path" || key === "url");
      if (command) return command;
      if (typeof marker === "string") {
        hasCommand = true;
        if (envelopeCommands.has(marker.toLowerCase())) isEnvelope = true;
        else if (key === "method" || key === "command" || key === "action") hasApplicationMethod = true;
      }
    }
    // Only transport envelopes contain another command. Application payloads
    // such as chat prompts, tool parameters and descriptions are opaque here.
    if (hasApplicationMethod || (hasCommand && !isEnvelope)) return null;
    for (const key of envelopeKeys) {
      if (budget.remaining-- <= 0) break;
      try {
        const command = pluginRequest(value[key], depth + 1, seen, budget);
        if (command) return command;
      } catch {}
    }
    return null;
  };
  const pluginRequestArgs = (args) => {
    const first = args[0];
    if (typeof first !== "string") return pluginRequest(first);
    const command = pluginCommand(first);
    if (command) return command;
    if (envelopeCommands.has(first.toLowerCase())) return pluginRequest(args[1]);
    return pluginRequest(first);
  };

  let bridgeRetryTimer = 0;
  let bridgeRetryDelay = 50;
  let bridgeRetryDeadline = Date.now() + 30_000;
  // 慢速重试（30s 周期）封顶：桥长期缺席时不再无限期探测。
  // __codeyEnsurePluginBridge 会重置计数并重新打开 30s 快速窗口。
  const MAX_SLOW_BRIDGE_RETRIES = 20;
  let bridgeSlowRetries = 0;
  const markPluginBridgeEffective = () => {
    const entry = window.__codeyInjectionStatus?.["plugin-marketplace-compatibility"];
    if (!entry || entry.status === "failed") return;
    const changed = entry.status !== "effective" || entry.detail !== "插件市场桥接已接管";
    entry.status = "effective";
    entry.detail = "插件市场桥接已接管";
    entry.error = null;
    if (changed) {
      window.dispatchEvent(new CustomEvent("codey-injection-status-changed", {
        detail: { id: "plugin-marketplace-compatibility", status: "effective" },
      }));
    }
  };
  const patchElectronBridge = () => {
    const electronBridge = window.electronBridge;
    if (!electronBridge || typeof electronBridge.sendMessageFromView !== "function") return false;
    if (electronBridge.sendMessageFromView.__codeyPatched) {
      window.clearTimeout(bridgeRetryTimer);
      markPluginBridgeEffective();
      return true;
    }
    const original = electronBridge.sendMessageFromView;
    const wrapped = function (...args) {
      let request = null;
      try {
        request = pluginRequestArgs(args);
      } catch {}
      // Every IPC message passes through here; unrelated requests must not
      // pay for an extra promise hop or a second argument walk.
      if (!request) return original.apply(this, args);
      const normalizedArgs = args.map(normalizeRequestArg);
      const result = original.apply(this, normalizedArgs);
      if (!result || typeof result.then !== "function") return result;
      const localRefresh = request.list ? waitForLocalPlugins() : Promise.resolve();
      return Promise.all([result, localRefresh]).then(([response]) => {
        let patched = response;
        try {
          patched = patchResponse(response);
        } catch {}
        if (request.mutation) {
          refreshLocalPlugins(true);
        }
        return patched;
      });
    };
    wrapped.__codeyPatched = true;
    electronBridge.sendMessageFromView = wrapped;
    window.clearTimeout(bridgeRetryTimer);
    markPluginBridgeEffective();
    return true;
  };
  const retryPatchElectronBridge = () => {
    bridgeRetryTimer = 0;
    if (patchElectronBridge()) return;
    const fastRetry = Date.now() < bridgeRetryDeadline;
    if (!fastRetry) {
      if (bridgeSlowRetries >= MAX_SLOW_BRIDGE_RETRIES) return;
      bridgeSlowRetries += 1;
    }
    const delay = fastRetry ? bridgeRetryDelay : 30_000;
    if (fastRetry) bridgeRetryDelay = Math.min(bridgeRetryDelay * 2, 2_000);
    bridgeRetryTimer = window.setTimeout(retryPatchElectronBridge, delay);
  };
  window.__codeyEnsurePluginBridge = () => {
    bridgeRetryDeadline = Date.now() + 30_000;
    bridgeRetryDelay = 50;
    bridgeSlowRetries = 0;
    if (patchElectronBridge()) return;
    window.clearTimeout(bridgeRetryTimer);
    bridgeRetryTimer = window.setTimeout(retryPatchElectronBridge, bridgeRetryDelay);
  };

  const registerFetchInterceptor = window.__codeySharedRuntime?.registerFetchInterceptor;
  if (typeof registerFetchInterceptor === "function") {
    registerFetchInterceptor("plugin-marketplace", (next, ...args) => {
      const url = typeof args[0] === "string" ? args[0] : args[0]?.url || "";
      const body = args[1]?.body;
      const routeRequest = pluginCommand(url, true);
      const request = routeRequest || pluginRequest(body);
      if (!request) return next(...args);
      const patchesPluginResponse = Boolean(routeRequest);

      const responsePromise = next(...args);
      const ready = request.list
        ? Promise.all([responsePromise, waitForLocalPlugins()]).then(([response]) => response)
        : responsePromise;
      return ready.then(async (response) => {
        const contentType = response.headers.get("content-type") || "";
        if (!patchesPluginResponse || !contentType.includes("application/json")) return response;
        try {
          const patched = patchResponse(await response.clone().json());
          const headers = new Headers(response.headers);
          headers.delete("content-length");
          return new Response(JSON.stringify(patched), { status: response.status, statusText: response.statusText, headers });
        } catch {
          return response;
        }
      });
    }, 30);
  }
  const bridgePatched = patchElectronBridge();
  if (!bridgePatched) {
    bridgeRetryTimer = window.setTimeout(retryPatchElectronBridge, bridgeRetryDelay);
  }
})();
