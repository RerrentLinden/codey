import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";
import vm from "node:vm";
import test from "node:test";

const startup = await readFile(new URL("../backend/src/codex_startup_patch.js", import.meta.url), "utf8");
const compatibility = startup.slice(startup.indexOf("  const cuaCompatibilityLaunchers"),
  startup.indexOf("  const externalPluginFocusReconcileMinIntervalMs"));

// Same policy boundary as the browser runtime; the SDK's two independent
// deadlines are represented by a delayed server response without wall-clock waits.
const policy = `var T=1e4,C;function client(e){if(C!=null)return C;
  return C={client:new sdk.StatsigClient("public",e,{networkConfig:{api:api,sdkExceptionUrl:api+"/sdk_exception"}})}}
async function check(){let t=client({id:"caller"});
  let result=await t.client.initializeAsync({timeoutMs:T});
  if(!result.success||t.client.loadingStatus!=="Ready")
    throw new Error("Unable to load browser request-header policy. Retry the browser command.");
  let gate=t.client.getFeatureGate("codex_browser_use_agent_request_header");
  if(!gate.details.reason.endsWith(":Recognized"))throw new Error("unrecognized policy");
  return gate.value;}`;

test("slow policy responses retain identity, failure checks and gate values", async () => {
  const context = vm.createContext({ process, console, recordCodeyPatchFailure() {} });
  vm.runInContext(compatibility, context);
  const patched = context.__CODEY_PATCH_CUA_BROWSER_POLICY_TIMEOUT__(policy);
  const run = (source, { delay = 18_500, success = true, value = true, reason = "Network:Recognized" } = {}) => {
    class StatsigClient {
      constructor(key, user, options) {
        assert.equal(user.id, "caller");
        this.networkTimeout = options.networkConfig.networkTimeoutMs ?? 10_000;
      }
      async initializeAsync({ timeoutMs }) {
        const ready = success && delay < Math.min(timeoutMs, this.networkTimeout);
        this.loadingStatus = ready ? "Ready" : "Loading";
        return { success: ready };
      }
      getFeatureGate(name) {
        assert.equal(name, "codex_browser_use_agent_request_header");
        return { value, details: { reason } };
      }
    }
    return vm.runInNewContext(source + "check()", { sdk: { StatsigClient }, api: "https://policy.example" });
  };
  await assert.rejects(run(policy), /Unable to load/);
  assert.equal(await run(patched), true);
  assert.equal(await run(patched, { value: false }), false);
  await assert.rejects(run(patched, { reason: "Unrecognized" }), /unrecognized policy/);
  await assert.rejects(run(patched, { success: false }), /Unable to load/);
  await assert.rejects(run(patched, { delay: 30_000 }), /Unable to load/);
  assert.throws(() => context.__CODEY_PATCH_CUA_BROWSER_POLICY_TIMEOUT__(policy.repeat(2)), /Unsupported/);
});

test("official plugin configuration selects an atomic cached copy and keeps original resources", async () => {
  const directory = await mkdtemp(join(tmpdir(), "codey-cua-$&-"));
  const warnings = [];
  const context = vm.createContext({ process, console: { warn: (message) => warnings.push(message) },
    recordCodeyPatchFailure() {} });
  vm.runInContext(compatibility, context);
  try {
    const launcher = join(directory, "plugins", "unified-computer-use", "1.0", "scripts", "launch.mjs");
    const modules = join(directory, "modules");
    const service = join(modules, "@oai", "browser-desktop", "scripts", "browser-service.mjs");
    const launcherSource = 'export default {browser: "@oai/browser-desktop/service", resource:new URL("../resources/banner.js",import.meta.url).href};';
    await Promise.all([mkdir(dirname(launcher), { recursive: true }), mkdir(dirname(service), { recursive: true })]);
    await Promise.all([writeFile(launcher, launcherSource), writeFile(service, policy)]);
    const config = { enabled: true, command: "official-node", args: [launcher], env: {
      CODEX_HOME: directory, NODE_REPL_NODE_MODULE_DIRS: modules,
      NODE_REPL_TRUSTED_CODE_PATHS: [directory, modules].join(delimiter),
      CUA_REPL_ENABLED_SURFACES: "browser,computer", unchanged: "keep",
    } };
    const original = structuredClone(config);
    const fixture = 'async function configure(e){let c=e;const p={join(){return e.args[0]}};let i="plugin";c.env.CUA_REPL_NODE_REPL_PATH="official";e!=null&&(c.command=e.command,c.args=[p.join(i,`scripts/launch.mjs`)]);return c;}';
    vm.runInContext(context.__CODEY_PATCH_CODEX_CUA_PLUGIN_CONFIG__(fixture), context);
    const configured = await context.configure(config);
    assert.notEqual(configured.args[0], launcher);
    assert.deepEqual(configured.env, { ...original.env, CUA_REPL_NODE_REPL_PATH: "official" });
    assert.equal(configured.command, original.command);
    const loaded = (await import(pathToFileURL(configured.args[0]).href)).default;
    assert.equal(loaded.resource, new URL("../resources/banner.js", pathToFileURL(launcher)).href);
    assert.match(await readFile(loaded.browser, "utf8"), /networkTimeoutMs:25e3/);
    assert.equal(await readFile(service, "utf8"), policy);
    assert.equal(await readFile(launcher, "utf8"), launcherSource);
    const again = structuredClone(original);
    await context.__CODEY_PREPARE_CUA_COMPATIBILITY_LAUNCHER__(again);
    assert.equal(again.args[0], configured.args[0]);
    const disabled = { ...structuredClone(original), enabled: false };
    await context.__CODEY_PREPARE_CUA_COMPATIBILITY_LAUNCHER__(disabled);
    assert.equal(disabled.args[0], launcher);
    assert.equal(warnings.length, 0);

    // A new incompatible upstream bundle keeps its original launcher and reports
    // the skipped compatibility patch; it must never create a partial override.
    const otherModules = join(directory, "updated-modules");
    const otherService = join(otherModules, "@oai", "browser-desktop", "scripts", "browser-service.mjs");
    await mkdir(dirname(otherService), { recursive: true });
    await writeFile(otherService, "changed upstream runtime");
    const changed = structuredClone(original);
    changed.env.NODE_REPL_NODE_MODULE_DIRS = otherModules;
    await context.__CODEY_PREPARE_CUA_COMPATIBILITY_LAUNCHER__(changed);
    assert.equal(changed.args[0], launcher);
    assert.equal(warnings.length, 1);
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("direct runtime configurations redirect the mapped browser service", async () => {
  const directory = await mkdtemp(join(tmpdir(), "codey-cua-direct-"));
  const warnings = [];
  const context = vm.createContext({ process, console: { warn: (message) => warnings.push(message) },
    recordCodeyPatchFailure() {} });
  vm.runInContext(compatibility, context);
  try {
    const modules = join(directory, "cua_node", "lib", "node_modules");
    const service = join(modules, "@oai", "browser-desktop", "scripts", "browser-service.mjs");
    const runtime = join(modules, "@oai", "cua-repl", "bin", "cua-repl.mjs");
    await mkdir(dirname(service), { recursive: true });
    await writeFile(service, policy);
    const config = { enabled: true, env: {
      CODEX_HOME: directory, NODE_REPL_NODE_MODULE_DIRS: modules,
      NODE_REPL_TRUSTED_CODE_PATHS: [directory, modules].join(delimiter),
      CUA_REPL_ENABLED_SURFACES: "browser,computer",
      NODE_REPL_TRUSTED_SERVICES: JSON.stringify({ browser: "@oai/browser-desktop/service", sky: "@oai/sky/service" }),
    } };
    const original = structuredClone(config);
    // 与新版主包一致：属性名被压缩，运行时入口由平台的 path.join 拼接，
    // 因此 Windows 上同样得到反斜杠路径。
    const fixture = 'async function configure(e){let c=e;const n={dirs:"NODE_REPL_NODE_MODULE_DIRS"},p={default:process.getBuiltinModule("path")};c.env.CUA_REPL_NODE_REPL_PATH="official";e.env!=null&&(c.command="official-node",c.args=[p.default.join(e.env[n.dirs],`@oai/cua-repl/bin/cua-repl.mjs`)]);return c;}';
    vm.runInContext(context.__CODEY_PATCH_CODEX_CUA_PLUGIN_CONFIG__(fixture), context);
    const configured = await context.configure(config);
    assert.equal(configured.args[0], runtime);
    assert.equal(configured.command, "official-node");
    const services = JSON.parse(configured.env.NODE_REPL_TRUSTED_SERVICES);
    assert.equal(services.sky, "@oai/sky/service");
    assert.equal(dirname(dirname(services.browser)), join(directory, ".tmp", "codey-cua"));
    const cached = await readFile(services.browser, "utf8");
    assert.match(cached, /networkTimeoutMs:25e3/);
    assert.match(cached, /Unable to load browser request-header policy\./);
    assert.deepEqual(await readdir(dirname(services.browser)), ["browser-service.mjs"]);
    assert.equal(await readFile(service, "utf8"), policy);
    const again = structuredClone(original);
    await context.__CODEY_PREPARE_CUA_COMPATIBILITY_LAUNCHER__(again);
    assert.equal(JSON.parse(again.env.NODE_REPL_TRUSTED_SERVICES).browser, services.browser);
    assert.equal(warnings.length, 0);
    // A build that names the bundled browser implementation by path keeps that
    // implementation and replaces only the file that is actually loaded.
    const pluginService = join(directory, "plugins", "browser", "scripts", "browser-service.mjs");
    await mkdir(dirname(pluginService), { recursive: true });
    await writeFile(pluginService, ["// browser plugin service", policy].join("\n"));
    const mapped = { enabled: true, env: { ...structuredClone(original.env),
      NODE_REPL_TRUSTED_SERVICES: JSON.stringify({ browser: pluginService, sky: "@oai/sky/service" }) } };
    await context.__CODEY_PREPARE_CUA_COMPATIBILITY_LAUNCHER__(mapped);
    const mappedServices = JSON.parse(mapped.env.NODE_REPL_TRUSTED_SERVICES);
    assert.notEqual(mappedServices.browser, pluginService);
    const mappedCache = await readFile(mappedServices.browser, "utf8");
    assert.match(mappedCache, /^\/\/ browser plugin service/);
    assert.match(mappedCache, /networkTimeoutMs:25e3/);
    assert.equal(await readFile(pluginService, "utf8"), ["// browser plugin service", policy].join("\n"));
    assert.equal(warnings.length, 0);

    // A build whose browser service no longer matches the policy anchors keeps
    // the configured specifier and reports the skipped compatibility patch.
    const otherModules = join(directory, "updated-modules");
    const otherService = join(otherModules, "@oai", "browser-desktop", "scripts", "browser-service.mjs");
    await mkdir(dirname(otherService), { recursive: true });
    await writeFile(otherService, "changed upstream runtime");
    const changed = structuredClone(original);
    changed.env.NODE_REPL_NODE_MODULE_DIRS = otherModules;
    await context.__CODEY_PREPARE_CUA_COMPATIBILITY_LAUNCHER__(changed);
    assert.equal(changed.env.NODE_REPL_TRUSTED_SERVICES, original.env.NODE_REPL_TRUSTED_SERVICES);
    assert.equal(warnings.length, 1);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
