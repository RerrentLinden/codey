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
const commandSource = readFileSync(
  new URL("../backend/src/commands/prompt_optimization.rs", import.meta.url),
  "utf8",
);

test("prompt optimization switches between Codey routing and manual upstream configuration", () => {
  assert.match(cardSource, /使用 Codey 路由/);
  assert.match(cardSource, /手动配置/);
  assert.match(cardSource, /OpenAI Responses/);
  assert.match(cardSource, /OpenAI Chat Completions/);
  assert.match(cardSource, /Anthropic Messages/);
  assert.match(cardSource, /<ModelCombobox/);
  assert.doesNotMatch(cardSource, /同步当前线路配置/);
  assert.doesNotMatch(commandSource, /sync_prompt_optimization_current_provider/);
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

test("prompt optimization supports all manual upstream request formats", () => {
  assert.match(backendSource, /fn responses_payload\(/);
  assert.match(backendSource, /fn openai_chat_payload\(/);
  assert.match(backendSource, /fn anthropic_payload\(/);
  assert.match(backendSource, /extract_anthropic_optimized_text/);
  assert.match(backendSource, /extract_responses_optimized_text\(response\)/);
  assert.match(commandSource, /optimization\.uses_codey_route\(\)/);
  assert.match(commandSource, /ROUTER_AUTH_HEADER/);
});
