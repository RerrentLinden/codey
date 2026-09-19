export const PLUGIN_UI_MAX_BYTES = 1024 * 1024;

/** Structured-cloned messages must still contain JSON, not arbitrary JS values. */
export function pluginJsonObject(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const seen = new Set<object>();
  function valid(item: unknown, depth: number): boolean {
    if (depth > 64) return false;
    if (item === null || typeof item === "string" || typeof item === "boolean") return true;
    if (typeof item === "number") return Number.isFinite(item);
    if (typeof item !== "object" || seen.has(item)) return false;
    if (!Array.isArray(item) && Object.getPrototypeOf(item) !== Object.prototype && Object.getPrototypeOf(item) !== null) return false;
    seen.add(item);
    const ok = Object.values(item).every(child => valid(child, depth + 1));
    seen.delete(item); return ok;
  }
  try {
    if (!valid(value, 0)) return null;
    const json = JSON.stringify(value);
    return new TextEncoder().encode(json).length <= PLUGIN_UI_MAX_BYTES ? JSON.parse(json) : null;
  } catch { return null; }
}

export function validPluginReady(event: { source: unknown; data: unknown; ports: readonly unknown[] }, source: unknown, token: string, bound: boolean, attemptId: string): boolean {
  const data = event.data as { type?: unknown; token?: unknown; attemptId?: unknown } | null;
  return !bound && source != null && event.source === source && event.ports.length === 1
    && data?.type === "codey-plugin-ready" && data.token === token && data.attemptId === attemptId;
}

export function pluginHtmlDocument(html: string, token: string): string {
  const csp = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; frame-src 'none'; object-src 'none'; worker-src 'none'; form-action 'none'; base-uri 'none'";
  // This bootstrap runs before plugin code; configuration only travels over its private port.
  const bootstrap = `(() => {
    let port, attemptId, initial; const callbacks = [];
    const send = message => port?.postMessage(message);
    const clone = value => JSON.parse(JSON.stringify(value));
    Object.defineProperty(window, 'CodeyPluginConfig', { value: Object.freeze({
      onInit(callback) { if (typeof callback !== 'function') return; callbacks.push(callback); if (initial) callback(clone(initial)); },
      setConfig(config) { send({ type: 'config', config }); },
      setValidity(valid, message = '') { send({ type: 'validity', valid, message }); }
    }), writable: false, configurable: false });
    window.addEventListener('message', event => {
      const data = event.data;
      if (event.source !== parent || data?.type !== 'codey-plugin-connect' || data.token !== ${JSON.stringify(token)}
        || typeof data.attemptId !== 'string' || data.attemptId === attemptId) return;
      port?.close(); initial = undefined; attemptId = data.attemptId;
      const channel = new MessageChannel(); port = channel.port1;
      const current = port;
      current.onmessage = event => {
        if (port !== current || event.data?.type !== 'init') return;
        initial = event.data; for (const callback of callbacks) callback(clone(initial));
      };
      parent.postMessage({ type: 'codey-plugin-ready', token: ${JSON.stringify(token)}, attemptId }, '*', [channel.port2]);
    });
    window.addEventListener('pagehide', () => { send({type:'unload'}); port?.close(); }, {once:true});
  })();`;
  return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><meta name="referrer" content="no-referrer"><script>${bootstrap}</script></head><body>${html}</body></html>`;
}
