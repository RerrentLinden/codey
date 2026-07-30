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
});
