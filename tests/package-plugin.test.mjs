import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const python = ["python3", "python"].find(command => {
  const result = spawnSync(command, ["--version"], { encoding: "utf8" });
  return result.status === 0 && /Python 3\./.test(result.stdout);
});
const options = { skip: python ? false : "插件打包测试需要 Python 3" };
const script = fileURLToPath(new URL("../scripts/package-plugin.py", import.meta.url));
const digest = value => createHash("sha256").update(value).digest("hex");

function fixture(t) {
  const dir = mkdtempSync(join(tmpdir(), "codey-plugin-package-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const library = join(dir, "demo.so"), schema = join(dir, "schema.json"), output = join(dir, "demo.codey-plugin");
  // 打包测试只处理字节，不加载动态库。
  writeFileSync(library, "test library bytes");
  writeFileSync(schema, '{"type":"object","properties":{}}');
  return { dir, library, schema, output, run: (extra = []) => spawnSync(python, [script,
    "--library", library, "--schema", schema, "--output", output,
    "--id", "test.demo", "--name", "打包测试", "--version", "1.0.0",
    "--platform", "linux", "--arch", "x86_64", ...extra,
  ], { encoding: "utf8" }) };
}

function archive(path) {
  const result = spawnSync(python, ["-c", `import base64,json,sys,zipfile
with zipfile.ZipFile(sys.argv[1]) as package:
    assert package.testzip() is None
    print(json.dumps({name:base64.b64encode(package.read(name)).decode() for name in package.namelist()}))`, path], { encoding: "utf8", maxBuffer: 2 * 1024 * 1024 });
  assert.equal(result.status, 0, result.stderr);
  return Object.fromEntries(Object.entries(JSON.parse(result.stdout)).map(([name, data]) => [name, Buffer.from(data, "base64")]));
}

test("plugin package preserves library, schema and manifest without overwriting output", options, t => {
  const f = fixture(t);
  assert.equal(f.run(["--header", "X-Demo"]).status, 0);
  const files = archive(f.output), manifest = JSON.parse(files["manifest.json"]);
  assert.deepEqual(Object.keys(files).sort(), ["config.schema.json", "lib/demo.so", "manifest.json"]);
  assert.deepEqual(files["lib/demo.so"], readFileSync(f.library));
  assert.deepEqual(files["config.schema.json"], readFileSync(f.schema));
  assert.deepEqual(manifest, {
    id: "test.demo", name: "打包测试", version: "1.0.0", abiVersion: 1,
    platform: "linux", arch: "x86_64", entry: "lib/demo.so",
    librarySha256: digest(readFileSync(f.library)), capabilities: ["request.beforeSend"],
    headerNames: ["X-Demo"], configSchema: "config.schema.json",
  });
  const saved = readFileSync(f.output);
  assert.notEqual(f.run().status, 0);
  assert.deepEqual(readFileSync(f.output), saved);
});

test("plugin package accepts a UTF-8 configuration page at the size limit", options, t => {
  const f = fixture(t), ui = join(f.dir, "config.html");
  const html = Buffer.concat([Buffer.from("<p>配置</p>"), Buffer.alloc(1024 * 1024 - Buffer.byteLength("<p>配置</p>"), 32)]);
  writeFileSync(ui, html);
  const result = f.run(["--config-ui", ui]);
  assert.equal(result.status, 0, result.stderr);
  const files = archive(f.output), manifest = JSON.parse(files["manifest.json"]);
  assert.deepEqual(files["ui/config.html"], html);
  assert.deepEqual(manifest.configUi, { type: "html", entry: "ui/config.html", sha256: digest(html) });
  assert.deepEqual(manifest.capabilities, []);
});

for (const [name, content] of [["oversized", Buffer.alloc(1024 * 1024 + 1)], ["invalid UTF-8", Buffer.from([0xff])]]) {
  test(`plugin package rejects ${name} configuration pages before writing output`, options, t => {
    const f = fixture(t), ui = join(f.dir, "config.html");
    writeFileSync(ui, content);
    assert.notEqual(f.run(["--config-ui", ui]).status, 0);
    assert.equal(existsSync(f.output), false);
  });
}

test("plugin package rejects configuration page symlinks", options, t => {
  const f = fixture(t), ui = join(f.dir, "config.html"), link = join(f.dir, "link.html");
  writeFileSync(ui, "<p>配置</p>");
  try { symlinkSync(ui, link, "file"); }
  catch (error) {
    if (process.platform === "win32" && ["EPERM", "EACCES"].includes(error.code)) return t.skip("当前 Windows 账号无创建符号链接权限");
    throw error;
  }
  assert.notEqual(f.run(["--config-ui", link]).status, 0);
  assert.equal(existsSync(f.output), false);
});

test("plugin package rejects invalid schema and output extension before writing", options, t => {
  const f = fixture(t), invalidOutput = join(f.dir, "demo.zip");
  assert.notEqual(f.run(["--output", invalidOutput]).status, 0);
  assert.equal(existsSync(invalidOutput), false);
  writeFileSync(f.schema, "invalid JSON");
  assert.notEqual(f.run().status, 0);
  assert.equal(existsSync(f.output), false);
});
