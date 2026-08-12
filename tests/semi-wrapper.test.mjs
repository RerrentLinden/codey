import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { readAppStyles } from "./helpers/read-app-styles.mjs";

const root = new URL("../", import.meta.url);

test("local badge does not pull the Semi Tag and Avatar dependency chain", async () => {
  const [wrapper, styles] = await Promise.all([
    readFile(new URL("src/components/semi/index.tsx", root), "utf8"),
    readAppStyles(root),
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

test("select dropdowns close when their settings scroller moves", async () => {
  const wrapper = await readFile(
    new URL("src/components/semi/index.tsx", root),
    "utf8",
  );

  assert.match(wrapper, /selectRef\.current\?\.triggerRef\.current/);
  assert.match(wrapper, /closest<HTMLElement>\("\.page-scroll"\)/);
  assert.match(
    wrapper,
    /addEventListener\("scroll", closeDropdown, \{\s*passive: true/,
  );
  assert.match(wrapper, /const closeDropdown = \(\) => selectRef\.current\?\.close\(\)/);
  assert.match(
    wrapper,
    /removeEventListener\("scroll", closeDropdown\)/,
  );
});
