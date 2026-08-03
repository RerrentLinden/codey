import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

const workflow = fs.readFileSync(
  new URL("../.github/workflows/build-desktop.yml", import.meta.url),
  "utf8",
);
const ciWorkflow = fs.readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);
const macBuildScript = fs.readFileSync(
  new URL("../scripts/build.mjs", import.meta.url),
  "utf8",
);
const windowsInstallerScript = fs.readFileSync(
  new URL("../scripts/installer/windows/Codey.nsi", import.meta.url),
  "utf8",
);

function assertRustQualityGates(job) {
  assert.match(job, /components: rustfmt, clippy/);
  assert.match(job, /cargo fmt --all -- --check/);
  assert.match(
    job,
    /cargo fmt --manifest-path vendor\/CodeyRuntime\/Cargo\.toml --all -- --check/,
  );
  assert.match(job, /cargo test --workspace --locked/);
  assert.match(
    job,
    /cargo test --manifest-path vendor\/CodeyRuntime\/Cargo\.toml --workspace --locked/,
  );
  assert.match(
    job,
    /cargo clippy --workspace --all-targets --locked -- -D warnings/,
  );
  assert.match(
    job,
    /cargo clippy --manifest-path vendor\/CodeyRuntime\/Cargo\.toml --workspace --all-targets --locked -- -D warnings/,
  );
}

test("every desktop release build enforces the Rust quality gates", () => {
  assert.match(workflow, /^\s*RUSTFLAGS: -D warnings$/m);
  const jobs = [
    workflow.slice(workflow.indexOf("\n  macos:"), workflow.indexOf("\n  windows:")),
    workflow.slice(workflow.indexOf("\n  windows:"), workflow.indexOf("\n  publish:")),
  ];

  for (const job of jobs) {
    assertRustQualityGates(job);
  }
});

test("pull requests enforce Rust quality gates for both workspaces", () => {
  assert.match(ciWorkflow, /^\s*RUSTFLAGS: -D warnings$/m);
  assertRustQualityGates(ciWorkflow);
  const windowsJob = ciWorkflow.slice(ciWorkflow.indexOf("\n  windows-rust:"));
  assert.match(windowsJob, /runs-on: windows-latest/);
  assert.match(windowsJob, /components: clippy/);
  assert.match(windowsJob, /cargo test --workspace --locked/);
  assert.match(
    windowsJob,
    /cargo test --manifest-path vendor\/CodeyRuntime\/Cargo\.toml --workspace --locked/,
  );
  assert.match(
    windowsJob,
    /cargo clippy --workspace --all-targets --locked -- -D warnings/,
  );
  assert.match(
    windowsJob,
    /cargo clippy --manifest-path vendor\/CodeyRuntime\/Cargo\.toml --workspace --all-targets --locked -- -D warnings/,
  );
});

test("desktop packages include FastCtx license and notice files", () => {
  for (const expected of [
    "README.md",
    "LICENSE",
    "THIRD_PARTY_NOTICES.md",
    "licenses/FastCtx/LICENSE-APACHE",
    "licenses/FastCtx/NOTICE",
  ]) {
    assert.match(macBuildScript, new RegExp(expected.replaceAll("/", "\\/")));
  }

  assert.match(workflow, /Contents\/Resources\/licenses\/FastCtx\/LICENSE-APACHE/);
  assert.match(workflow, /Contents\/Resources\/licenses\/FastCtx\/NOTICE/);
  assert.match(windowsInstallerScript, /licenses\\FastCtx\\LICENSE-APACHE/);
  assert.match(windowsInstallerScript, /licenses\\FastCtx\\NOTICE/);
});

test("Windows release publishes the installer without a portable zip", () => {
  assert.match(workflow, /name: codey-windows-x64-installer/);
  assert.match(workflow, /windows-x64-setup\.exe/);
  assert.doesNotMatch(workflow, /windows-x64-portable\.zip/);
  assert.doesNotMatch(workflow, /codey-windows-x64-portable/);
});
