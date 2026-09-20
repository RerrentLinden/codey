import assert from "node:assert/strict";
import test from "node:test";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
const { readInventory, invalidateInventory, withTimeout } =
  await loadTypeScriptModule(
    new URL("../src/features/codex-extensions/requests.ts", import.meta.url),
  );
const scope = { kind: "user" };
test("concurrent inventory reads and short-lived cache share one request", async () => {
  let calls = 0;
  const request = async () => {
    calls++;
    await new Promise((resolve) => setTimeout(resolve, 5));
    return { revision: String(calls) };
  };
  const [first, second] = await Promise.all([
    readInventory(request, scope),
    readInventory(request, scope),
  ]);
  assert.equal(first, second);
  assert.equal(calls, 1);
  assert.equal(await readInventory(request, scope), first);
  assert.equal(calls, 1);
  await readInventory(request, scope, true);
  assert.equal(calls, 2);
  invalidateInventory(request, scope);
  await readInventory(request, scope);
  assert.equal(calls, 3);
});
test("scope caches are isolated and failed reads can retry", async () => {
  let calls = 0;
  const request = async () => {
    if (++calls === 1) throw new Error("offline");
    return { revision: String(calls) };
  };
  await assert.rejects(readInventory(request, scope), /offline/);
  await readInventory(request, scope);
  await readInventory(request, {
    kind: "project",
    projectPath: "/tmp/project",
  });
  assert.equal(calls, 3);
});
test("shared user configuration changes invalidate every scope", async () => {
  let calls = 0;
  const request = async () => ({ revision: String(++calls) });
  const project = { kind: "project", projectPath: "/tmp/shared-config" };
  await readInventory(request, scope);
  await readInventory(request, project);
  invalidateInventory(request);
  await readInventory(request, scope);
  await readInventory(request, project);
  assert.equal(calls, 4);
});
test("invalidation prevents a pending old read from repopulating cache", async () => {
  let release,
    calls = 0;
  const request = () =>
    ++calls === 1
      ? new Promise((resolve) => {
          release = resolve;
        })
      : Promise.resolve({ revision: "new" });
  const pending = readInventory(request, scope);
  invalidateInventory(request, scope);
  const latest = await readInventory(request, scope);
  release({ revision: "old" });
  await pending;
  assert.equal(await readInventory(request, scope), latest);
});
test("timeouts describe uncertain mutations without claiming cancellation", async () => {
  let completed = false;
  const operation = new Promise((resolve) =>
    setTimeout(() => {
      completed = true;
      resolve("saved");
    }, 15),
  );
  await assert.rejects(withTimeout(operation, true, 1), /结果尚不确定/);
  assert.equal(await operation, "saved");
  assert.equal(completed, true);
  await assert.rejects(
    withTimeout(new Promise(() => {}), false, 1),
    /读取超时/,
  );
});
