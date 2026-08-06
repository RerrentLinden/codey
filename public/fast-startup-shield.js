(() => {
  const enabled = ["__CODEY_FAST_CODEX_STARTUP__"][0] === "true";
  const timeoutMs = Math.max(
    250,
    Math.min(Number("__CODEY_STATSIG_TIMEOUT_MS__") || 1500, 5000),
  );
  const existing = window.__codeyFastStartupShield;
  if (existing?.version === 1 && existing.enabled === enabled) return;

  const state = {
    version: 1,
    enabled,
    installed: false,
    active: false,
    timeoutMs,
    protectionWindowMs: 30000,
    statsigFetches: 0,
    statsigTimeouts: 0,
    clientFallbacks: 0,
    clientFailures: 0,
    snapshot() {
      return {
        version: this.version,
        enabled: this.enabled,
        installed: this.installed,
        active: this.active,
        timeoutMs: this.timeoutMs,
        protectionWindowMs: this.protectionWindowMs,
        statsigFetches: this.statsigFetches,
        statsigTimeouts: this.statsigTimeouts,
        clientFallbacks: this.clientFallbacks,
        clientFailures: this.clientFailures,
      };
    },
  };
  window.__codeyFastStartupShield = state;
  if (
    !enabled
    || typeof window.fetch !== "function"
    || typeof window.__codeySharedRuntime?.registerFetchInterceptor !== "function"
  ) return;

  const statsigHosts = new Set([
    "ab.chatgpt.com",
    "featureassets.org",
    "prodregistryv2.org",
    "api.statsigcdn.com",
    "statsigapi.net",
    "cloudflare-dns.com",
  ]);
  const patchedClients = new WeakSet();

  const isStatsigRequest = (input) => {
    try {
      const rawUrl = typeof input === "string" ? input : input?.url ?? "";
      return statsigHosts.has(new URL(rawUrl, window.location.href).hostname);
    } catch {
      return false;
    }
  };

  const statsigClients = window.__codeySharedRuntime.statsigClients;

  const markClientReady = (client) => {
    try {
      if (client.loadingStatus && client.loadingStatus !== "Ready") {
        client.loadingStatus = "Ready";
      }
    } catch {
    }
    try {
      if (typeof client.$emt === "function") {
        client.$emt({ name: "values_updated" });
      }
    } catch {
    }
  };

  const patchClient = (client) => {
    if (patchedClients.has(client)) return;
    let initializeAsync;
    try {
      initializeAsync = client.initializeAsync;
    } catch {
      return;
    }
    if (typeof initializeAsync !== "function") return;
    const originalInitializeAsync = initializeAsync.bind(client);
    const patchedInitializeAsync = (...args) => {
      if (!state.active) return originalInitializeAsync(...args);
      let fallbackTimer;
      const originalResult = Promise.resolve()
        .then(() => originalInitializeAsync(...args))
        .catch(() => {
          state.clientFailures += 1;
          markClientReady(client);
          return null;
        });
      const fallback = new Promise((resolve) => {
        fallbackTimer = window.setTimeout(() => {
          state.clientFallbacks += 1;
          markClientReady(client);
          resolve(null);
        }, timeoutMs);
      });
      return Promise.race([originalResult, fallback]).finally(() => {
        window.clearTimeout(fallbackTimer);
      });
    };
    try {
      client.initializeAsync = patchedInitializeAsync;
      if (client.initializeAsync === patchedInitializeAsync) patchedClients.add(client);
    } catch {
    }
  };

  const patchStatsigClients = () => {
    for (const client of statsigClients()) patchClient(client);
  };

  const releaseStatsigClients = () => {
    for (const client of statsigClients()) {
      patchClient(client);
      markClientReady(client);
    }
  };

  const unregisterFetchInterceptor = window.__codeySharedRuntime.registerFetchInterceptor(
    "fast-startup",
    (next, input, init = undefined) => {
      if (!state.active || !isStatsigRequest(input)) {
        return next(input, init);
      }
      state.statsigFetches += 1;

      const controller = new AbortController();
      const upstreamSignal = init?.signal || (
        input && typeof input === "object" ? input.signal : undefined
      );
      const propagateAbort = () => controller.abort();
      if (upstreamSignal) {
        if (upstreamSignal.aborted) propagateAbort();
        else upstreamSignal.addEventListener("abort", propagateAbort, { once: true });
      }
      const timer = window.setTimeout(() => {
        state.statsigTimeouts += 1;
        releaseStatsigClients();
        controller.abort();
      }, timeoutMs);
      const cleanup = () => {
        window.clearTimeout(timer);
        upstreamSignal?.removeEventListener?.("abort", propagateAbort);
      };
      const nextInit = { ...(init || {}), signal: controller.signal };
      try {
        return Promise.resolve(next(input, nextInit)).finally(cleanup);
      } catch (error) {
        cleanup();
        throw error;
      }
    },
    10,
  );
  state.installed = true;
  state.active = true;

  patchStatsigClients();
  const startedAt = Date.now();
  // Statsig clients are almost always constructed in the first moments after
  // boot, so the tight poll only needs to cover that window; keeping it at
  // 50 ms for the full 15 s competes with the startup it is meant to speed up.
  const fastScanWindowMs = 1000;
  const fastScanIntervalMs = 50;
  const slowScanIntervalMs = 250;
  let clientScanTimer = 0;
  let scanIntervalMs = fastScanIntervalMs;
  const scanForStatsigClients = () => {
    patchStatsigClients();
    const elapsed = Date.now() - startedAt;
    if (elapsed >= 15000) {
      window.clearInterval(clientScanTimer);
      return;
    }
    if (scanIntervalMs === fastScanIntervalMs && elapsed >= fastScanWindowMs) {
      scanIntervalMs = slowScanIntervalMs;
      window.clearInterval(clientScanTimer);
      clientScanTimer = window.setInterval(scanForStatsigClients, slowScanIntervalMs);
    }
  };
  clientScanTimer = window.setInterval(scanForStatsigClients, fastScanIntervalMs);
  window.setTimeout(() => {
    state.active = false;
    unregisterFetchInterceptor();
  }, state.protectionWindowMs);
})();
