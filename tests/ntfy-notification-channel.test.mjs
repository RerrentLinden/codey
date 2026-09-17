import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (relativePath) =>
  readFileSync(new URL(`../${relativePath}`, import.meta.url), "utf8");

const typesSource = read("src/notifications/types.ts");
const registrySource = read("src/notifications/channelRegistry.tsx");
const editorSource = read("src/notifications/NtfyChannelEditor.tsx");
const cardSource = read("src/notifications/NotificationChannelsCard.tsx");
const styleSource = read("src/styles.features.css");
const mockSource = read("src/dev/mockApi.ts");

test("ntfy is a server plus topic notification channel", () => {
  assert.match(typesSource, /"ntfy"/);
  assert.match(registrySource, /ntfy:\s*\{[\s\S]*?Editor: NtfyChannelEditor/);
  assert.match(registrySource, /displayName: "ntfy"/);
  assert.match(registrySource, /iconClassName: "ntfy"/);
  assert.match(editorSource, /https:\/\/ntfy\.sh/);
  assert.match(editorSource, /服务器地址/);
  assert.match(editorSource, /主题/);
  assert.match(editorSource, /访问令牌/);
  assert.match(editorSource, /公开主题可留空/);
  assert.match(editorSource, /clearBotToken: false/);
  assert.doesNotMatch(editorSource, /clearBotToken: true|clearUrl: true/);
  assert.match(styleSource, /\.notification-title span\.ntfy \{/);
  assert.doesNotMatch(cardSource, /ntfy/);
});

test("ntfy participates in the preview fixtures and runtime status count", () => {
  assert.match(
    mockSource,
    /kind: "ntfy" as const[\s\S]*?chatId: "preview-ntfy-topic"/,
  );
  assert.match(
    mockSource,
    /channel\.kind === "ntfy"[\s\S]*?channel\.urlConfigured && Boolean\(channel\.chatId\.trim\(\)\)/,
  );
  assert.match(
    mockSource,
    /channel\?\.kind === "ntfy"[\s\S]*?channel\.url\?\.trim\(\) && channel\.chatId\?\.trim\(\)/,
  );
});
