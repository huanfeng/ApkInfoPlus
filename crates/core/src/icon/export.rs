//! SVG 导出：把 XML 候选解析为 adaptive 分层数据或单个 vector，再交给 [`super::svg`]。
//! 移植自 `resolveAdaptiveIconForSvg` / `resolveToVectorElement` / `_resolveContainerToVector`。

use std::rc::Rc;

use crate::axml::XmlElement;

use super::resolve::{
    find_drawable_like_value, is_xml_path, normalize_zip_path, Resolver, MAX_DEPTH,
};
use super::svg::{self, AdaptiveSvgData, Layer};
use super::xml::{attr, find_child, tag};

/// 容器（background / foreground）→ vector 元素：内联 `<vector>` 或 drawable 引用到的 vector，
/// 引用到嵌套 adaptive-icon 时继续看其 foreground。
fn container_to_vector(
    res: &mut Resolver,
    container: &XmlElement,
    depth: u32,
) -> Option<Rc<XmlElement>> {
    if depth > MAX_DEPTH {
        return None;
    }
    if let Some(v) = find_child(container, "vector") {
        return Some(Rc::new(v.clone()));
    }
    let raw = attr(container, "drawable")
        .filter(|s| !s.is_empty())?
        .to_string();
    let path = res.resolve_drawable_to_path(&raw, depth + 1)?;
    if !is_xml_path(&path) {
        return None;
    }
    let root = res.load_xml(&path)?;
    match tag(&root).as_str() {
        "vector" => Some(root),
        "adaptive-icon" => {
            let fg = find_child(&root, "foreground")?;
            container_to_vector(res, fg, depth + 1)
        }
        _ => None,
    }
}

/// 容器 → 位图层（Dart 版没有这一步，位图前景在导出时会丢失）。
fn container_to_bitmap(res: &mut Resolver, container: &XmlElement, depth: u32) -> Option<Layer> {
    let raw = attr(container, "drawable")
        .map(str::to_string)
        .or_else(|| find_drawable_like_value(container))?;
    let path = res.resolve_drawable_to_path(&raw, depth + 1)?;
    if is_xml_path(&path) {
        return None;
    }
    // 按文件头判断类型：混淆包里的位图可能没有扩展名
    let bytes = res.read_file(&path)?;
    let mime = match bytes.as_slice() {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xff, 0xd8, ..] => "image/jpeg",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => return None,
    };
    Some(Layer::Bitmap { mime, bytes })
}

/// adaptive-icon → 分层数据；不是 adaptive-icon 或什么都解析不出时返回 `None`。
pub(crate) fn resolve_adaptive(res: &mut Resolver, icon_path: &str) -> Option<AdaptiveSvgData> {
    let n = normalize_zip_path(icon_path);
    if !is_xml_path(&n) {
        return None;
    }
    let root = res.load_xml(&n)?;
    if tag(&root) != "adaptive-icon" {
        return None;
    }
    let mut data = AdaptiveSvgData::default();
    if let Some(bg) = find_child(&root, "background") {
        if let Some(v) = container_to_vector(res, bg, 0) {
            data.background = Some(Layer::Vector(v));
        } else {
            let raw = attr(bg, "drawable")
                .map(str::to_string)
                .or_else(|| find_drawable_like_value(bg));
            if let Some(raw) = raw.filter(|s| !s.is_empty()) {
                data.background_color = res.resolve_color_ref(Some(&raw), 0);
            }
            if data.background_color.is_none() {
                data.background_color = res.resolve_container_color(bg, 0);
            }
            if data.background_color.is_none() {
                data.background = container_to_bitmap(res, bg, 0);
            }
        }
    }
    if let Some(fg) = find_child(&root, "foreground") {
        data.foreground = container_to_vector(res, fg, 0)
            .map(Layer::Vector)
            .or_else(|| container_to_bitmap(res, fg, 0));
    }
    if data.background_color.is_none() && data.background.is_none() && data.foreground.is_none() {
        return None;
    }
    Some(data)
}

/// 单个 vector：本身是 vector，或 adaptive-icon 的 foreground（其次 background）。
pub(crate) fn resolve_vector(res: &mut Resolver, icon_path: &str) -> Option<Rc<XmlElement>> {
    let n = normalize_zip_path(icon_path);
    if !is_xml_path(&n) {
        return None;
    }
    let root = res.load_xml(&n)?;
    match tag(&root).as_str() {
        "vector" => Some(root),
        "adaptive-icon" => ["foreground", "background"]
            .into_iter()
            .filter_map(|name| find_child(&root, name))
            .find_map(|c| container_to_vector(res, c, 0)),
        _ => None,
    }
}

/// 导出 SVG：优先 adaptive 完整分层，其次单个 vector。
pub(crate) fn export_svg(res: &mut Resolver, icon_path: &str) -> Option<String> {
    if let Some(data) = resolve_adaptive(res, icon_path) {
        return svg::adaptive_document(res, &data);
    }
    let v = resolve_vector(res, icon_path)?;
    Some(svg::vector_document(res, &v).0)
}
