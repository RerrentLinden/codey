import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("project moves refresh the native conversation list without periodic Chats sorting", async () => {
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
  assert.doesNotMatch(source, /chatsSortRefreshIntervalMs/);
  assert.doesNotMatch(source, /chatsSortDbRefreshIntervalMs/);
  assert.doesNotMatch(source, /scheduleChatsSortCorrection/);
  assert.doesNotMatch(source, /applyChatsSortCorrection/);
  assert.doesNotMatch(source, /__codexProjectMoveSortChats/);
});
