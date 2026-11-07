//! 导出原始文件：二进制 XML 反编译成可读文本，并收集它引用到的其它文件
//! （adaptive-icon 的前景 / 背景、inset 包着的位图、颜色 selector 等）。
//!
//! AXML / proto XML 解析后不保留命名空间，属性统一按 `android:` 前缀输出
//! （`aapt:attr` 的 `name` 与已带前缀 / 未能还原名称的属性除外），这与 drawable 的实际写法一致。

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;

use crate::axml::XmlElement;

use super::resolve::{is_xml_path, looks_like_zip_path, normalize_zip_path, Resolver, MAX_DEPTH};
use super::svg::esc;
use super::xml::{real_attrs, tag, text};

/// 一次导出最多带上的文件数：真实图标用不到两位数，防止异常资源表把整个包拖进来。
const MAX_FILES: usize = 64;

/// 导出的一个文件：zip 内原路径 + 内容（XML 已反编译为文本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// 从 `path` 出发、沿 XML 引用收集到的全部文件路径（第一个是 `path` 本身）。
pub(crate) fn original_paths(res: &mut Resolver, path: &str) -> Vec<String> {
    let start = normalize_zip_path(path);
    let mut seen = HashSet::from([start.clone()]);
    let mut queue = VecDeque::from([start]);
    let mut out = Vec::new();
    while let Some(p) = queue.pop_front() {
        if out.len() >= MAX_FILES {
            break;
        }
        if is_xml_path(&p) {
            if let Some(root) = res.load_xml(&p) {
                let mut refs = Vec::new();
                collect_refs(res, &root, 0, &mut refs);
                for r in refs {
                    if seen.insert(r.clone()) {
                        queue.push_back(r);
                    }
                }
            }
        }
        out.push(p);
    }
    out
}

/// [`original_paths`] 的文件内容；XML 反编译为文本，读不到的文件跳过。
pub(crate) fn original_files(res: &mut Resolver, path: &str) -> Vec<OriginalFile> {
    original_paths(res, path)
        .into_iter()
        .filter_map(|p| {
            let bytes = match res.load_xml(&p).filter(|_| is_xml_path(&p)) {
                Some(root) => to_text_xml(res, &root).into_bytes(),
                None => res.read_file(&p)?,
            };
            Some(OriginalFile { path: p, bytes })
        })
        .collect()
}

/// 多个文件打成 zip，条目沿用 APK 内的路径。
pub fn zip_files(files: &[OriginalFile]) -> Result<Vec<u8>, String> {
    use std::io::Write as _;
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut w = zip::ZipWriter::new(&mut buf);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for f in files {
        w.start_file(f.path.as_str(), opts)
            .map_err(|e| e.to_string())?;
        w.write_all(&f.bytes).map_err(|e| e.to_string())?;
    }
    w.finish().map_err(|e| e.to_string())?;
    Ok(buf.into_inner())
}

/// 元素树里所有属性值 / 文本引用到的文件。
fn collect_refs(res: &Resolver, e: &XmlElement, depth: u32, out: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let values = real_attrs(e).map(|(_, v)| v).chain(text(e));
    for v in values {
        let v = v.trim();
        if let Some(id) = ref_id(v) {
            out.extend(res.resource_files(id, 0));
        } else if v.starts_with("res/") && looks_like_zip_path(v) {
            out.push(normalize_zip_path(v));
        }
    }
    for child in &e.children {
        collect_refs(res, child, depth + 1, out);
    }
}

/// `@res/0x…` / `@dyn/0x…` → 资源 ID（`@attr` 是主题属性，不指向文件）。
fn ref_id(v: &str) -> Option<u32> {
    let hex = v
        .strip_prefix("@res/0x")
        .or_else(|| v.strip_prefix("@dyn/0x"))?;
    u32::from_str_radix(hex, 16).ok()
}

/// 元素树 → 文本 XML（4 空格缩进，资源 ID 尽量还原为名称）。
pub(crate) fn to_text_xml(res: &Resolver, root: &XmlElement) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    let uses_aapt = uses_aapt(root, 0);
    write_element(res, root, 0, uses_aapt, &mut out);
    out
}

fn is_aapt_attr(e: &XmlElement) -> bool {
    tag(e) == "attr"
}

fn uses_aapt(e: &XmlElement, depth: u32) -> bool {
    depth <= MAX_DEPTH && (is_aapt_attr(e) || e.children.iter().any(|c| uses_aapt(c, depth + 1)))
}

fn write_element(res: &Resolver, e: &XmlElement, depth: u32, aapt_ns: bool, out: &mut String) {
    if depth > MAX_DEPTH {
        return;
    }
    let indent = "    ".repeat(depth as usize);
    let aapt = is_aapt_attr(e) && !e.name.contains(':');
    let name = if aapt {
        format!("aapt:{}", e.name)
    } else {
        e.name.clone()
    };
    let _ = write!(out, "{indent}<{name}");
    let attr_indent = format!("\n{indent}    ");
    if depth == 0 {
        let _ = write!(
            out,
            "{attr_indent}xmlns:android=\"http://schemas.android.com/apk/res/android\""
        );
        if aapt_ns {
            let _ = write!(
                out,
                "{attr_indent}xmlns:aapt=\"http://schemas.android.com/aapt\""
            );
        }
    }
    let attrs: Vec<_> = real_attrs(e).collect();
    let sep = if depth == 0 || attrs.len() > 1 {
        attr_indent.as_str()
    } else {
        " "
    };
    for (k, v) in attrs {
        let key = if (aapt && k == "name") || k.contains(':') || k.starts_with("attr_") {
            k.to_string()
        } else {
            format!("android:{k}")
        };
        let _ = write!(out, "{sep}{key}=\"{}\"", esc(&display_value(res, v)));
    }
    let body = text(e).map(str::trim).filter(|t| !t.is_empty());
    if e.children.is_empty() && body.is_none() {
        out.push_str(" />\n");
        return;
    }
    out.push('>');
    if let Some(t) = body {
        out.push_str(&esc(t));
    }
    if !e.children.is_empty() {
        out.push('\n');
        for child in &e.children {
            write_element(res, child, depth + 1, aapt_ns, out);
        }
        out.push_str(&indent);
    }
    let _ = writeln!(out, "</{name}>");
}

/// 属性值里的资源 ID 还原为 `@type/name` / `?attr/name`；还原不了时保留十六进制 ID。
fn display_value(res: &Resolver, v: &str) -> String {
    if let Some(id) = ref_id(v) {
        return match res.resource_name(id) {
            Some(name) => format!("@{name}"),
            None => format!("@0x{id:08x}"),
        };
    }
    if let Some(id) = v
        .strip_prefix("@attr/0x")
        .and_then(|h| u32::from_str_radix(h, 16).ok())
    {
        if let Some(name) = crate::axml::framework_attr_name(id) {
            return format!("?android:attr/{name}");
        }
        return match res.resource_name(id) {
            Some(name) => format!("?{name}"),
            None => format!("?0x{id:08x}"),
        };
    }
    short_float(v).unwrap_or_else(|| v.to_string())
}

/// 二进制 XML 里的浮点是 f32，解析时按 f64 展开（`3070.976318359375`）；
/// 文本输出改用 f32 的最短往返写法（`3070.9763`），数值不变、可读得多。
fn short_float(v: &str) -> Option<String> {
    if !v.contains('.') || v.contains(['e', 'E']) {
        return None;
    }
    let wide: f64 = v.parse().ok()?;
    let narrow = wide as f32;
    (f64::from(narrow) == wide).then(|| {
        let s = narrow.to_string();
        if s.contains('.') {
            s
        } else {
            format!("{s}.0")
        }
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::icon::resolve::tests::{res, MemSource};
    use crate::icon::{IconCandidate, IconKind, IconRenderer};

    fn el(name: &str, attrs: &[(&str, &str)], children: Vec<XmlElement>) -> XmlElement {
        XmlElement {
            name: name.into(),
            attributes: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children,
            ..Default::default()
        }
    }

    fn table() -> BTreeMap<u32, crate::model::ResourceInfo> {
        let mut map = BTreeMap::new();
        for (id, ty, key, files) in [
            (
                0x7f080001,
                "drawable",
                "ic_Foreground",
                vec!["res/drawable/fg.xml"],
            ),
            (
                0x7f080002,
                "drawable",
                "art",
                vec!["res/drawable-xxhdpi/art.png", "res/drawable-mdpi/art.png"],
            ),
            (0x7f040000, "attr", "colorPrimary", vec![]),
        ] {
            map.insert(id, res(id, ty, key, &files, &[], &[]));
        }
        map
    }

    #[test]
    fn decompiled_xml_restores_names_and_prefixes() {
        let mut src = MemSource::default();
        let map = table();
        let r = Resolver::new(&mut src, &map);
        let root = el(
            "adaptive-icon",
            &[],
            vec![
                el("background", &[("drawable", "#ff102030")], vec![]),
                el(
                    "foreground",
                    &[
                        ("drawable", "@res/0x7f080001"),
                        ("tint", "@attr/0x7f040000"),
                    ],
                    vec![],
                ),
                el(
                    "monochrome",
                    &[
                        ("drawable", "@res/0x7f0899aa"),
                        ("color", "@attr/0x1010002"),
                    ],
                    vec![],
                ),
            ],
        );
        assert_eq!(
            to_text_xml(&r, &root),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <adaptive-icon\n    xmlns:android=\"http://schemas.android.com/apk/res/android\">\n\
             \x20   <background android:drawable=\"#ff102030\" />\n\
             \x20   <foreground\n        android:drawable=\"@drawable/ic_Foreground\"\n        android:tint=\"?attr/colorPrimary\" />\n\
             \x20   <monochrome\n        android:color=\"?android:attr/icon\"\n        android:drawable=\"@0x7f0899aa\" />\n\
             </adaptive-icon>\n"
        );
    }

    #[test]
    fn decompiled_xml_keeps_aapt_inline_attr() {
        let mut src = MemSource::default();
        let map = BTreeMap::new();
        let r = Resolver::new(&mut src, &map);
        let root = el(
            "vector",
            &[("viewportWidth", "3070.976318359375"), ("alpha", "0.5")],
            vec![el(
                "path",
                &[("pathData", "M0,0h1 & <x>")],
                vec![el(
                    "attr",
                    &[("name", "android:fillColor")],
                    vec![el("gradient", &[("type", "0")], vec![])],
                )],
            )],
        );
        let text = to_text_xml(&r, &root);
        assert!(
            text.contains("xmlns:aapt=\"http://schemas.android.com/aapt\""),
            "{text}"
        );
        assert!(
            text.contains("<aapt:attr name=\"android:fillColor\">"),
            "{text}"
        );
        assert!(text.contains("</aapt:attr>"), "{text}");
        assert!(
            text.contains("android:pathData=\"M0,0h1 &amp; &lt;x&gt;\""),
            "{text}"
        );
        assert!(text.contains("<gradient android:type=\"0\" />"), "{text}");
        assert!(
            text.contains("android:viewportWidth=\"3070.9763\""),
            "{text}"
        );
        assert!(text.contains("android:alpha=\"0.5\""), "{text}");
    }

    const NS: &str = r#"xmlns:android="http://schemas.android.com/apk/res/android""#;

    fn layered_apk() -> (MemSource, BTreeMap<u32, crate::model::ResourceInfo>) {
        let adaptive = format!(
            r#"<adaptive-icon {NS}><background android:drawable="@res/0x7f080003"/><foreground android:drawable="@res/0x7f080001"/></adaptive-icon>"#
        );
        let fg = format!(r#"<inset {NS} android:drawable="@res/0x7f080002" android:inset="10%"/>"#);
        let bg = format!(r#"<vector {NS} android:viewportWidth="1" android:viewportHeight="1"/>"#);
        let src = MemSource::with(&[
            ("res/mipmap-anydpi-v26/ic_launcher.xml", adaptive.as_bytes()),
            ("res/drawable/fg.xml", fg.as_bytes()),
            ("res/drawable/bg.xml", bg.as_bytes()),
            ("res/drawable-xxhdpi/art.png", b"\x89PNG-xx"),
            ("res/drawable-mdpi/art.png", b"\x89PNG-m"),
        ]);
        let mut map = table();
        map.insert(
            0x7f080003,
            res(
                0x7f080003,
                "drawable",
                "bg",
                &["res/drawable/bg.xml"],
                &[],
                &[],
            ),
        );
        (src, map)
    }

    #[test]
    fn adaptive_icon_original_collects_every_layer() {
        let (mut src, map) = layered_apk();
        let mut r = Resolver::new(&mut src, &map);
        assert_eq!(
            original_paths(&mut r, "res/mipmap-anydpi-v26/ic_launcher.xml"),
            vec![
                "res/mipmap-anydpi-v26/ic_launcher.xml",
                "res/drawable/bg.xml",
                "res/drawable/fg.xml",
                "res/drawable-xxhdpi/art.png",
                "res/drawable-mdpi/art.png",
            ]
        );
    }

    #[test]
    fn export_original_zips_layers_and_decompiles_xml() {
        let (mut src, map) = layered_apk();
        let mut renderer = IconRenderer::new(&mut src, &map);
        let mut c = IconCandidate::from_path("res/mipmap-anydpi-v26/ic_launcher.xml", None);
        renderer.annotate(&mut c);
        assert_eq!(c.original_file_count, 5);
        assert!(c.original_is_bundle());
        let zip = renderer.export_original(&c).expect("zip");
        let mut archive = crate::zip::Archive::from_bytes(zip).expect("open zip");
        assert_eq!(archive.file_names().len(), 5);
        let fg = String::from_utf8(archive.read("res/drawable/fg.xml").expect("fg")).unwrap();
        assert!(fg.contains("android:drawable=\"@drawable/art\""), "{fg}");
        assert_eq!(
            archive.read("res/drawable-mdpi/art.png").ok().as_deref(),
            Some(&b"\x89PNG-m"[..])
        );

        // 不引用其它文件的单个 vector：直接导出反编译后的文本
        let mut v = IconCandidate::from_path("res/drawable/bg.xml", None);
        renderer.annotate(&mut v);
        assert!(!v.original_is_bundle());
        let text = String::from_utf8(renderer.export_original(&v).expect("xml")).unwrap();
        assert!(text.starts_with("<?xml"), "{text}");
        assert!(text.contains("android:viewportWidth=\"1\""), "{text}");

        // 位图原样导出
        let b = IconCandidate::from_path("res/drawable-mdpi/art.png", None);
        assert_eq!(b.kind, IconKind::Bitmap);
        assert_eq!(
            renderer.export_original(&b).as_deref(),
            Some(&b"\x89PNG-m"[..])
        );
    }
}
