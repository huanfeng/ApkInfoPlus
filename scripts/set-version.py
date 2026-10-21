#!/usr/bin/env python3
"""把版本号写进 Cargo.toml（workspace.package）与 Windows 资源 app.rc。

发布流程按 tag 调用：`python3 scripts/set-version.py 1.2.3`。
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> None:
    if len(sys.argv) != 2 or not re.fullmatch(r"\d+\.\d+\.\d+", sys.argv[1]):
        sys.exit("usage: set-version.py X.Y.Z")
    ver = sys.argv[1]
    nums = ver.split(".") + ["0"]

    cargo = ROOT / "Cargo.toml"
    text = cargo.read_text(encoding="utf-8")
    text, n = re.subn(
        r'(\[workspace\.package\][^\[]*?\nversion = )"[^"]*"', rf'\g<1>"{ver}"', text, count=1
    )
    if n != 1:
        sys.exit("Cargo.toml: 找不到 [workspace.package] 的 version")
    cargo.write_text(text, encoding="utf-8")

    rc = ROOT / "crates/app/assets/windows/app.rc"
    text = rc.read_text(encoding="utf-8")
    text = re.sub(r"(FILEVERSION|PRODUCTVERSION)(\s+)[\d,]+", rf"\1\g<2>{','.join(nums)}", text)
    text = re.sub(r'(VALUE "(?:FileVersion|ProductVersion)", )"[^"]*"', rf'\1"{ver}"', text)
    rc.write_text(text, encoding="utf-8")
    print(f"version -> {ver}")


if __name__ == "__main__":
    main()
