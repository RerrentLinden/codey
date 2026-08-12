import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const cardSource = readFileSync(
  new URL("../src/PromptOptimizationCard.tsx", import.meta.url),
  "utf8",
);
const backendSource = readFileSync(
  new URL("../backend/src/prompt_optimization.rs", import.meta.url),
  "utf8",
);

test("prompt optimization exposes the persisted upstream API format", () => {
  assert.match(cardSource, /<span>上游格式<\/span>/);
  assert.match(cardSource, /import \{ Button, Card, Input, Select, Switch \}/);
  assert.match(cardSource, /PROMPT_OPTIMIZATION_PROTOCOL_OPTIONS/);
  assert.match(cardSource, /value=\{optimization\.protocol\}/);
  assert.match(cardSource, /label: "Responses API", value: "responses"/);
  assert.match(
    cardSource,
    /label: "Chat Completions", value: "chatCompletions"/,
  );
  assert.match(cardSource, /optionList=\{PROMPT_OPTIMIZATION_PROTOCOL_OPTIONS\}/);
  assert.match(cardSource, /updateOptimization\(\{\s*protocol:/);
});

test("prompt optimization refreshes the creatable model picker after fetching", () => {
  assert.match(
    cardSource,
    /const modelSelectKey = JSON\.stringify\(cloudModels\)/,
  );
  assert.match(
    cardSource,
    /<Select\s+key=\{modelSelectKey\}[\s\S]*?optionList=\{modelSelectOptions\}[\s\S]*?allowCreate/,
  );
  assert.match(
    cardSource,
    /renderCreateItem=\{\(inputValue, focused, style\) =>/,
  );
  assert.match(
    cardSource,
    /prompt-optimization-model-create-option\$\{focused \? " focused" : ""\}/,
  );
});

test("prompt optimization reuses the runtime protocol converters", () => {
  assert.match(backendSource, /responses_to_chat_completions/);
  assert.match(backendSource, /chat_completion_to_response_with_request/);
  assert.match(
    backendSource,
    /let original_request = responses_payload\([\s\S]*upstream_request_payload/,
  );
  assert.match(backendSource, /extract_responses_optimized_text\(&response\)/);
});
