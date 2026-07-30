import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import ts from "typescript";

const apiSource = fs.readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");
const apiJavaScript = ts.transpileModule(apiSource, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2020,
  },
}).outputText;
const api = await import(
  `data:text/javascript;base64,${Buffer.from(apiJavaScript).toString("base64")}`
);

test("overlay API paths reject commands outside the backend whitelist", () => {
  assert.equal(api.codeyApiPath("save_codey_config"), "/api/save_codey_config");
  assert.equal(api.isCodeyApiCommand("runtime_status"), true);
  assert.equal(api.isCodeyApiCommand("../session/delete"), false);
  assert.throws(
    () => api.codeyApiPath("../session/delete"),
    /不允许的 Codey API 命令/,
  );
});

test("frontend and backend API command whitelists stay in sync", () => {
  const backend = fs.readFileSync(
    new URL("../backend/src/commands.rs", import.meta.url),
    "utf8",
  );
  const invokeApi = backend.slice(
    backend.indexOf("pub async fn invoke_api"),
    backend.indexOf("pub async fn load_codey_config"),
  );
  const backendCommands = [
    ...invokeApi.matchAll(/^\s*"([a-z][a-z0-9_]*)"\s*=>/gm),
  ].map((match) => match[1]);

  assert.deepEqual(
    [...api.CODEY_API_COMMANDS].sort(),
    backendCommands.sort(),
  );
});

test("overlay resolves bridge paths through the checked helper", () => {
  const overlay = fs.readFileSync(
    new URL("../src/overlay.tsx", import.meta.url),
    "utf8",
  );
  assert.match(overlay, /__codexSessionDeleteBridge\(codeyApiPath\(command\), args\)/);
  assert.doesNotMatch(overlay, /__codexSessionDeleteBridge\(`\/api\/\$\{command\}`/);
});
