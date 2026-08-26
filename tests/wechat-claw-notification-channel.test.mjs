import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const typesSource = readFileSync(
  new URL("../src/notifications/types.ts", import.meta.url),
  "utf8",
);
const registrySource = readFileSync(
  new URL("../src/notifications/channelRegistry.tsx", import.meta.url),
  "utf8",
);
const editorSource = readFileSync(
  new URL("../src/notifications/WechatClawChannelEditor.tsx", import.meta.url),
  "utf8",
);
const mainSource = readFileSync(
  new URL("../src/main.tsx", import.meta.url),
  "utf8",
);

test("WeChat ClawBot is a scan-bound, token-protected notification channel", () => {
  assert.match(typesSource, /"wechatClaw"/);
  assert.match(
    registrySource,
    /wechatClaw:\s*\{[\s\S]*?Editor: WechatClawChannelEditor/,
  );
  assert.match(registrySource, /displayName: "微信 ClawBot"/);
  assert.match(editorSource, /invoke<WechatClawLoginStartResult>\(\s*"start_wechat_claw_login"/);
  assert.match(editorSource, /"poll_wechat_claw_login"/);
  assert.match(editorSource, /window\.setTimeout\(\(\) => void poll\(\), 1_200\)/);
  assert.match(editorSource, /qrCodeImageUrl/);
  assert.match(editorSource, /clearBotToken: true/);
  assert.doesNotMatch(editorSource, /type="password"/);
  assert.match(
    mainSource,
    /channel\.kind === "telegram" \|\| channel\.kind === "wechatClaw"/,
  );
});
