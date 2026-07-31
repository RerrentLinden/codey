import assert from "node:assert/strict";
import { join } from "node:path";
import test from "node:test";
import {
  cargoProfile,
  codeyExecutablePaths,
  ensureUnlockedCodeyOnWindows,
  forceKillEnabled,
} from "../scripts/dev.mjs";

test("dev script derives the exact Cargo output executable", () => {
  const root = "/workspace/codey";
  const env = {};

  assert.equal(cargoProfile([]), "debug");
  assert.equal(cargoProfile(["--release"]), "release");
  assert.equal(cargoProfile(["--", "--release"]), "debug");
  assert.equal(cargoProfile(["--profile", "bench"]), "bench");
  assert.equal(cargoProfile(["--profile=custom"]), "custom");
  assert.deepEqual(codeyExecutablePaths({ root, args: [], env }), [
    join(root, "target", "debug", "codey.exe").toLowerCase(),
  ]);
  assert.deepEqual(
    codeyExecutablePaths({ root, args: ["--release"], env }),
    [join(root, "target", "release", "codey.exe").toLowerCase()],
  );
  assert.deepEqual(
    codeyExecutablePaths({ root, args: ["--target", "x86_64-pc-windows-msvc"], env }),
    [join(root, "target", "x86_64-pc-windows-msvc", "debug", "codey.exe").toLowerCase()],
  );
  assert.deepEqual(codeyExecutablePaths({ root, args: ["--profile", "../other"], env }), []);
});

test("force termination requires an explicit opt-in", () => {
  assert.equal(forceKillEnabled({}), false);
  assert.equal(forceKillEnabled({ CODEY_DEV_FORCE_KILL: "true" }), false);
  assert.equal(forceKillEnabled({ CODEY_DEV_FORCE_KILL: "1" }), true);
});

test("Windows dev start stops before Cargo unless force termination is requested", () => {
  const messages = [];
  const log = {
    error: (message) => messages.push(message),
    log: () => assert.fail("should not report a forced termination"),
    warn: () => assert.fail("should inspect the matching process successfully"),
  };
  const calls = [];
  const spawn = (...args) => {
    calls.push(args);
    return { status: 0, stdout: "412\r\n" };
  };

  const proceed = ensureUnlockedCodeyOnWindows({
    args: [],
    env: {},
    log,
    platform: "win32",
    root: "/workspace/codey",
    spawn,
  });

  assert.equal(proceed, false);
  assert.equal(calls.length, 1);
  assert.match(calls[0][1][3], /^\$paths = @\(.+\); Get-CimInstance/);
  assert.match(messages[0], /CODEY_DEV_FORCE_KILL=1/);
});

test("Windows dev start verifies a forced termination before continuing", () => {
  const messages = [];
  const responses = [
    { status: 0, stdout: "412\n" },
    { status: 0, stdout: "412\n" },
    { status: 0, stdout: "" },
  ];
  const calls = [];
  const proceed = ensureUnlockedCodeyOnWindows({
    args: [],
    env: { CODEY_DEV_FORCE_KILL: "1" },
    log: {
      error: (message) => messages.push(message),
      log: (message) => messages.push(message),
      warn: (message) => messages.push(message),
    },
    platform: "win32",
    root: "/workspace/codey",
    spawn: (...args) => {
      calls.push(args);
      return responses.shift();
    },
  });

  assert.equal(proceed, true);
  assert.equal(responses.length, 0);
  assert.match(calls[1][1][3], /Where-Object .+ \| ForEach-Object/s);
  assert.deepEqual(messages, ["[dev] 已强制结束本地 Codey 进程：412"]);
});

test("Windows dev start does not continue when a forced termination fails", () => {
  const messages = [];
  const responses = [
    { status: 0, stdout: "412\n" },
    { status: 1, stderr: "access denied" },
  ];
  const proceed = ensureUnlockedCodeyOnWindows({
    args: [],
    env: { CODEY_DEV_FORCE_KILL: "1" },
    log: {
      error: (message) => messages.push(message),
      log: () => assert.fail("should not report a forced termination"),
      warn: () => assert.fail("should not downgrade a termination failure"),
    },
    platform: "win32",
    root: "/workspace/codey",
    spawn: () => responses.shift(),
  });

  assert.equal(proceed, false);
  assert.equal(responses.length, 0);
  assert.match(messages[0], /access denied/);
});
