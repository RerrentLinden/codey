import assert from "node:assert/strict";
import test from "node:test";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";

const { createAccountUsageReader } = await loadTypeScriptModule(new URL("../src/accountUsageRequests.ts", import.meta.url));
const deferred = () => Promise.withResolvers();

test("concurrent reads share one request but completed values are not cached", async () => {
  let calls = 0;
  const result = deferred();
  const read = createAccountUsageReader(() => { calls += 1; return result.promise; });
  const first = read("account-a", false);
  assert.equal(read("account-a", false), first);
  await Promise.resolve();
  assert.equal(calls, 1);
  result.resolve({ status: "ok" });
  await first;
  assert.notEqual(read("account-a", false), first);
  await Promise.resolve();
  assert.equal(calls, 2);
});

test("forced refresh waits for the ordinary read and preserves its stronger freshness", async () => {
  const ordinary = deferred(), forced = deferred(), calls = [];
  const read = createAccountUsageReader((accountId, forceRefresh) => {
    calls.push({ accountId, forceRefresh });
    return forceRefresh ? forced.promise : ordinary.promise;
  });
  const first = read("account-a", false);
  const refresh = read("account-a", true);
  assert.notEqual(refresh, first);
  assert.equal(read("account-a", true), refresh);
  assert.equal(read("account-a", false), refresh);
  await Promise.resolve();
  assert.deepEqual(calls, [{ accountId: "account-a", forceRefresh: false }]);
  ordinary.resolve({ status: "ok", marker: "old" });
  assert.equal((await first).marker, "old");
  await Promise.resolve();
  assert.equal(calls.length, 2);
  assert.equal(calls[1].forceRefresh, true);
  // Cleanup of the old read must not remove the queued refresh.
  assert.equal(read("account-a", true), refresh);
  forced.resolve({ status: "ok", marker: "fresh" });
  assert.equal((await refresh).marker, "fresh");
});

test("a failed ordinary request still permits the queued forced refresh", async () => {
  const first = deferred();
  const read = createAccountUsageReader((_id, force) => force ? Promise.resolve({ status: "ok" }) : first.promise);
  const ordinary = read("a", false);
  const refresh = read("a", true);
  first.reject(new Error("offline"));
  await assert.rejects(ordinary, /offline/);
  assert.deepEqual(await refresh, { status: "ok" });
});

test("different accounts and the Codex login never share results", async () => {
  const calls = [];
  const read = createAccountUsageReader(async (id) => { calls.push(id); return { status: "ok", id }; });
  const values = await Promise.all([read("a", false), read("b", false), read(undefined, false)]);
  assert.deepEqual(calls, ["a", "b", undefined]);
  assert.deepEqual(values.map(value => value.id), calls);
});

test("synchronous failures reject and clear the request so a retry can succeed", async () => {
  let attempts = 0;
  const read = createAccountUsageReader(() => {
    if (++attempts === 1) throw new Error("bridge missing");
    return Promise.resolve({ status: "ok" });
  });
  await assert.rejects(read("a", true), /bridge missing/);
  assert.deepEqual(await read("a", true), { status: "ok" });
});
