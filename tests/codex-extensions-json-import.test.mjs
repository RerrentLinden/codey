import assert from "node:assert/strict";
import test from "node:test";
import { loadTypeScriptModule } from "./helpers/load-typescript-module.mjs";
const {
  parseMcpJson,
  selectedMcpJson,
  updateMcpJsonDraft,
  mcpJsonError,
  exportMcpJson,
} = await loadTypeScriptModule(
  new URL("../src/features/codex-extensions/mcpJson.ts", import.meta.url),
);

test("accepts wrapped and standalone JSON while preserving configuration values", () => {
  const config = { command: "npx", args: ["-y", "demo"], env: { KEY: "test" } };
  for (const value of [
    config,
    { mcpServers: { demo: config } },
    { mcp_servers: { demo: config } },
  ])
    assert.deepEqual(parseMcpJson(JSON.stringify(value))[0].config, config);
});
test("rejects invalid, empty, oversized and malformed wrapper inputs", () => {
  for (const value of [
    "{",
    "null",
    "[]",
    "{}",
    '{"mcpServers":[]}',
    '{"mcpServers":{}}',
    '{"mcpServers":{"demo":null}}',
    '{"mcpServers":{},"mcp_servers":{}}',
    '{"command":""}',
    " ".repeat(1024 * 1024 + 1),
  ])
    assert.throws(() => parseMcpJson(value));
});
test("multi-service selection remains independent of final renamed identity", () => {
  const content = JSON.stringify({
    mcpServers: {
      first: { command: "one" },
      second: { url: "https://example.com/mcp" },
    },
  });
  assert.throws(() => selectedMcpJson({ content }));
  assert.equal(
    selectedMcpJson({ content, jsonService: "second", id: "renamed" }).config
      .url,
    "https://example.com/mcp",
  );
});
test("new JSON keeps revision and drafts through invalid edits and refresh", () => {
  const draft = {
    kind: "mcp",
    isNew: true,
    id: "manual",
    revision: "r1",
    content: "",
    original: "",
  };
  const next = updateMcpJsonDraft(
    draft,
    '{"mcpServers":{"imported":{"command":"node"}}}',
  );
  assert.equal(next.id, "imported");
  assert.equal(next.revision, "r1");
  const invalid = updateMcpJsonDraft(next, "{");
  assert.equal(invalid.content, "{");
  assert.equal(invalid.id, "imported");
  assert.match(mcpJsonError(invalid), /JSON/);
  assert.equal(mcpJsonError({ ...next, revision: "r2" }), "");
});
test("JSON edits preserve the selected server and its renamed identity", () => {
  const content = JSON.stringify({
    mcpServers: { first: { command: "one" }, second: { command: "two" } },
  });
  const draft = {
    kind: "mcp",
    isNew: true,
    id: "custom-name",
    jsonService: "second",
    content,
  };
  const incomplete = updateMcpJsonDraft(draft, "{");
  const restored = updateMcpJsonDraft(
    incomplete,
    content.replace("two", "updated"),
  );
  assert.equal(restored.id, "custom-name");
  assert.equal(restored.jsonService, "second");
  assert.equal(selectedMcpJson(restored).config.command, "updated");
  const single = {
    ...draft,
    content: JSON.stringify({ mcpServers: { second: { command: "two" } } }),
  };
  assert.equal(
    updateMcpJsonDraft(single, single.content.replace("two", "updated")).id,
    "custom-name",
  );
  assert.equal(parseMcpJson("\uFEFF" + single.content)[0].name, "second");
});
test("editing imports preserve target identity and validate incomplete JSON", () => {
  const draft = {
    kind: "mcp",
    id: "existing",
    content: '{"command":"node"}',
    revision: "r2",
  };
  const imported = updateMcpJsonDraft(
    draft,
    exportMcpJson("other", { command: "updated" }),
  );
  assert.equal(imported.id, "existing");
  assert.equal(imported.revision, "r2");
  assert.equal(selectedMcpJson(imported).config.command, "updated");
  assert.match(mcpJsonError({ ...imported, content: "{" }), /JSON/);
});
test("JSON exports round trip with service name and all configuration fields", () => {
  const config = {
    url: "https://example.com/mcp",
    http_headers: { Authorization: "REDACTED" },
    enabled: false,
  };
  const [service] = parseMcpJson(exportMcpJson("round-trip", config));
  assert.equal(service.name, "round-trip");
  assert.deepEqual(service.config, config);
});
test("校验结果按内容记忆化，编辑器重渲染不再重复解析整份 JSON", () => {
  const content = '{"mcpServers":{"demo":{"command":"npx"}}}';
  assert.equal(parseMcpJson(content), parseMcpJson(content));
  const invalid = '{"mcpServers":{"demo":null}}';
  assert.throws(() => parseMcpJson(invalid));
  assert.throws(() => parseMcpJson(invalid), "同一份无效内容也要记住结论");
  assert.notEqual(parseMcpJson(content), parseMcpJson(content + "\n"));
});
test("超过 1 MB 的内容按字节数拒绝，非 ASCII 也不会漏判", () => {
  assert.throws(() => parseMcpJson(" ".repeat(1024 * 1024 + 1)), /1 MB/);
  // 40 万汉字只有 40 万个码元，但 UTF-8 超过 1 MB，必须靠精确字节数判定。
  assert.throws(() => parseMcpJson("中".repeat(400_000)), /1 MB/);
  assert.throws(() => parseMcpJson("\uFEFF" + " ".repeat(1024 * 1024 + 1)), /1 MB/);
});
