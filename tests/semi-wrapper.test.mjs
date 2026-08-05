import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("local badge does not pull the Semi Tag and Avatar dependency chain", async () => {
  const [wrapper, styles] = await Promise.all([
    readFile(new URL("src/components/semi/index.tsx", root), "utf8"),
    readFile(new URL("src/styles.css", root), "utf8"),
  ]);

  assert.doesNotMatch(wrapper, /@douyinfe\/semi-ui\/lib\/es\/tag/);
  assert.match(wrapper, /<span/);
  for (const appearance of [
    "neutral",
    "destructive",
    "outline",
    "success",
    "warning",
    "info",
    "brand",
  ]) {
    assert.match(styles, new RegExp(`\\.codey-tag-${appearance}\\b`));
  }
});
