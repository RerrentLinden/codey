import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(
  new URL("../src/ModelSection.tsx", import.meta.url),
  "utf8",
);

test("official routes display WS while third-party routes remain explicit opt-in", () => {
  assert.match(source, /\(isOfficial \|\| profile\.supportsWebsockets\) &&/);
  assert.match(source, /routeDraft\.upstreamProtocol === "openaiResponses" && \(/);
  assert.match(source, /aria-label="WebSocket"/);
  assert.match(
    source,
    /checked=\{Boolean\(routeDraft\.supportsWebsockets\)\}[\s\S]*?disabled=\{isBusy\}/,
  );
  assert.doesNotMatch(
    source,
    /仅在线路明确支持 Responses WebSocket 时开启；连接失败会自动回退 HTTP\/SSE。/,
  );
});
