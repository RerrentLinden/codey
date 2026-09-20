import assert from "node:assert/strict";
import test from "node:test";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
const { createSkillCacheRequests, getSkillCacheRequests } = await loadTypeScriptModule(new URL("../src/features/codex-extensions/requests.ts", import.meta.url));
const { createCodexExtensionsPreview } = await loadTypeScriptModule(new URL("../src/dev/codexExtensionsMock.ts", import.meta.url));
const { selectResources, batchTargets } = await loadTypeScriptModule(new URL("../src/features/codex-extensions/state.ts", import.meta.url));

test("cache is global, read-only, and independent from ordinary inventory", async () => {
  globalThis.window = { location: { search: "" } };
  const request = createCodexExtensionsPreview("macos");
  const inventory = await request({ action: "list" });
  const cache = await request({ action: "list_skill_cache", scope: { kind: "project", projectPath: "/demo" } });
  assert.equal(inventory.skills.some((item) => item.ownership === "plugin"), false);
  assert.equal(cache.skills.length, 1);
  const id = cache.skills[0].id;
  const document = await request({ action: "read_skill_cache", id });
  assert.equal(document.readOnly, true);
  assert.match(document.content, /name: browser/);
  await assert.rejects(request({ action: "remove_skill_cache", id, revision: cache.revision }), /确认/);
  await assert.rejects(request({ action: "remove_skill_cache", id, revision: "stale", confirmed: true }), /变化/);
  const result = await request({ action: "remove_skill_cache", id, revision: cache.revision, confirmed: true });
  assert.equal(result.cache.skills.length, 0);
  assert.equal((await request({ action: "list_skill_cache" })).skills.length, 0);
  assert.deepEqual(await request({ action: "list" }), inventory);
});

test("ordinary filtering and batch actions reject plugin cache even with writable flags", () => {
  const plugin = { id: "cache", name: "cache", sourcePath: "/cache", ownership: "plugin", scope: "plugin", enabled: false, readOnly: false, canToggle: true };
  assert.deepEqual(selectResources([plugin], "", "all", "all", "name"), []);
  assert.deepEqual(batchTargets([plugin], [plugin.id], true), []);
});

test("conflicts require a successful refresh before another delete", async () => {
  globalThis.window = { location: { search: "?extensions=cache-conflict" } };
  const request = createCodexExtensionsPreview("macos");
  const session = getSkillCacheRequests(request);
  assert.equal(getSkillCacheRequests(request), session);
  const cache = await session.list();
  await assert.rejects(session.remove(cache.skills[0].id, cache.revision), /刷新/);
  assert.equal(session.blocked, true);
  await assert.rejects(session.remove(cache.skills[0].id, cache.revision), /刷新/);
  const refreshed = await session.list();
  assert.equal(session.blocked, false);
  assert.equal((await session.remove(refreshed.skills[0].id, refreshed.revision)).cache.skills.length, 0);
});

test("timed out mutation cannot be repeated and refresh waits for its actual completion", async () => {
  let finish;
  let reads = 0;
  let removed = false;
  const request = async ({ action }) => {
    if (action === "list_skill_cache") { reads++; return { revision: removed ? "after" : "before", skills: [], warnings: [] }; }
    await new Promise((resolve) => { finish = resolve; });
    removed = true;
    return { cache: { revision: "after", skills: [], warnings: [] }, message: "done" };
  };
  const session = createSkillCacheRequests(request, 15);
  await session.list();
  await assert.rejects(session.remove("cache", "before"), /超时/);
  await assert.rejects(session.remove("cache", "before"), /刷新/);
  const refresh = session.list();
  assert.equal(reads, 1);
  assert.equal(session.blocked, true);
  finish();
  assert.equal((await refresh).revision, "after");
  assert.equal(session.blocked, false);
});

test("an older refresh cannot unlock deletion after a newer operation failed", async () => {
  const reads = [];
  const request = async ({ action }) => {
    if (action === "list_skill_cache")
      return new Promise((resolve) => reads.push(resolve));
    throw new Error("缓存已变化，请刷新后重新操作");
  };
  const session = createSkillCacheRequests(request);
  const older = session.list();
  const latest = session.list();
  reads[1]({ revision: "latest", skills: [], warnings: [] });
  await latest;
  await assert.rejects(session.remove("cache", "latest"), /变化/);
  reads[0]({ revision: "old", skills: [], warnings: [] });
  await older;
  assert.equal(session.blocked, true);
  await assert.rejects(session.remove("cache", "old"), /刷新/);
});

test("a failed refresh keeps deletion blocked until the current state is read", async () => {
  let fail = false;
  const request = async ({ action }) => {
    assert.equal(action, "list_skill_cache");
    if (fail) throw new Error("无法读取缓存");
    return { revision: "current", skills: [], warnings: [] };
  };
  const session = createSkillCacheRequests(request);
  await session.list();
  fail = true;
  await assert.rejects(session.list(), /无法读取/);
  assert.equal(session.blocked, true);
  await assert.rejects(session.remove("cache", "current"), /刷新/);
  fail = false;
  await session.list();
  assert.equal(session.blocked, false);
});
