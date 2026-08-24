import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const cardSource = readFileSync(
  new URL("../src/PromptOptimizationCard.tsx", import.meta.url),
  "utf8",
);
const mantineWrapperSource = readFileSync(
  new URL("../src/components/mantine/index.tsx", import.meta.url),
  "utf8",
);
const backendSource = readFileSync(
  new URL("../backend/src/prompt_optimization.rs", import.meta.url),
  "utf8",
);

test("prompt optimization exposes no upstream protocol selector", () => {
  assert.doesNotMatch(cardSource, /上游格式/);
  assert.doesNotMatch(cardSource, /PROMPT_OPTIMIZATION_PROTOCOL_OPTIONS/);
  assert.doesNotMatch(cardSource, /optimization\.protocol/);
  assert.doesNotMatch(cardSource, /Chat Completions/);
});

test("prompt optimization refreshes the creatable model picker without remounting", () => {
  assert.doesNotMatch(cardSource, /modelSelectKey/);
  assert.match(
    cardSource,
    /<Select[\s\S]*?optionList=\{modelSelectOptions\}[\s\S]*?allowCreate/,
  );
  assert.match(
    mantineWrapperSource,
    /React\.useEffect\(\(\) => \{\s*setSearch\(selectedValue\);\s*\}, \[selectedValue\]\)/,
  );
  assert.match(
    cardSource,
    /renderCreateItem=\{\(inputValue, focused, style\) =>/,
  );
  assert.match(
    cardSource,
    /focused \? "bg-blue-500\/8" : ""/,
  );
  assert.doesNotMatch(cardSource, /prompt-optimization-model-create-option/);
});

test("prompt optimization uses Responses without runtime converters", () => {
  assert.match(backendSource, /fn responses_payload\(/);
  assert.match(backendSource, /extract_responses_optimized_text\(&value\)/);
  assert.doesNotMatch(backendSource, /responses_to_chat_completions/);
  assert.doesNotMatch(backendSource, /chat_completion_to_response_with_request/);
  assert.doesNotMatch(backendSource, /upstream_request_payload/);
});
