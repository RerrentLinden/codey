#!/usr/bin/env python3
"""打包已构建的 Codey 原生插件；不构建、不加载、不执行动态库。"""
import argparse
import hashlib
import json
import pathlib
import platform
import zipfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--library", type=pathlib.Path, required=True)
parser.add_argument("--schema", type=pathlib.Path, required=True)
parser.add_argument("--output", type=pathlib.Path, required=True)
parser.add_argument("--id", required=True)
parser.add_argument("--name", required=True)
parser.add_argument("--version", required=True)
parser.add_argument("--platform", choices=["macos", "windows", "linux"], default={"Darwin":"macos", "Windows":"windows", "Linux":"linux"}.get(platform.system()))
parser.add_argument("--arch", default={"arm64":"aarch64", "AMD64":"x86_64"}.get(platform.machine(), platform.machine()))
parser.add_argument("--header", action="append", default=[])
args = parser.parse_args()
if args.output.suffix != ".codey-plugin":
    parser.error("输出文件必须使用 .codey-plugin 扩展名")
library = args.library.read_bytes()
schema = args.schema.read_bytes()
json.loads(schema)
entry = "lib/" + args.library.name
manifest = {
    "id": args.id, "name": args.name, "version": args.version,
    "abiVersion": 1, "platform": args.platform, "arch": args.arch,
    "entry": entry, "librarySha256": hashlib.sha256(library).hexdigest(),
    "capabilities": ["request.beforeSend"] if args.header else [],
    "headerNames": args.header, "configSchema": "config.schema.json"
}
args.output.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(args.output, "x", compression=zipfile.ZIP_DEFLATED) as package:
    package.writestr("manifest.json", json.dumps(manifest, ensure_ascii=False, indent=2))
    package.writestr(entry, library)
    package.writestr("config.schema.json", schema)
print(args.output)
