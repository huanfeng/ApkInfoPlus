//! 图标候选收集。移植自 `apk_info.dart` 的 `IconCandidate` 与 `collectIconCandidates`。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::model::ResourceInfo;

use super::resolve::{is_bitmap_path, is_xml_path, normalize_zip_path, sort_by_file_score};
use super::IconSource;

static DPI_IN_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[/-](nodpi|anydpi(?:-v\d+)?|[a-z]*dpi)(?:[/-])").expect("static regex")
});

/// 候选类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Bitmap,
    /// XML drawable（vector / adaptive-icon / layer-list …）
    XmlVector,
}

/// 一个可渲染的图标候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconCandidate {
    /// zip 内路径；无法定位文件时也可能是原始资源引用（渲染时再解析）。
    pub path: String,
    pub kind: IconKind,
    pub dpi: Option<u32>,
    pub dpi_label: Option<String>,
    /// UI 展示用，如 `XML ic_launcher (anydpi-v26)`、`PNG ic_launcher (xxxhdpi)`。
    pub display_label: String,
}

fn dpi_label_to_value(label: &str) -> Option<u32> {
    match label {
        "ldpi" => Some(120),
        "mdpi" => Some(160),
        "hdpi" => Some(240),
        "xhdpi" => Some(320),
        "xxhdpi" => Some(480),
        "xxxhdpi" => Some(640),
        "tvdpi" => Some(213),
        _ => None,
    }
}

fn dpi_value_to_label(v: u32) -> String {
    match v {
        120 => "ldpi".into(),
        160 => "mdpi".into(),
        240 => "hdpi".into(),
        320 => "xhdpi".into(),
        480 => "xxhdpi".into(),
        640 => "xxxhdpi".into(),
        213 => "tvdpi".into(),
        // 资源表 density 的特殊值：0xfffe = anydpi，0xffff = nodpi（Dart 版显示成 65534dpi）
        0xfffe => "anydpi".into(),
        0xffff => "nodpi".into(),
        _ => format!("{v}dpi"),
    }
}

/// 从路径目录名提取 dpi 标签（如 `mipmap-xxxhdpi-v4/` → `xxxhdpi`）。
pub(crate) fn extract_dpi_label(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    DPI_IN_PATH.captures(&lower).map(|c| c[1].to_string())
}

impl IconCandidate {
    /// 从路径推断类型与 dpi；路径里没有 dpi 时用资源表给出的 `known_dpi`。
    pub fn from_path(path: &str, known_dpi: Option<u32>) -> Self {
        let kind = if path.to_ascii_lowercase().ends_with(".xml") {
            IconKind::XmlVector
        } else {
            IconKind::Bitmap
        };
        let mut dpi_label = extract_dpi_label(path);
        let mut dpi = dpi_label.as_deref().and_then(dpi_label_to_value);
        if dpi_label.is_none() {
            // density 0 是默认配置（无限定符），不是一个真实 dpi
            if let Some(k) = known_dpi.filter(|k| *k != 0) {
                dpi = Some(k);
                dpi_label = Some(dpi_value_to_label(k));
            }
        }
        let display_label = display_label(path, kind, dpi_label.as_deref());
        Self {
            path: path.to_string(),
            kind,
            dpi,
            dpi_label,
            display_label,
        }
    }
}

fn display_label(path: &str, kind: IconKind, dpi_label: Option<&str>) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let (base, ext) = match file.rfind('.') {
        Some(i) => (&file[..i], file[i + 1..].to_ascii_uppercase()),
        None => (file, "?".to_string()),
    };
    let dpi = dpi_label.unwrap_or("?");
    match kind {
        IconKind::XmlVector => format!("XML {base} ({dpi})"),
        IconKind::Bitmap => format!("{ext} {base} ({dpi})"),
    }
}

/// 未知路径的启发式打分（`_unknownPathScore`）。
pub(crate) fn unknown_path_score(candidate: &str) -> i32 {
    let l = candidate.to_ascii_lowercase();
    let mut s = 0;
    if l.contains("mipmap") {
        s += 300;
    }
    if l.contains("drawable") {
        s += 220;
    }
    if l.contains("launcher") {
        s += 180;
    }
    if is_bitmap_path(&l) {
        s += 200;
    }
    if is_xml_path(&l) {
        s += 120;
    }
    // 带 `-` 前缀匹配，各档互斥（`-xxxhdpi` 不含子串 `-xxhdpi`）
    for (k, v) in [
        ("-xxxhdpi", 90),
        ("-xxhdpi", 80),
        ("-xhdpi", 70),
        ("-hdpi", 60),
        ("-mdpi", 50),
    ] {
        if l.contains(k) {
            s += v;
        }
    }
    s
}

fn sort_by_unknown_score(v: &mut [String]) {
    v.sort_by_key(|p| std::cmp::Reverse(unknown_path_score(p)));
}

/// 资源引用 / 不完整路径 → zip 内可能的文件（按文件名匹配，按启发式分排序）。
fn find_unknown_candidate_paths(
    all_files: &[String],
    file_set: &HashSet<&str>,
    candidate: &str,
) -> Vec<String> {
    let raw = candidate.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let normalized = raw.replace('\\', "/");
    let mut result: Vec<String> = Vec::new();
    let push = |s: String, r: &mut Vec<String>| {
        if !r.contains(&s) {
            r.push(s);
        }
    };
    if file_set.contains(normalized.as_str()) {
        push(normalized.clone(), &mut result);
    }
    if let Some(tmp) = normalized.strip_prefix('/') {
        if file_set.contains(tmp) {
            push(tmp.to_string(), &mut result);
        }
    }
    let base = normalized
        .rsplit('/')
        .next()
        .unwrap_or(&normalized)
        .to_ascii_lowercase();
    if !base.is_empty() {
        for f in all_files {
            let fl = f.to_ascii_lowercase();
            let fb = fl.rsplit('/').next().unwrap_or(&fl);
            if fb == base || fl.ends_with(&format!("/{base}")) {
                push(f.clone(), &mut result);
            }
        }
    }
    sort_by_unknown_score(&mut result);
    result
}

/// 资源表中包含该 XML 路径的条目里的位图备选（按 `file_score` 降序）。
/// 用途：adaptive-icon 渲染失败时还能退回到同名资源的位图变体。
pub(crate) fn find_bitmap_alternatives(
    resources: &BTreeMap<u32, ResourceInfo>,
    file_path: &str,
) -> Vec<String> {
    let target = normalize_zip_path(file_path).to_ascii_lowercase();
    if target.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for info in resources.values() {
        if !info
            .file_paths
            .iter()
            .any(|p| normalize_zip_path(p).to_ascii_lowercase() == target)
        {
            continue;
        }
        for p in &info.file_paths {
            let n = normalize_zip_path(p);
            if is_bitmap_path(&n) && !out.contains(&n) {
                out.push(n);
            }
        }
    }
    sort_by_file_score(&mut out);
    out
}

/// 收集图标候选（不渲染）。顺序：
/// 1. 主图标路径 + `icons`（density → path，按 density 降序），去重；
/// 2. 每个 XML 候选后紧跟同一资源的位图备选；
/// 3. 有效性过滤：位图 / XML 须存在于 zip，其它（引用 / 未知）按文件名查找，找不到则原样保留；
/// 4. 仍为空时兜底扫描 mipmap 与 drawable+launcher；
/// 5. 去掉 `_round.` 变体并去重。
pub fn collect_candidates(
    source: &mut dyn IconSource,
    resources: &BTreeMap<u32, ResourceInfo>,
    main_icon_path: Option<&str>,
    icons: &BTreeMap<u32, String>,
) -> Vec<IconCandidate> {
    let main = main_icon_path.filter(|s| !s.is_empty());
    if main.is_none() && icons.is_empty() {
        return Vec::new();
    }
    let all_files = source.file_names();
    let file_set: HashSet<&str> = all_files.iter().map(String::as_str).collect();

    // density 降序；同一路径多个 density 时保留最后写入者（与 Dart 的 map 覆盖语义一致）
    let mut with_dpi: Vec<(u32, &String)> = icons.iter().map(|(k, v)| (*k, v)).collect();
    with_dpi.sort_by_key(|(d, _)| std::cmp::Reverse(*d));
    let mut dpi_map: HashMap<&str, u32> = HashMap::new();
    for (d, p) in &with_dpi {
        dpi_map.insert(p.as_str(), *d);
    }

    let mut unique: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for p in main
        .into_iter()
        .map(str::to_string)
        .chain(with_dpi.iter().map(|(_, p)| (*p).clone()))
    {
        if seen.insert(p.clone()) {
            unique.push(p);
        }
    }

    let mut prioritized: Vec<String> = Vec::new();
    let mut pseen: HashSet<String> = HashSet::new();
    for p in &unique {
        let alts = if is_xml_path(p) {
            find_bitmap_alternatives(resources, p)
        } else {
            Vec::new()
        };
        if pseen.insert(p.clone()) {
            prioritized.push(p.clone());
        }
        for b in alts {
            if pseen.insert(b.clone()) {
                prioritized.push(b);
            }
        }
    }

    let mut valid: Vec<String> = Vec::new();
    for p in &prioritized {
        // Dart 的 `_isBitmapIcon` 只认小写 .png/.webp；.jpg 等走未知路径分支
        if p.ends_with(".webp") || p.ends_with(".png") || is_xml_path(p) {
            if file_set.contains(p.as_str()) {
                valid.push(p.clone());
            }
        } else {
            let resolved = find_unknown_candidate_paths(&all_files, &file_set, p);
            if resolved.is_empty() {
                valid.push(p.clone());
            } else {
                for r in resolved {
                    if pseen.insert(r.clone()) {
                        valid.push(r);
                    }
                }
            }
        }
    }

    if valid.is_empty() {
        let mut fallback: Vec<String> = all_files
            .iter()
            .filter(|e| {
                let l = e.to_ascii_lowercase();
                l.contains("mipmap") || (l.contains("drawable") && l.contains("launcher"))
            })
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        // HashSet 无序：先按路径排序保证结果确定，再按启发式分稳定排序
        fallback.sort();
        sort_by_unknown_score(&mut fallback);
        for c in fallback {
            if pseen.insert(c.clone()) {
                valid.push(c);
            }
        }
    }

    let mut out = Vec::new();
    let mut fseen: HashSet<String> = HashSet::new();
    for p in valid {
        if p.to_ascii_lowercase().contains("_round.") {
            continue;
        }
        if fseen.insert(p.clone()) {
            let known = dpi_map.get(p.as_str()).copied();
            out.push(IconCandidate::from_path(&p, known));
        }
    }
    log::debug!(
        "icon: {} candidates: {:?}",
        out.len(),
        out.iter().map(|c| &c.path).collect::<Vec<_>>()
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon::resolve::tests::{res, MemSource};

    #[test]
    fn dpi_extraction() {
        assert_eq!(
            extract_dpi_label("res/mipmap-xxxhdpi-v4/ic.png").as_deref(),
            Some("xxxhdpi")
        );
        assert_eq!(
            extract_dpi_label("res/mipmap-anydpi-v26/ic.xml").as_deref(),
            Some("anydpi-v26")
        );
        assert_eq!(
            extract_dpi_label("res/drawable-nodpi/x.png").as_deref(),
            Some("nodpi")
        );
        assert_eq!(extract_dpi_label("res/drawable/x.png"), None);
        let c = IconCandidate::from_path("res/mipmap-xhdpi/ic_launcher.webp", None);
        assert_eq!((c.dpi, c.kind), (Some(320), IconKind::Bitmap));
        assert_eq!(c.display_label, "WEBP ic_launcher (xhdpi)");
        let c = IconCandidate::from_path("res/a.png", Some(480));
        assert_eq!((c.dpi, c.dpi_label.as_deref()), (Some(480), Some("xxhdpi")));
        let c = IconCandidate::from_path("res/BW.xml", Some(65534));
        assert_eq!(c.display_label, "XML BW (anydpi)");
        let c = IconCandidate::from_path("res/qr.xml", Some(0));
        assert_eq!((c.dpi, c.display_label.as_str()), (None, "XML qr (?)"));
        let c = IconCandidate::from_path("res/mipmap-anydpi-v26/ic.xml", None);
        assert_eq!(
            (c.dpi, c.display_label.as_str()),
            (None, "XML ic (anydpi-v26)")
        );
    }

    #[test]
    fn unknown_score_order() {
        let mut v: Vec<String> = [
            "res/raw/a.png",
            "res/drawable-hdpi/ic_launcher.png",
            "res/mipmap-xxxhdpi/ic_launcher.png",
            "res/mipmap-mdpi/ic_launcher.xml",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        sort_by_unknown_score(&mut v);
        assert_eq!(v[0], "res/mipmap-xxxhdpi/ic_launcher.png");
        // drawable(220)+launcher+png(200)+hdpi(60)=660 > mipmap(300)+launcher+xml(120)+mdpi(50)=650
        assert_eq!(v[1], "res/drawable-hdpi/ic_launcher.png");
        assert_eq!(v[2], "res/mipmap-mdpi/ic_launcher.xml");
        assert_eq!(v[3], "res/raw/a.png");
    }

    fn src() -> MemSource {
        MemSource::with(&[
            ("res/mipmap-anydpi-v26/ic_launcher.xml", b"<adaptive-icon/>"),
            (
                "res/mipmap-anydpi-v26/ic_launcher_round.xml",
                b"<adaptive-icon/>",
            ),
            ("res/mipmap-mdpi/ic_launcher.png", b"p"),
            ("res/mipmap-xxxhdpi/ic_launcher.png", b"p"),
            ("res/mipmap-xxxhdpi/ic_launcher_round.png", b"p"),
            ("res/drawable/other.png", b"p"),
        ])
    }

    #[test]
    fn collects_in_dart_order() {
        let mut map = BTreeMap::new();
        map.insert(
            0x7f0d0000,
            res(
                0x7f0d0000,
                "mipmap",
                "ic_launcher",
                &[
                    "res/mipmap-mdpi/ic_launcher.png",
                    "res/mipmap-xxxhdpi/ic_launcher.png",
                    "res/mipmap-anydpi-v26/ic_launcher.xml",
                ],
                &[],
                &[],
            ),
        );
        let mut icons = BTreeMap::new();
        icons.insert(160, "res/mipmap-mdpi/ic_launcher.png".to_string());
        icons.insert(65534, "res/mipmap-anydpi-v26/ic_launcher.xml".to_string());
        icons.insert(640, "res/mipmap-xxxhdpi/ic_launcher.png".to_string());
        let mut s = src();
        let c = collect_candidates(
            &mut s,
            &map,
            Some("res/mipmap-anydpi-v26/ic_launcher.xml"),
            &icons,
        );
        let paths: Vec<&str> = c.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "res/mipmap-anydpi-v26/ic_launcher.xml",
                "res/mipmap-xxxhdpi/ic_launcher.png",
                "res/mipmap-mdpi/ic_launcher.png"
            ]
        );
        assert_eq!(c[2].dpi, Some(160));
    }

    #[test]
    fn filters_missing_and_resolves_unknown() {
        let map = BTreeMap::new();
        let mut icons = BTreeMap::new();
        icons.insert(480, "res/mipmap-xxhdpi/missing.png".to_string());
        icons.insert(160, "ic_launcher".to_string());
        let mut s = MemSource::with(&[
            ("res/mipmap-mdpi/ic_launcher", b"p"),
            ("res/x/ic_launcher", b"p"),
        ]);
        let c = collect_candidates(&mut s, &map, None, &icons);
        let paths: Vec<&str> = c.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["res/mipmap-mdpi/ic_launcher", "res/x/ic_launcher"]
        );
    }

    #[test]
    fn unresolvable_reference_kept() {
        let map = BTreeMap::new();
        let mut s = MemSource::with(&[("classes.dex", b"d")]);
        let c = collect_candidates(&mut s, &map, Some("@res/0x7f0d0000"), &BTreeMap::new());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].path, "@res/0x7f0d0000");
    }

    #[test]
    fn fallback_scan_skips_round() {
        let map = BTreeMap::new();
        let mut s = src();
        let c = collect_candidates(
            &mut s,
            &map,
            Some("res/mipmap-xxhdpi/gone.png"),
            &BTreeMap::new(),
        );
        let paths: Vec<&str> = c.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "res/mipmap-xxxhdpi/ic_launcher.png",
                "res/mipmap-mdpi/ic_launcher.png",
                "res/mipmap-anydpi-v26/ic_launcher.xml"
            ]
        );
    }

    #[test]
    fn empty_inputs() {
        let mut s = src();
        assert!(collect_candidates(&mut s, &BTreeMap::new(), None, &BTreeMap::new()).is_empty());
        assert!(
            collect_candidates(&mut s, &BTreeMap::new(), Some(""), &BTreeMap::new()).is_empty()
        );
    }
}
