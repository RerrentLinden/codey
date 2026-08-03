import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

test("FastCtx optimization is opt-in and exposed through the settings switch", async () => {
  const [appSource, sectionsSource, configSource, commandSource] = await Promise.all([
    readFile(new URL("src/App.tsx", root), "utf8"),
    readFile(new URL("src/FeaturePolicyCard.tsx", root), "utf8"),
    readFile(new URL("backend/src/config.rs", root), "utf8"),
    readFile(new URL("backend/src/commands.rs", root), "utf8"),
  ]);
  const uiSource = `${appSource}\n${sectionsSource}`;

  assert.match(configSource, /pub fast_context_tools: bool/);
  assert.match(configSource, /fast_context_tools: false/);
  assert.match(commandSource, /config\.fast_context_tools = config_input\.fast_context_tools/);
  assert.match(uiSource, /checked=\{config\.fastContextTools\}/);
  assert.match(uiSource, /aria-label="启用 FastCtx 上下文工具"/);
  assert.match(uiSource, /优先复用已配置的 FastCtx；未配置时加载 Codey 内置工具/);
  assert.doesNotMatch(uiSource, /下次启动提供分页读取、搜索、文件发现与批量替换/);
});

test("Codey keeps FastCtx in the dedicated sidecar", async () => {
  const [manifest, sidecarSource, mainSource, libSource, configPatchSource] = await Promise.all([
    readFile(new URL("backend/Cargo.toml", root), "utf8"),
    readFile(new URL("backend/src/bin/codey-fastctx.rs", root), "utf8"),
    readFile(new URL("backend/src/main.rs", root), "utf8"),
    readFile(new URL("backend/src/lib.rs", root), "utf8"),
    readFile(new URL("backend/src/codex_config.rs", root), "utf8"),
  ]);

  assert.match(manifest, /fastctx = \{ git = "https:\/\/github\.com\/yc-duan\/fastctx", rev = "8056641e3969bc10ffc4871b5fa01ca8a91544e4", default-features = false \}/);
  assert.match(manifest, /name = "codey-fastctx"/);
  assert.match(sidecarSource, /fastctx::cli::run_server/);
  // 主程序既不链接 FastCtx，也不再充当启动 sidecar 的兼容代理。
  assert.doesNotMatch(mainSource, /fastctx::/);
  assert.doesNotMatch(libSource, /fastctx::/);
  assert.doesNotMatch(mainSource, /--codey-fastctx-mcp/);
  assert.doesNotMatch(mainSource, /codey-fastctx(?:\.exe)?/);
  assert.match(configPatchSource, /--codey-fastctx-mcp/);
  assert.match(configPatchSource, /CODEY_FASTCTX_SERVER_ID: &str = "codey_fastctx"/);
  assert.match(configPatchSource, /CODEY_FASTCTX_NAMESPACE: &str = "mcp__codey_fastctx"/);
  assert.match(configPatchSource, /FASTCTX_TOKEN_BUDGET/);
});
