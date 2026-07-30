import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const source = fs.readFileSync(
  new URL("../src/main.tsx", import.meta.url),
  "utf8",
);

test("development preview fixtures cannot be mistaken for live credentials", () => {
  assert.match(source, /example\.invalid/);
  assert.match(source, /preview-only-not-a-secret/);
  assert.match(source, /preview-chat-id/);
  assert.doesNotMatch(source, /\bsk-(?:proj-)?[A-Za-z0-9._-]+/);
  assert.doesNotMatch(source, /api\.(?:openai|anthropic)\.com/);
  assert.doesNotMatch(
    source,
    /open\.feishu\.cn\/open-apis\/bot\/v2\/hook\/[A-Za-z0-9-]+/,
  );
});
