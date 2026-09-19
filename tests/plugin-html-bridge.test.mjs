import assert from 'node:assert/strict';
import test from 'node:test';
import vm from 'node:vm';
import { once } from 'node:events';
import { MessageChannel } from 'node:worker_threads';
import { loadTypeScriptModule } from './helpers/load-typescript-module.mjs';
const { pluginJsonObject, validPluginReady, pluginHtmlDocument } = await loadTypeScriptModule(new URL('../src/pluginHtmlBridge.ts', import.meta.url));
test('bridge only accepts bounded JSON objects', () => {
  assert.deepEqual(pluginJsonObject({ value: 'demo', list: [null, 1, true] }), { value: 'demo', list: [null, 1, true] });
  const cyclic = {}; cyclic.self = cyclic;
  for (const value of [null, [], new Date(), { a: undefined }, { a: NaN }, { a: 1n }, cyclic, { a: 'a'.repeat(1024 * 1024) }]) assert.equal(pluginJsonObject(value), null);
});
test('ready binds one matching frame session and transferred port', () => {
  const frame = {}, event = { source: frame, data: { type: 'codey-plugin-ready', token: 'token', attemptId: 'attempt' }, ports: [{}] };
  assert.equal(validPluginReady(event, frame, 'token', false, 'attempt'), true);
  assert.equal(validPluginReady(event, {}, 'token', false, 'attempt'), false);
  assert.equal(validPluginReady(event, frame, 'stale', false, 'attempt'), false);
  assert.equal(validPluginReady(event, frame, 'token', true, 'attempt'), false);
  assert.equal(validPluginReady({ ...event, ports: [] }, frame, 'token', false, 'attempt'), false);
  assert.equal(validPluginReady(event, frame, 'token', false, 'next-attempt'), false);
});

function frameBootstrap(t) {
  const listeners = new Map(), ready = [], channels = [];
  const parent = { postMessage(data, origin, ports) {
    assert.equal(origin, '*');
    // 使用真正的可转移 MessagePort，检查重新连接时端口确实可收发。
    ready.push(structuredClone({ data, ports }, { transfer: ports }));
  } };
  const context = vm.createContext({ parent, window: { addEventListener(type, fn) { listeners.set(type, fn); } }, MessageChannel: class extends MessageChannel {
    constructor() { super(); channels.push(this); }
  } });
  vm.runInContext(pluginHtmlDocument('', 'session').match(/<script>([\s\S]*?)<\/script>/)[1], context);
  t.after(() => { channels.forEach(c => { c.port1.close(); c.port2.close(); }); ready.forEach(r => r.ports[0].close()); });
  const connect = (attemptId, source = parent, token = 'session') => listeners.get('message')({ source, data: { type: 'codey-plugin-connect', token, attemptId } });
  const run = source => vm.runInContext(source, context);
  return { connect, ready, run, listeners };
}

test('loaded child waits for parent and transfers a working private port', async t => {
  const frame = frameBootstrap(t);
  assert.equal(frame.ready.length, 0);
  frame.run("window.CodeyPluginConfig.onInit(({config}) => window.CodeyPluginConfig.setConfig({...config, edited: true}))");
  frame.connect('late-parent');
  const { ports: [port], data } = frame.ready[0];
  assert.equal(data.attemptId, 'late-parent');
  const reply = once(port, 'message', { signal: AbortSignal.timeout(1000) });
  port.postMessage({ type: 'init', config: { value: 'demo' }, theme: 'light' });
  assert.deepEqual((await reply)[0], { type: 'config', config: { value: 'demo', edited: true } });
});

test('effect replay establishes a new channel and rejects stale ready messages', async t => {
  const frame = frameBootstrap(t);
  frame.connect('first-effect');
  frame.ready[0].ports[0].close();
  frame.connect('second-effect');
  frame.connect('second-effect');
  assert.equal(frame.ready.length, 2);
  const source = {};
  assert.equal(validPluginReady({ ...frame.ready[0], source }, source, 'session', false, 'second-effect'), false);
  assert.equal(validPluginReady({ ...frame.ready[1], source }, source, 'session', false, 'second-effect'), true);
  frame.run("window.CodeyPluginConfig.onInit(({config}) => window.CodeyPluginConfig.setConfig(config))");
  const port = frame.ready[1].ports[0];
  const reply = once(port, 'message', { signal: AbortSignal.timeout(1000) });
  port.postMessage({ type: 'init', config: { active: 'second' }, theme: 'dark' });
  assert.deepEqual((await reply)[0].config, { active: 'second' });
});

test('child ignores foreign windows and stale session tokens', t => {
  const frame = frameBootstrap(t);
  frame.connect('attempt', {});
  frame.connect('attempt', undefined, 'wrong-token');
  assert.equal(frame.ready.length, 0);
});
test('host CSP and bridge precede plugin content without embedding config', () => {
  const html = pluginHtmlDocument('<script>pluginCode()</script>', 'session');
  assert.ok(html.indexOf('Content-Security-Policy') < html.indexOf('CodeyPluginConfig'));
  assert.ok(html.indexOf('CodeyPluginConfig') < html.indexOf('pluginCode'));
  assert.ok(html.includes("connect-src 'none'"));
  assert.ok(html.includes('MessageChannel'));
  assert.ok(!html.includes('__codeyInvokeApi'));
});
