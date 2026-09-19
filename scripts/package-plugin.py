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
parser.add_argument("--config-ui", type=pathlib.Path, help="可选单文件 UTF-8 HTML 配置页（最多 1 MiB，内嵌 CSS/JS）")
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
config_ui = None
if args.config_ui is not None:
    if args.config_ui.is_symlink() or not args.config_ui.is_file():
        parser.error("配置页必须是普通文件，不能是符号链接")
    with args.config_ui.open("rb") as source:
        config_ui = source.read(1024 * 1024 + 1)
    if len(config_ui) > 1024 * 1024:
        parser.error("配置页超过 1 MiB")
    try:
        config_ui.decode("utf-8")
    except UnicodeDecodeError:
        parser.error("配置页必须为 UTF-8")
    manifest["configUi"] = {"type": "html", "entry": "ui/config.html", "sha256": hashlib.sha256(config_ui).hexdigest()}
args.output.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(args.output, "x", compression=zipfile.ZIP_DEFLATED) as package:
    package.writestr("manifest.json", json.dumps(manifest, ensure_ascii=False, indent=2))
    package.writestr(entry, library)
    package.writestr("config.schema.json", schema)
    if config_ui is not None:
        package.writestr("ui/config.html", config_ui)
print(args.output)
