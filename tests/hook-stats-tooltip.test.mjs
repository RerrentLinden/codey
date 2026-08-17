import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const source = readFileSync(new URL("../public/codey-inject.js", import.meta.url), "utf8");

test("oversized conversation detail tooltips cannot cover their trigger", () => {
  assert.match(
    source,
    /const conversationRichTooltipSelector = conversationTurnSelector[\s\S]*span\[tabindex="0"\]\[aria-describedby\]/,
  );

  const rule = source.match(/\$\{conversationRichTooltipSelector\} \{([^}]*)\}/)?.[1] || "";
  assert.match(rule, /overflow-x: hidden !important/);
  assert.match(rule, /overflow-y: auto !important/);
  assert.match(rule, /overscroll-behavior: contain/);
  assert.doesNotMatch(rule, /pointer-events/);
});
