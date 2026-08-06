import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("project moves refresh natively and project rows prioritize running tasks", async () => {
  const source = await readFile(
    new URL("vendor/CodeyRuntime/assets/inject/renderer-inject.js", root),
    "utf8",
  );

  assert.match(
    source,
    /function refreshAfterProjectMove\(\) \{[\s\S]*applyProjectMoveProjection\(\);[\s\S]*void refreshRecentConversationsForHost\(\)\.finally/,
  );
  assert.match(
    source,
    /refresh-recent-conversations-for-host", \{ hostId: "local", sortKey: "updated_at" \}/,
  );
  assert.match(
    source,
    /function finishProjectMove[\s\S]*refreshAfterProjectMove\(\);/,
  );
  assert.match(
    source,
    /function rowHasRunningStatus[\s\S]*querySelector\?\.\("\.animate-spin"\)[\s\S]*loading\|processing\|running\|working/,
  );
  assert.match(
    source,
    /function prioritizeRunningRowsInList[\s\S]*const ordered = \[\.\.\.running, \.\.\.idle\][\s\S]*ordered\.forEach/,
  );
  assert.match(
    source,
    /function scanDeferred[\s\S]*prioritizeRunningProjectRows\(\);/,
  );
  assert.match(source, /function insertProjectedRowItem/);
  assert.doesNotMatch(source, /function insertRowItemByTime/);
  assert.doesNotMatch(
    source,
    /sortMs > childSortMs \|\| \(sortMs === childSortMs/,
  );
  assert.doesNotMatch(source, /chatsSortRefreshIntervalMs/);
  assert.doesNotMatch(source, /chatsSortDbRefreshIntervalMs/);
  assert.doesNotMatch(source, /scheduleChatsSortCorrection/);
  assert.doesNotMatch(source, /applyChatsSortCorrection/);
  assert.doesNotMatch(source, /__codexProjectMoveSortChats/);
});
