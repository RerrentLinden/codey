import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const paths = {
  app: new URL("../src/App.tsx", import.meta.url),
  sections: new URL("../src/ExperimentalFeaturesCard.tsx", import.meta.url),
  types: new URL("../src/App.types.ts", import.meta.url),
  config: new URL("../backend/src/config.rs", import.meta.url),
  commands: new URL("../backend/src/commands.rs", import.meta.url),
  runtimeCommands: new URL("../backend/src/commands/runtime.rs", import.meta.url),
  cdp: new URL("../backend/src/cdp.rs", import.meta.url),
  launcher: new URL("../backend/src/launcher.rs", import.meta.url),
  patch: new URL("../backend/src/codex_startup_patch.rs", import.meta.url),
};

const flags = [
  "unified_exec",
  "shell_snapshot",
  "responses_websockets_v2",
  "tool_search_always_defer_mcp_tools",
  "standalone_web_search",
  "enable_request_compression",
  "remote_compaction_v2",
  "apply_patch_streaming_events",
  "concurrent_reasoning_summaries",
];

const normalizeLineEndings = (source) => source.replace(/\r\n/g, "\n");

async function sources() {
  return Object.fromEntries(
    await Promise.all(
      Object.entries(paths).map(async ([key, path]) => [
        key,
        normalizeLineEndings(await readFile(path, "utf8")),
      ]),
    ),
  );
}

test("the experimental feature card exposes all switches with concise descriptions", async () => {
  const { app, sections, types } = await sources();

  assert.match(app, /<ExperimentalFeaturesCard/);
  assert.match(app, /sync_official_experimental_features/);
  assert.match(app, /保存并重启 Codex 后生效/);
  assert.match(sections, /同步官方配置/);
  assert.match(sections, /打开配置面板时读取一次运行态/);
  assert.match(sections, /运行态与当前页面配置一致/);
  assert.match(sections, /不点击同步时会一直使用已保存的用户配置/);
  assert.match(types, /experimentalFeatures: ExperimentalFeaturesConfig/);
  assert.match(
    types,
    /experimentalFeatureRuntime\?: ExperimentalFeatureRuntimeStatus/,
  );

  for (const flag of flags) {
    assert.match(sections, new RegExp(`flag: "${flag}"`));
  }
  const featureDefinitions = sections.slice(
    sections.indexOf("const EXPERIMENTAL_FEATURES"),
    sections.indexOf("type ExperimentalFeaturesCardProps"),
  );
  const descriptions = featureDefinitions.match(/description: "[^"]+"/g) ?? [];
  assert.equal(descriptions.length, flags.length);
});

test("user feature settings persist, require restart, and override official values last", async () => {
  const { config, commands, runtimeCommands, cdp, launcher, patch } = await sources();

  assert.match(config, /pub experimental_features: ExperimentalFeaturesConfig/);
  assert.match(
    commands,
    /config\.experimental_features = config_input\.experimental_features/,
  );
  assert.match(
    commands,
    /applied\.experimental_features != current\.experimental_features/,
  );
  assert.match(commands, /"sync_official_experimental_features"/);
  assert.match(runtimeCommands, /"experimentalFeatureRuntime"/);
  assert.match(cdp, /_store\?\._valuesForExternalUse/);
  assert.match(cdp, /read_experimental_feature_runtime_status/);
  assert.match(cdp, /__CODEY_EXPERIMENTAL_FEATURE_RUNTIME__/);
  assert.match(cdp, /layer\?\.value\?\.feature_overrides/);
  assert.match(launcher, /config\.experimental_features/);
  assert.match(launcher, /experimental_feature_runtime/);
  assert.match(
    patch,
    /\.\.\.\$\{JSON\.stringify\(experimentalFeatureOverrides\)\}/,
  );
  assert.match(patch, /__CODEY_EXPERIMENTAL_FEATURE_RUNTIME__/);
  assert.match(patch, /codey-startup-patch-installed-v19/);
});

test("official synchronization reads raw Statsig gates and applies the official layer", async () => {
  const { cdp } = await sources();
  const script = cdp.match(
    /fn official_experimental_features_script\(\) -> &'static str \{\n\s+r#\"([\s\S]*?)\"#\n\}/,
  )?.[1];
  assert.ok(script);

  const gate = (value) => ({ value });
  const raw = {
    feature_gates: {
      1786883712: gate(false),
      1615536597: gate(true),
      2734851136: gate(false),
      2701734443: gate(false),
      3701003275: gate(false),
      30039772: gate(true),
      321109023: gate(true),
      358284800: gate(true),
      2508143457: gate(true),
    },
    dynamic_configs: {
      3902942138: {
        value: {
          feature_overrides: {
            unified_exec: true,
            shell_snapshot: false,
          },
        },
      },
    },
  };
  const payload = Function(
    "globalThis",
    `return ${script}`,
  )({
    __STATSIG__: {
      firstInstance: {
        _store: { _valuesForExternalUse: raw },
      },
    },
  });

  assert.deepEqual(JSON.parse(payload), {
    unifiedExec: true,
    shellSnapshot: false,
    responsesWebsocketsV2: false,
    toolSearchAlwaysDeferMcpTools: false,
    standaloneWebSearch: false,
    enableRequestCompression: true,
    remoteCompactionV2: true,
    applyPatchStreamingEvents: true,
    concurrentReasoningSummaries: true,
  });
});
