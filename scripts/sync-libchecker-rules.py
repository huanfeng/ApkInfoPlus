#!/usr/bin/env python3
"""从 LibChecker-Rules 上游拉取 v5 portable 规则包，校验后重新打包成内嵌用的 rules.zip。

用法：
  python3 scripts/sync-libchecker-rules.py [--output PATH] [--version N]

- 不带 --version 时取上游清单里的最新版；也可用环境变量 LIBCHECKER_RULES_VERSION 固定版本。
- 输出默认是 crates/core/assets/libchecker/rules.zip（已 gitignore），构建时由 core 的 build.rs 内嵌。
- 重新打包只改变布局与压缩方式，不改规则内容：details/ 合并成 details.json（相同内容只存一份），
  全部条目改用 deflate，并附上上游 LICENSE（Apache-2.0）。细节见 docs/libchecker.md §4。

只用 Python 标准库，CI 三平台都能直接跑。
"""

import argparse
import hashlib
import io
import json
import os
import sys
import urllib.request
import zipfile

ROOT = "https://raw.githubusercontent.com/LibChecker/LibChecker-Rules/rules-data/"
LICENSE_URL = "https://raw.githubusercontent.com/LibChecker/LibChecker-Rules/v4/LICENSE"
READER_VERSION = 5
DEFAULT_OUTPUT = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "..", "crates", "core", "assets", "libchecker", "rules.zip"
)
# 原样带进重新打包的条目（details/ 另行合并）
KEEP = ("metadata.json", "core.json", "matching-fixtures.json")


def fetch(url):
    req = urllib.request.Request(url, headers={"User-Agent": "apkinfoplus-rules-sync"})
    with urllib.request.urlopen(req, timeout=60) as resp:
        return resp.read()


def load_manifest(version):
    url = ROOT + (f"releases/{version}/manifest.json" if version else "manifest.json")
    manifest = json.loads(fetch(url))
    if manifest.get("schemaVersion") != READER_VERSION:
        sys.exit(f"上游规则 schemaVersion={manifest.get('schemaVersion')}，本工具只支持 {READER_VERSION}")
    minimum = manifest.get("minimumReader", {}).get("portable", 0)
    if minimum > READER_VERSION:
        sys.exit(f"上游规则要求 portable 读取器 >= {minimum}，本工具是 {READER_VERSION}")
    return manifest


def download_portable(manifest):
    art = manifest["artifacts"]["portable"]
    data = fetch(ROOT + art["path"])
    if len(data) != art["size"]:
        sys.exit(f"portable 包大小不符：{len(data)} != {art['size']}")
    digest = hashlib.sha256(data).hexdigest()
    if digest != art["sha256"]:
        sys.exit(f"portable 包 sha256 不符：{digest} != {art['sha256']}")
    return data


def repack(portable, license_text):
    src = zipfile.ZipFile(io.BytesIO(portable))
    names = set(src.namelist())
    for name in KEEP:
        if name not in names:
            sys.exit(f"portable 包缺少 {name}")

    paths, data, seen = {}, [], {}
    for name in sorted(names):
        if not (name.startswith("details/") and name.endswith(".json")):
            continue
        entries = json.loads(src.read(name))["data"]
        key = json.dumps(entries, ensure_ascii=False, sort_keys=True)
        if key not in seen:
            seen[key] = len(data)
            data.append(entries)
        paths[name] = seen[key]

    out = io.BytesIO()
    # 固定时间戳，同一版规则重复打包得到相同字节
    def write(zf, name, payload):
        info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        zf.writestr(info, payload, compresslevel=9)

    with zipfile.ZipFile(out, "w") as zf:
        for name in KEEP:
            write(zf, name, src.read(name))
        details = {"paths": paths, "data": data}
        write(zf, "details.json", json.dumps(details, ensure_ascii=False, separators=(",", ":")).encode())
        for name in sorted(n for n in names if n.startswith("icons/") and n.endswith(".svg")):
            write(zf, name, src.read(name))
        write(zf, "LICENSE", license_text)
    return out.getvalue(), len(paths), len(data)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--output", default=DEFAULT_OUTPUT)
    ap.add_argument("--version", type=int, default=os.environ.get("LIBCHECKER_RULES_VERSION") or None)
    args = ap.parse_args()

    manifest = load_manifest(args.version)
    portable = download_portable(manifest)
    license_text = fetch(LICENSE_URL)
    packed, detail_files, detail_unique = repack(portable, license_text)

    output = os.path.abspath(args.output)
    os.makedirs(os.path.dirname(output), exist_ok=True)
    with open(output, "wb") as f:
        f.write(packed)
    # 许可文本单独放一份，打包脚本把它附进发布包
    with open(os.path.join(os.path.dirname(output), "LICENSE"), "wb") as f:
        f.write(license_text)

    print(
        f"LibChecker-Rules dataVersion={manifest['dataVersion']} rules={manifest['ruleCount']} "
        f"details={detail_files}->{detail_unique} portable={len(portable)}B -> {output} ({len(packed)}B)"
    )


if __name__ == "__main__":
    main()
