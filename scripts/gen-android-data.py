#!/usr/bin/env python3
"""从 Android SDK 与 ICU 生成权限 / 语言名称数据（生成结果入库，运行时不依赖 SDK）。

用法：
  python3 scripts/gen-android-data.py [--jar PATH]

- 权限：读 `android.jar` 里的框架 AndroidManifest.xml（protectionLevel、label / description
  引用）与 resources.arsc（各语言的 permlab_* / permdesc_* 字符串），经 `aapt2 dump` 取出。
  不带 --jar 时取 $ANDROID_HOME（或 $ANDROID_SDK_ROOT、~/Android/sdk）下 API 最高的平台。
- 语言 / 地区 / 文字名称：由 Node 自带的 ICU（`Intl.DisplayNames`）给出。

输出：
- crates/core/src/android/generated.rs：权限保护级别表与语言自称（autonym）表；
- crates/app/i18n/android/<界面语言>.toml：权限标题 / 说明、语言 / 地区 / 文字名称，
  作为 i18n 附加层随界面语言切换。

依赖 aapt2 与 node（≥ 14，需完整 ICU，官方构建默认即是）。
"""

import argparse
import glob
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.join(HERE, "..")
CORE_OUT = os.path.join(REPO, "crates", "core", "src", "android", "generated.rs")
I18N_DIR = os.path.join(REPO, "crates", "app", "i18n", "android")

# 界面语言 → android.jar 资源限定符 / ICU locale
UI_LANGS = {
    "zh-CN": ("zh-rCN", "zh-CN"),
    "en": ("", "en"),
    "zh-TW": ("zh-rTW", "zh-TW"),
    "zh-HK": ("zh-rHK", "zh-HK"),
    "ja": ("ja", "ja"),
    "ko": ("ko", "ko"),
}
PERM_PREFIX = "android.permission."

# Android 资源限定符里常见的文字（script）子标签；ICU 无法枚举，列常用的即可。
SCRIPTS = (
    "Adlm Arab Armn Beng Cans Cher Cyrl Deva Ethi Geor Grek Gujr Guru Hang Hans Hant "
    "Hebr Hira Jpan Kana Khmr Knda Kore Laoo Latn Mlym Mong Mtei Mymr Nkoo Olck Orya "
    "Rohg Sinh Syrc Taml Telu Tfng Thaa Thai Tibt Vaii"
).split()


def find_jar():
    roots = [os.environ.get("ANDROID_HOME"), os.environ.get("ANDROID_SDK_ROOT"),
             os.path.expanduser("~/Android/sdk")]
    for root in filter(None, roots):
        jars = glob.glob(os.path.join(root, "platforms", "android-*", "android.jar"))
        if jars:
            def api(p):
                m = re.search(r"android-(\d+)", p)
                return int(m.group(1)) if m else 0
            return max(jars, key=api)
    sys.exit("找不到 android.jar：请用 --jar 指定，或设置 ANDROID_HOME")


def aapt2(*args):
    return subprocess.run(["aapt2", "dump", *args], check=True, capture_output=True,
                          text=True).stdout


def unescape(s):
    s = re.sub(r"\\u([0-9a-fA-F]{4})", lambda m: chr(int(m.group(1), 16)), s)
    return s.replace("\\n", " ").replace("\\'", "'").replace('\\"', '"').replace("\\\\", "\\")


def parse_resources(jar):
    """resource id（int）→ {限定符: 字符串}，只收 string 资源。"""
    out = {}
    cur = None
    for line in aapt2("resources", jar).splitlines():
        m = re.match(r"\s+resource 0x([0-9a-f]+) string/", line)
        if m:
            cur = out.setdefault(int(m.group(1), 16), {})
            continue
        if re.match(r"\s+resource ", line):
            cur = None
            continue
        if cur is None:
            continue
        m = re.match(r'\s+\(([^)]*)\) "(.*)"$', line)
        if m:
            cur.setdefault(m.group(1), unescape(m.group(2)))
    return out


def parse_permissions(jar):
    """[(name, protectionLevel, label_id, desc_id)]，同名只取第一条（featureFlag 变体）。"""
    perms = []
    cur = None
    for line in aapt2("xmltree", "--file", "AndroidManifest.xml", jar).splitlines():
        if re.match(r"\s+E: ", line):
            cur = {} if re.match(r"\s+E: permission \(", line) else None
            if cur is not None:
                perms.append(cur)
            continue
        if cur is None:
            continue
        m = re.search(r':(name|protectionLevel|label|description)\(0x[0-9a-f]+\)=(.*)$', line)
        if not m:
            continue
        key, val = m.groups()
        if key == "name":
            cur["name"] = re.match(r'"([^"]*)"', val).group(1)
        elif key == "protectionLevel":
            cur["level"] = int(val, 16)
        else:
            ref = re.match(r"@0x([0-9a-f]+)", val)
            if ref:
                cur[key] = int(ref.group(1), 16)
    seen = {}
    for p in perms:
        if "name" in p and p["name"] not in seen:
            seen[p["name"]] = p
    return sorted(seen.values(), key=lambda p: p["name"])


ICU_JS = r"""
const [uiLangs, scripts] = JSON.parse(process.argv[1]);
const letters = 'abcdefghijklmnopqrstuvwxyz';
const langCodes = [];
for (const a of letters) for (const b of letters) langCodes.push(a + b);
for (const a of letters) for (const b of letters) for (const c of letters) langCodes.push(a + b + c);
const regionCodes = [];
for (const a of letters) for (const b of letters) regionCodes.push((a + b).toUpperCase());
for (let i = 1; i < 1000; i++) regionCodes.push(String(i).padStart(3, '0'));
// 个别代码（保留字之类）会让 of() 抛异常，统一当作不认识。
const wrap = d => ({ of: c => { try { return d.of(c); } catch (e) { return undefined; } } });
const dn = (loc, type) => wrap(new Intl.DisplayNames([loc], { type, fallback: 'none' }));
const en = dn('en', 'language'), enR = dn('en', 'region');
// 只要 ICU 认得的代码（英文名存在且不等于代码本身）。
const langs = langCodes.filter(c => { const n = en.of(c); return n && n !== c; });
const regions = regionCodes.filter(c => { const n = enR.of(c); return n && n !== c; });
const out = { langs: {}, autonyms: {} };
for (const ui of uiLangs) {
  const l = dn(ui, 'language'), r = dn(ui, 'region'), s = dn(ui, 'script');
  const rs = wrap(new Intl.DisplayNames([ui], { type: 'region', style: 'short', fallback: 'none' }));
  out.langs[ui] = {
    lang: Object.fromEntries(langs.map(c => [c, l.of(c)]).filter(x => x[1])),
    region: Object.fromEntries(regions.map(c => [c, rs.of(c) || r.of(c)]).filter(x => x[1])),
    script: Object.fromEntries(scripts.map(c => [c, s.of(c)]).filter(x => x[1])),
  };
}
for (const c of langs) {
  let n;
  try { n = dn(c, 'language').of(c); } catch (e) { n = undefined; } // 构造本身也可能抛
  // ICU 没有该语言自己的数据时会退回英文名，那不算自称。
  const known = Intl.DisplayNames.supportedLocalesOf([c]).length > 0;
  if (n && known) out.autonyms[c] = n;
}
console.log(JSON.stringify(out));
"""


def icu_names():
    icu = [loc for _, loc in UI_LANGS.values()]
    res = subprocess.run(["node", "-e", ICU_JS, json.dumps([icu, SCRIPTS])], check=True,
                         capture_output=True, text=True)
    return json.loads(res.stdout)


def toml_str(s):
    # 花括号是 windui 的占位符语法，字面量要双写。
    s = s.replace("{", "{{").replace("}", "}}")
    return json.dumps(s, ensure_ascii=False)


def rust_str(s):
    return json.dumps(s, ensure_ascii=False)


LEVEL_NOTE = "protectionLevel 原值：低 4 位是基本级别，其余位是附加标志（见 R.attr#protectionLevel）"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jar")
    args = ap.parse_args()
    jar = args.jar or find_jar()
    api = re.search(r"android-([\d.]+)", jar)
    source = f"android-{api.group(1)}" if api else os.path.basename(jar)
    print(f"读取 {jar}")

    strings = parse_resources(jar)
    perms = parse_permissions(jar)
    names = icu_names()

    # ---- core：保护级别表 + 自称表 ----
    lines = [
        "// @generated by scripts/gen-android-data.py —— 不要手改，重新运行脚本生成。",
        f"// 数据来源：{source} 的 android.jar（权限）、ICU（语言自称）。",
        "",
        f"/// 框架权限（按名字排序，可二分查找）→ {LEVEL_NOTE}。",
        "pub(super) static PERMISSIONS: &[(&str, u32)] = &[",
    ]
    lines += [f"    ({rust_str(p['name'])}, 0x{p.get('level', 0):x})," for p in perms]
    lines += ["];", "", "/// 语言代码（按代码排序）→ 该语言对自己的称呼。",
              "pub(super) static AUTONYMS: &[(&str, &str)] = &["]
    lines += [f"    ({rust_str(c)}, {rust_str(n)})," for c, n in sorted(names["autonyms"].items())]
    lines += ["];", ""]
    os.makedirs(os.path.dirname(CORE_OUT), exist_ok=True)
    with open(CORE_OUT, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))
    print(f"写出 {CORE_OUT}：{len(perms)} 条权限，{len(names['autonyms'])} 条自称")

    # ---- app：各界面语言的附加译文层 ----
    os.makedirs(I18N_DIR, exist_ok=True)
    for ui, (qual, icu) in UI_LANGS.items():
        def pick(res_id):
            vals = strings.get(res_id, {})
            return vals.get(qual) or vals.get("")

        out = [
            "# @generated by scripts/gen-android-data.py —— 不要手改，重新运行脚本生成。",
            f"# 权限文案来自 {source} 的 android.jar；语言 / 地区 / 文字名称来自 ICU。",
            "",
            "[meta]",
            f'locale = "{ui}"',
            "",
            "[perm_label]",
        ]
        descs = []
        for p in perms:
            if not p["name"].startswith(PERM_PREFIX):
                continue
            key = p["name"][len(PERM_PREFIX):]
            label = pick(p["label"]) if "label" in p else None
            desc = pick(p["description"]) if "description" in p else None
            if label:
                out.append(f"{key} = {toml_str(label)}")
            if desc:
                descs.append(f"{key} = {toml_str(desc)}")
        out += ["", "[perm_desc]", *descs]
        table = names["langs"][icu]
        english = names["langs"]["en"]
        for section, key in (("lang_name", "lang"), ("region_name", "region"),
                             ("script_name", "script")):
            # ICU 对冷门代码不一定每种语言都有译名：缺的填英文，各语言 key 集合保持一致
            merged = {**english[key], **table[key]}
            out += ["", f"[{section}]"]
            out += [f'"{c}" = {toml_str(n)}' for c, n in sorted(merged.items())]
        path = os.path.join(I18N_DIR, f"{ui}.toml")
        with open(path, "w", encoding="utf-8") as f:
            f.write("\n".join(out) + "\n")
        print(f"写出 {path}")


if __name__ == "__main__":
    main()
