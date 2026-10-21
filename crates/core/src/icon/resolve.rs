//! 资源引用解析与 zip 文件访问。移植自 `adaptive_icon_renderer.dart` 的
//! `_resolveResourceReference` / `_resolveEntryValue` / `_findFileByResourceName` /
//! `_resolveColorRef` / `_resolveVectorFill` / `_readZipFile` 等。

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;

use crate::axml::XmlElement;
use crate::model::ResourceInfo;

use super::color::{self, is_android_int_literal, is_color_literal};
use super::svg::{gradient_from_element, Paint, PaintResolver};
use super::xml::{self, attr, find_child, tag, text};
use super::{decode_bitmap, IconSource, RgbaImage};

/// 引用链最大深度，防止资源互相引用导致死循环。
pub(crate) const MAX_DEPTH: u32 = 16;

static RESOURCE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@(?:res/)?(0x[0-9a-fA-F]+)$").expect("static regex"));
static NAMED_RESOURCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^@(?:\+)?(?:[\w.]+:)?([a-zA-Z_][\w-]*)/([a-zA-Z0-9_.$-]+)$").expect("static regex")
});

/// 常用框架颜色（`@android:color/*`）。资源表里不含框架资源，只能硬编码兜底。
const FRAMEWORK_COLOR_FALLBACK: &[(u32, u32)] = &[
    (0x0106000c, 0xff000000), // android:color/black
    (0x0106000d, 0xffffffff), // android:color/white
    (0x01060000, 0x00000000), // android:color/transparent
    (0x0106000e, 0xff444444), // android:color/darker_gray
    (0x01060010, 0xffaaaaaa), // android:color/lighter_gray
    (0x01060012, 0xff0000ff), // android:color/holo_blue_bright
    (0x01060013, 0xff0099cc), // android:color/holo_blue_dark
    (0x01060014, 0xff33b5e5), // android:color/holo_blue_light
    (0x01060015, 0xffff8800), // android:color/holo_orange_dark
    (0x01060016, 0xffffbb33), // android:color/holo_orange_light
    (0x01060017, 0xff669900), // android:color/holo_green_dark
    (0x01060018, 0xff99cc00), // android:color/holo_green_light
    (0x01060019, 0xffcc0000), // android:color/holo_red_dark
    (0x0106001a, 0xffff4444), // android:color/holo_red_light
    (0x0106001b, 0xff9933cc), // android:color/holo_purple
    // 以上照抄 Dart 版。注意 Dart 表与 AOSP 公开 ID 并不一致（android.R.color.white 实为
    // 0x0106000b，0x0106000d 实为 transparent）；为不改变既有行为只追加缺失的 white。
    (0x0106000b, 0xffffffff), // android:color/white（AOSP 实际 ID）
];

/// 规范化 zip 内路径：去引号、`file://`、反斜杠与前导 `/`。
pub(crate) fn normalize_zip_path(value: &str) -> String {
    let mut s = value.trim().replace('\\', "/");
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = s[1..s.len() - 1].to_string();
    }
    if let Some(rest) = s.strip_prefix("file://") {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_prefix('/') {
        s = rest.to_string();
    }
    s
}

pub(crate) fn is_bitmap_path(path: &str) -> bool {
    let l = path.to_ascii_lowercase();
    l.ends_with(".png") || l.ends_with(".webp") || l.ends_with(".jpg") || l.ends_with(".jpeg")
}

pub(crate) fn is_xml_path(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".xml")
}

pub(crate) fn looks_like_zip_path(value: &str) -> bool {
    let l = value.to_ascii_lowercase();
    l.starts_with("res/") || l.ends_with(".xml") || is_bitmap_path(&l)
}

/// 路径中 `-<density>` 后紧跟 `-`、`/` 或结尾时返回 density。
/// Dart 版正则 `-(…dpi)(?:-|$)` 不认 `/`，导致 `mipmap-xxhdpi/` 这类最常见的目录
/// 拿不到密度分；这里补上 `/` 以便真正按密度选择。
fn density_of(lower: &str) -> Option<&'static str> {
    const DENSITIES: [&str; 6] = ["xxxhdpi", "xxhdpi", "xhdpi", "hdpi", "mdpi", "ldpi"];
    for (i, _) in lower.match_indices('-') {
        let rest = &lower[i + 1..];
        for d in DENSITIES {
            if let Some(after) = rest.strip_prefix(d) {
                if after.is_empty() || after.starts_with('-') || after.starts_with('/') {
                    return Some(d);
                }
            }
        }
    }
    None
}

/// 文件路径优先级分：XML（可能是 adaptive-icon）> 位图 > 无扩展名，再按 anydpi 与密度加分。
pub(crate) fn file_score(path: &str) -> i32 {
    let lower = path.to_ascii_lowercase();
    let mut score = 0;
    let has_ext = match (lower.rfind('.'), lower.rfind('/')) {
        (Some(d), Some(s)) => s < d,
        (Some(_), None) => true,
        _ => false,
    };
    let is_xml = lower.ends_with(".xml");
    if is_xml {
        score += 420;
    } else if is_bitmap_path(&lower) {
        score += 180;
    } else if !has_ext {
        score += 160;
    }
    if lower.contains("anydpi-v26") {
        score += if is_xml { 20 } else { 160 };
    } else if lower.contains("anydpi") {
        score += if is_xml { 15 } else { 120 };
    }
    score += match density_of(&lower) {
        Some("xxxhdpi") => 70,
        Some("xxhdpi") => 65,
        Some("xhdpi") => 60,
        Some("hdpi") => 55,
        Some("mdpi") => 50,
        Some("ldpi") => 45,
        _ => 0,
    };
    score
}

/// 取分数最高的路径（同分保持原顺序中靠前者，结果确定）。
pub(crate) fn pick_best_file_path(paths: &[String]) -> Option<String> {
    let mut best: Option<&String> = None;
    for p in paths {
        if best.is_none_or(|b| file_score(p) > file_score(b)) {
            best = Some(p);
        }
    }
    best.cloned()
}

/// 按分数降序稳定排序。
pub(crate) fn sort_by_file_score(paths: &mut [String]) {
    paths.sort_by_key(|p| std::cmp::Reverse(file_score(p)));
}

fn res_ref(id: u32) -> String {
    format!("@res/0x{id:08x}")
}

#[derive(Debug, Clone, Default)]
struct Entry {
    /// 小写 `type/key`
    name: String,
    file_paths: Vec<String>,
    colors: Vec<String>,
    references: Vec<String>,
}

/// 资源表索引（对应 Dart `_ResourceTable`）。
#[derive(Debug, Default)]
struct ResTable {
    by_id: HashMap<u32, Entry>,
    by_name: HashMap<String, Vec<u32>>,
}

impl ResTable {
    fn from_resources(resources: &BTreeMap<u32, ResourceInfo>) -> Self {
        let mut t = ResTable::default();
        for info in resources.values() {
            let name = info.name().to_ascii_lowercase();
            let mut e = Entry {
                name: name.clone(),
                ..Default::default()
            };
            for p in &info.file_paths {
                let n = normalize_zip_path(p);
                if !n.is_empty() && !e.file_paths.contains(&n) {
                    e.file_paths.push(n);
                }
            }
            for c in &info.colors {
                if !e.colors.contains(c) {
                    e.colors.push(c.clone());
                }
            }
            for r in &info.references {
                let s = res_ref(*r);
                if !e.references.contains(&s) {
                    e.references.push(s);
                }
            }
            t.by_id.insert(info.resource_id, e);
            t.by_name.entry(name).or_default().push(info.resource_id);
        }
        t
    }

    fn entry_score(e: &Entry) -> i32 {
        if let Some(p) = pick_best_file_path(&e.file_paths) {
            return file_score(&p);
        }
        if !e.colors.is_empty() {
            return 10;
        }
        if !e.references.is_empty() {
            return 5;
        }
        0
    }
}

/// 解析器：持有文件访问、资源表与缓存。
pub(crate) struct Resolver<'a> {
    source: &'a mut dyn IconSource,
    table: ResTable,
    zip_files: Option<Rc<Vec<String>>>,
    lower_map: Option<HashMap<String, String>>,
    xml_cache: HashMap<String, Option<Rc<XmlElement>>>,
    bitmap_cache: HashMap<String, Option<Rc<RgbaImage>>>,
}

impl<'a> Resolver<'a> {
    pub fn new(source: &'a mut dyn IconSource, resources: &BTreeMap<u32, ResourceInfo>) -> Self {
        Self {
            source,
            table: ResTable::from_resources(resources),
            zip_files: None,
            lower_map: None,
            xml_cache: HashMap::new(),
            bitmap_cache: HashMap::new(),
        }
    }

    pub fn zip_files(&mut self) -> Rc<Vec<String>> {
        if self.zip_files.is_none() {
            self.zip_files = Some(Rc::new(self.source.file_names()));
        }
        self.zip_files.clone().unwrap_or_default()
    }

    /// 读取 zip 条目：依次尝试原路径、去前导 `/`、补 / 去 `res/` 前缀，最后大小写不敏感匹配。
    pub fn read_file(&mut self, path: &str) -> Option<Vec<u8>> {
        let n = normalize_zip_path(path);
        if n.is_empty() {
            return None;
        }
        let alt = match n.strip_prefix("res/") {
            Some(rest) => rest.to_string(),
            None => format!("res/{n}"),
        };
        for cand in [n.as_str(), alt.as_str()] {
            if cand.is_empty() {
                continue;
            }
            if let Some(d) = self.source.read(cand).filter(|d| !d.is_empty()) {
                return Some(d);
            }
        }
        if self.lower_map.is_none() {
            let files = self.zip_files();
            let mut m = HashMap::new();
            for f in files.iter() {
                // 保留第一个，避免同名不同大小写互相覆盖
                m.entry(f.to_ascii_lowercase()).or_insert_with(|| f.clone());
            }
            self.lower_map = Some(m);
        }
        let actual = self
            .lower_map
            .as_ref()?
            .get(&n.to_ascii_lowercase())?
            .clone();
        self.source.read(&actual)
    }

    /// 加载并缓存 XML 顶层元素。
    pub fn load_xml(&mut self, path: &str) -> Option<Rc<XmlElement>> {
        let key = normalize_zip_path(path);
        if let Some(v) = self.xml_cache.get(&key) {
            return v.clone();
        }
        let v = self
            .read_file(&key)
            .and_then(|b| xml::parse_root(&b))
            .map(Rc::new);
        if v.is_none() {
            log::debug!("icon: load xml failed: {key}");
        }
        self.xml_cache.insert(key, v.clone());
        v
    }

    /// 解码并缓存位图。
    pub fn load_bitmap(&mut self, path: &str) -> Option<Rc<RgbaImage>> {
        let key = normalize_zip_path(path);
        if let Some(v) = self.bitmap_cache.get(&key) {
            return v.clone();
        }
        let v = self
            .read_file(&key)
            .and_then(|b| decode_bitmap(&b))
            .map(Rc::new);
        if v.is_none() {
            log::debug!("icon: decode bitmap failed: {key}");
        }
        self.bitmap_cache.insert(key, v.clone());
        v
    }

    /// 资源引用 → 值（文件路径 / `#aarrggbb` / 另一个 `@res/...` 引用）。
    pub fn resolve_reference(&mut self, reference: &str, depth: u32) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        let t = reference.trim();
        if t.is_empty() {
            return None;
        }
        if is_color_literal(t) || looks_like_zip_path(t) {
            return Some(t.to_string());
        }
        if let Some(m) = RESOURCE_ID.captures(t) {
            let id = u32::from_str_radix(&m[1][2..], 16).ok()?;
            if self.table.by_id.contains_key(&id) {
                if let Some(v) = self.resolve_entry_value(id) {
                    return Some(v);
                }
            }
            return framework_fallback(id).map(color::format_argb);
        }
        if let Some(m) = NAMED_RESOURCE.captures(t) {
            let key = format!(
                "{}/{}",
                m[1].to_ascii_lowercase(),
                m[2].to_ascii_lowercase()
            );
            let ids = self.table.by_name.get(&key)?;
            let best = ids.iter().copied().max_by_key(|id| {
                // 同分时取 id 最小者，保证结果确定
                let score = self
                    .table
                    .by_id
                    .get(id)
                    .map(ResTable::entry_score)
                    .unwrap_or(0);
                (score, std::cmp::Reverse(*id))
            })?;
            return self.resolve_entry_value(best);
        }
        None
    }

    fn resolve_entry_value(&mut self, id: u32) -> Option<String> {
        let e = self.table.by_id.get(&id)?;
        if !e.file_paths.is_empty() {
            let paths = e.file_paths.clone();
            return self.pick_best_file(&paths);
        }
        if let Some(c) = e.colors.first() {
            return Some(c.clone());
        }
        if let Some(r) = e.references.first() {
            return Some(r.clone());
        }
        // 条目有名字但无数据（混淆 / 拆分包）：按资源名在 zip 的 res/ 下兜底查找
        let name = e.name.clone();
        if name.is_empty() {
            return None;
        }
        self.find_file_by_resource_name(&name)
    }

    /// 按 `file_score` 选文件；最高分并列且都不是 XML 时，取像素最多者。
    /// 混淆 / 资源压缩后的 APK 把各密度变体改名为 `res/xx.png`，路径里没有密度信息，
    /// 仅靠路径会随机选到低密度版本（Dart 版就是这样，图标发糊）。
    fn pick_best_file(&mut self, paths: &[String]) -> Option<String> {
        let top = paths.iter().map(|p| file_score(p)).max()?;
        let mut tied: Vec<&String> = Vec::new();
        for p in paths.iter().filter(|p| file_score(p) == top) {
            if !tied.contains(&p) {
                tied.push(p);
            }
        }
        // 混淆包里还可能连扩展名都没有（`res/Qt`），只要不是 XML 就按位图比较
        if tied.len() < 2 || tied.iter().any(|p| is_xml_path(p)) {
            return tied.first().map(|s| s.to_string());
        }
        let mut best: Option<(u64, String)> = None;
        for p in tied {
            let area = self.bitmap_area(p);
            if best.as_ref().is_none_or(|(a, _)| area > *a) {
                best = Some((area, p.clone()));
            }
        }
        best.map(|(_, p)| p)
    }

    /// 位图像素数（只读头部，不解码）；读不到为 0。
    fn bitmap_area(&mut self, path: &str) -> u64 {
        let Some(bytes) = self.read_file(path) else {
            return 0;
        };
        image::ImageReader::new(std::io::Cursor::new(&bytes))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok())
            .map(|(w, h)| w as u64 * h as u64)
            .unwrap_or(0)
    }

    /// `type/name` → zip 内同名文件；mipmap 与 drawable 互通，类型目录匹配优先。
    fn find_file_by_resource_name(&mut self, resource_name: &str) -> Option<String> {
        let (ty, name) = resource_name.split_once('/')?;
        if name.contains('/') {
            return None;
        }
        let (ty, name) = (ty.to_ascii_lowercase(), name.to_ascii_lowercase());
        let mut related = vec![ty.clone()];
        match ty.as_str() {
            "mipmap" => related.push("drawable".into()),
            "drawable" => related.push("mipmap".into()),
            _ => {}
        }
        let files = self.zip_files();
        let mut exact = Vec::new();
        let mut broad = Vec::new();
        for f in files.iter() {
            let lower = f.to_ascii_lowercase();
            let Some(after_res) = lower.strip_prefix("res/") else {
                continue;
            };
            let Some(slash) = lower.rfind('/') else {
                continue;
            };
            let file_name = &lower[slash + 1..];
            let base = match file_name.rfind('.') {
                Some(d) if d > 0 => &file_name[..d],
                _ => file_name,
            };
            if base != name {
                continue;
            }
            if related.iter().any(|t| {
                after_res.starts_with(&format!("{t}-")) || after_res.starts_with(&format!("{t}/"))
            }) {
                exact.push(f.clone());
            } else {
                broad.push(f.clone());
            }
        }
        let selected = if exact.is_empty() { broad } else { exact };
        self.pick_best_file(&selected)
    }

    /// 值 → 颜色：字面量直接解析，引用则递归解析（XML 颜色 / selector / shape）。
    pub fn resolve_color_ref(&mut self, value: Option<&str>, depth: u32) -> Option<u32> {
        let t = value?.trim();
        if t.is_empty() {
            return None;
        }
        // 字面量不受深度限制
        if let Some(c) = color::parse_any_color_literal(t) {
            return Some(c);
        }
        if depth > MAX_DEPTH || !t.starts_with('@') {
            return None;
        }
        let resolved = self.resolve_reference(t, depth + 1)?;
        if resolved == t {
            return None;
        }
        if is_xml_path(&resolved) {
            if let Some(c) = self.resolve_color_from_xml(&resolved, depth + 1) {
                return Some(c);
            }
        }
        self.resolve_color_ref(Some(&resolved), depth + 1)
    }

    /// XML 资源中的颜色：`<selector>` 第一个 item、`<color>` 文本、`<shape><solid>`。
    pub fn resolve_color_from_xml(&mut self, path: &str, depth: u32) -> Option<u32> {
        if depth > MAX_DEPTH {
            return None;
        }
        let root = self.load_xml(path)?;
        match tag(&root).as_str() {
            "selector" => {
                let c = find_child(&root, "item")
                    .and_then(|i| attr(i, "color"))?
                    .to_string();
                self.resolve_color_ref(Some(&c), depth + 1)
            }
            "color" => text(&root).and_then(color::parse_color),
            "shape" => {
                let c = find_child(&root, "solid")
                    .and_then(|s| attr(s, "color"))?
                    .to_string();
                self.resolve_color_ref(Some(&c), depth + 1)
            }
            _ => None,
        }
    }

    /// 容器内联子元素的颜色（`<color>`、`<shape><solid>`），用于 adaptive 导出背景。
    pub fn resolve_container_color(&mut self, container: &XmlElement, depth: u32) -> Option<u32> {
        if depth > MAX_DEPTH {
            return None;
        }
        for child in &container.children {
            match tag(child).as_str() {
                "color" => {
                    if let Some(c) = attr(child, "color") {
                        return self.resolve_color_ref(Some(c), depth + 1);
                    }
                    if let Some(t) = text(child) {
                        return self.resolve_color_ref(Some(t), depth + 1);
                    }
                }
                "shape" => {
                    if let Some(c) = find_child(child, "solid").and_then(|s| attr(s, "color")) {
                        return self.resolve_color_ref(Some(c), depth + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// path 的 fillColor：颜色字面量 / 引用到颜色 / 引用到 `<gradient>` XML。
    pub fn resolve_vector_fill(&mut self, raw: &str, depth: u32) -> Option<Paint> {
        let t = raw.trim();
        if t.is_empty() {
            return None;
        }
        if let Some(c) = color::parse_any_color_literal(t) {
            return Some(Paint::Color(c));
        }
        if depth > MAX_DEPTH || !t.starts_with('@') {
            return None;
        }
        let resolved = self.resolve_reference(t, depth + 1)?;
        if resolved == t {
            return None;
        }
        if is_xml_path(&resolved) {
            if let Some(g) = self.resolve_gradient_xml(&resolved, depth + 1) {
                return Some(g);
            }
            if let Some(c) = self.resolve_color_from_xml(&resolved, depth + 1) {
                return Some(Paint::Color(c));
            }
        }
        self.resolve_color_ref(Some(&resolved), depth + 1)
            .map(Paint::Color)
    }

    fn resolve_gradient_xml(&mut self, path: &str, depth: u32) -> Option<Paint> {
        let root = self.load_xml(path)?;
        if tag(&root) != "gradient" {
            return None;
        }
        gradient_from_element(&root, &mut |s| self.resolve_color_ref(Some(s), depth + 1))
            .map(Paint::Gradient)
    }

    /// drawable 值 → zip 路径（或颜色字面量），供判断前景类型。
    pub fn resolve_drawable_to_path(&mut self, value: &str, depth: u32) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        let t = value.trim();
        if t.is_empty() {
            return None;
        }
        if looks_like_zip_path(t) {
            return Some(normalize_zip_path(t));
        }
        if !t.starts_with('@') {
            return None;
        }
        self.resolve_reference(t, depth + 1)
    }
}

impl PaintResolver for Resolver<'_> {
    fn paint(&mut self, raw: &str) -> Option<Paint> {
        self.resolve_vector_fill(raw, 0)
    }
    fn color(&mut self, raw: &str) -> Option<u32> {
        self.resolve_color_ref(Some(raw), 0)
    }
}

fn framework_fallback(id: u32) -> Option<u32> {
    if id >> 24 != 0x01 {
        return None;
    }
    FRAMEWORK_COLOR_FALLBACK
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, v)| *v)
}

/// `_findDrawableLikeValue`：容器上第一个像 drawable 的属性值（引用 / 路径 / 颜色），否则看文本。
pub(crate) fn find_drawable_like_value(e: &XmlElement) -> Option<String> {
    let like = |v: &str| {
        v.starts_with('@')
            || looks_like_zip_path(v)
            || is_color_literal(v)
            || is_android_int_literal(v)
    };
    for (_, v) in xml::real_attrs(e) {
        let v = v.trim();
        if !v.is_empty() && like(v) {
            return Some(v.to_string());
        }
    }
    text(e)
        .map(str::trim)
        .filter(|t| !t.is_empty() && like(t))
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 内存版 IconSource，测试共用。
    #[derive(Default)]
    pub(crate) struct MemSource {
        pub files: BTreeMap<String, Vec<u8>>,
    }

    impl MemSource {
        pub fn with(files: &[(&str, &[u8])]) -> Self {
            Self {
                files: files
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_vec()))
                    .collect(),
            }
        }
    }

    impl IconSource for MemSource {
        fn read(&mut self, path: &str) -> Option<Vec<u8>> {
            self.files.get(path).cloned()
        }
        fn file_names(&mut self) -> Vec<String> {
            self.files.keys().cloned().collect()
        }
    }

    pub(crate) fn res(
        id: u32,
        ty: &str,
        key: &str,
        files: &[&str],
        colors: &[&str],
        refs: &[u32],
    ) -> ResourceInfo {
        ResourceInfo {
            resource_id: id,
            type_name: ty.into(),
            key_name: key.into(),
            file_paths: files.iter().map(|s| s.to_string()).collect(),
            colors: colors.iter().map(|s| s.to_string()).collect(),
            references: refs.to_vec(),
        }
    }

    #[test]
    fn file_score_order() {
        let mut v: Vec<String> = [
            "res/mipmap-mdpi/ic.png",
            "res/mipmap-xxxhdpi/ic.png",
            "res/mipmap-anydpi-v26/ic.xml",
            "res/mipmap-xhdpi-v4/ic.webp",
            "res/raw/ic",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        sort_by_file_score(&mut v);
        assert_eq!(v[0], "res/mipmap-anydpi-v26/ic.xml");
        assert_eq!(v[1], "res/mipmap-xxxhdpi/ic.png");
        assert_eq!(v[2], "res/mipmap-xhdpi-v4/ic.webp");
        assert_eq!(v[3], "res/mipmap-mdpi/ic.png");
        assert_eq!(v[4], "res/raw/ic");
    }

    #[test]
    fn resolves_ids_names_colors_and_chains() {
        let mut map = BTreeMap::new();
        map.insert(
            0x7f010001,
            res(0x7f010001, "color", "bg", &[], &["#ff112233"], &[]),
        );
        map.insert(
            0x7f010002,
            res(0x7f010002, "color", "alias", &[], &[], &[0x7f010001]),
        );
        map.insert(
            0x7f020001,
            res(
                0x7f020001,
                "mipmap",
                "ic_launcher",
                &[
                    "res/mipmap-mdpi/ic_launcher.png",
                    "res/mipmap-anydpi-v26/ic_launcher.xml",
                ],
                &[],
                &[],
            ),
        );
        map.insert(
            0x7f020002,
            res(0x7f020002, "drawable", "ic_fg", &[], &[], &[]),
        );
        let mut src = MemSource::with(&[
            ("res/drawable-xxhdpi/ic_fg.png", b"x"),
            ("res/mipmap-mdpi/ic_fg.png", b"x"),
        ]);
        let mut r = Resolver::new(&mut src, &map);
        assert_eq!(
            r.resolve_reference("@res/0x7f010001", 0).as_deref(),
            Some("#ff112233")
        );
        assert_eq!(
            r.resolve_reference("@0x7f020001", 0).as_deref(),
            Some("res/mipmap-anydpi-v26/ic_launcher.xml")
        );
        assert_eq!(
            r.resolve_reference("@mipmap/ic_launcher", 0).as_deref(),
            Some("res/mipmap-anydpi-v26/ic_launcher.xml")
        );
        assert_eq!(
            r.resolve_color_ref(Some("@res/0x7f010002"), 0),
            Some(0xff112233)
        );
        // 空条目按名字在 zip 中兜底，类型互通且按密度选
        assert_eq!(
            r.resolve_reference("@res/0x7f020002", 0).as_deref(),
            Some("res/drawable-xxhdpi/ic_fg.png")
        );
        // 框架颜色兜底
        assert_eq!(
            r.resolve_color_ref(Some("@res/0x0106000d"), 0),
            Some(0xffffffff)
        );
        assert_eq!(r.resolve_color_ref(Some("@res/0x7f999999"), 0), None);
    }

    #[test]
    fn color_from_xml_resources() {
        let mut map = BTreeMap::new();
        map.insert(
            0x7f010001,
            res(0x7f010001, "color", "sel", &["res/color/sel.xml"], &[], &[]),
        );
        map.insert(
            0x7f010002,
            res(
                0x7f010002,
                "drawable",
                "g",
                &["res/drawable/g.xml"],
                &[],
                &[],
            ),
        );
        let sel = br##"<selector xmlns:android="http://schemas.android.com/apk/res/android"><item android:color="#ff00ff00"/></selector>"##;
        let grad = br##"<gradient xmlns:android="http://schemas.android.com/apk/res/android" android:type="radial" android:centerX="5" android:centerY="5" android:gradientRadius="5"><item android:color="#ff000000" android:offset="0"/><item android:color="@res/0x7f010001" android:offset="1"/></gradient>"##;
        let mut src = MemSource::with(&[("res/color/sel.xml", sel), ("res/drawable/g.xml", grad)]);
        let mut r = Resolver::new(&mut src, &map);
        assert_eq!(
            r.resolve_color_ref(Some("@res/0x7f010001"), 0),
            Some(0xff00ff00)
        );
        match r.resolve_vector_fill("@res/0x7f010002", 0) {
            Some(Paint::Gradient(g)) => {
                assert_eq!(g.colors, vec![0xff000000, 0xff00ff00]);
                assert_eq!(g.radius, Some(5.0));
            }
            other => panic!("expected gradient, got {other:?}"),
        }
    }

    #[test]
    fn obfuscated_bitmap_tie_prefers_largest() {
        let png = |w: u32| {
            let mut v = Vec::new();
            image::RgbaImage::new(w, w)
                .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png)
                .expect("png");
            v
        };
        let (small, big) = (png(4), png(16));
        let mut map = BTreeMap::new();
        map.insert(
            0x7f020001,
            res(
                0x7f020001,
                "drawable",
                "fg",
                &["res/aa.png", "res/zz.png"],
                &[],
                &[],
            ),
        );
        let mut src = MemSource::with(&[("res/aa.png", &small), ("res/zz.png", &big)]);
        let mut r = Resolver::new(&mut src, &map);
        assert_eq!(
            r.resolve_reference("@res/0x7f020001", 0).as_deref(),
            Some("res/zz.png")
        );
    }

    #[test]
    fn read_file_fallbacks() {
        let mut src = MemSource::with(&[("res/Drawable/A.png", b"1"), ("res/x.xml", b"2")]);
        let map = BTreeMap::new();
        let mut r = Resolver::new(&mut src, &map);
        assert_eq!(
            r.read_file("/res/drawable/a.png").as_deref(),
            Some(&b"1"[..])
        );
        assert_eq!(r.read_file("x.xml").as_deref(), Some(&b"2"[..]));
        assert!(r.read_file("missing.png").is_none());
    }
}
