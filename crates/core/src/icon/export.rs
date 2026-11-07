//! SVG 导出：把 XML 候选解析为 adaptive 分层数据或单个 vector，再交给 [`super::svg`]。
//! 移植自 `resolveAdaptiveIconForSvg` / `resolveToVectorElement` / `_resolveContainerToVector`。

use std::rc::Rc;

use crate::axml::XmlElement;

use super::color::{is_fraction, parse_dimension};
use super::resolve::{
    find_drawable_like_value, is_xml_path, normalize_zip_path, Resolver, MAX_DEPTH,
};
use super::svg::{self, AdaptiveSvgData, Layer};
use super::xml::{attr, find_child, tag};

/// 108dp：adaptive-icon 图层画布的边长，dp 写法的 inset 按它换算成比例。
const ADAPTIVE_LAYER_DP: f32 = 108.0;

/// 容器（background / foreground / inset / item）→ 图层：内联子元素优先，其次 drawable 引用。
fn container_layer(res: &mut Resolver, container: &XmlElement, depth: u32) -> Option<Layer> {
    if depth > MAX_DEPTH {
        return None;
    }
    if let Some(layer) = container
        .children
        .iter()
        .find_map(|child| element_layer(res, child, depth + 1))
    {
        return Some(layer);
    }
    let raw = attr(container, "drawable")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| find_drawable_like_value(container))?;
    value_layer(res, &raw, depth + 1)
}

/// drawable 值（引用 / zip 路径）→ 图层。
fn value_layer(res: &mut Resolver, raw: &str, depth: u32) -> Option<Layer> {
    let path = res.resolve_drawable_to_path(raw, depth + 1)?;
    if !is_xml_path(&path) {
        return bitmap_layer(res, &path);
    }
    let root = res.load_xml(&path)?;
    if tag(&root) == "vector" {
        return Some(Layer::Vector(root));
    }
    element_layer(res, &root, depth + 1)
}

/// XML 元素 → 图层。vector / inset / bitmap / 嵌套 adaptive-icon（取前景）；其余不支持。
fn element_layer(res: &mut Resolver, e: &XmlElement, depth: u32) -> Option<Layer> {
    if depth > MAX_DEPTH {
        return None;
    }
    match tag(e).as_str() {
        "vector" => Some(Layer::Vector(Rc::new(e.clone()))),
        "inset" => {
            let inner = container_layer(res, e, depth + 1)?;
            Some(Layer::Inset {
                inner: Box::new(inner),
                insets: inset_fractions(e),
            })
        }
        "bitmap" | "nine-patch" => {
            let src = attr(e, "src").or_else(|| attr(e, "drawable"))?.to_string();
            value_layer(res, &src, depth + 1)
        }
        "adaptive-icon" => container_layer(res, find_child(e, "foreground")?, depth + 1),
        _ => None,
    }
}

/// `<inset>` 的四边内缩比例（左 / 上 / 右 / 下）：百分比直接换算，dp 相对 108dp 图层。
fn inset_fractions(e: &XmlElement) -> [f32; 4] {
    let frac = |raw: Option<&str>| -> Option<f32> {
        let raw = raw?;
        let v = parse_dimension(Some(raw), 0.0);
        Some(if is_fraction(Some(raw)) {
            v / 100.0
        } else {
            v / ADAPTIVE_LAYER_DP
        })
    };
    let all = frac(attr(e, "inset")).unwrap_or(0.0);
    ["insetLeft", "insetTop", "insetRight", "insetBottom"]
        .map(|name| frac(attr(e, name)).unwrap_or(all).clamp(0.0, 1.0))
}

/// 位图文件 → 图层。按文件头判断类型：混淆包里的位图可能没有扩展名。
fn bitmap_layer(res: &mut Resolver, path: &str) -> Option<Layer> {
    let bytes = res.read_file(path)?;
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
        // 顺序与原先一致：vector 优先，其次纯色，最后位图等其它图层。
        let layer = container_layer(res, bg, 0);
        if let Some(layer) = layer.filter(|l| matches!(l, Layer::Vector(_))) {
            data.background = Some(layer);
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
                data.background = container_layer(res, bg, 0);
            }
        }
    }
    if let Some(fg) = find_child(&root, "foreground") {
        data.foreground = container_layer(res, fg, 0);
    }
    if data.background_color.is_none() && data.background.is_none() && data.foreground.is_none() {
        return None;
    }
    Some(data)
}

/// 导出 SVG：adaptive-icon 输出完整分层，否则输出单层（vector 或包着 vector 的 inset 等）。
/// 整棵树里没有任何 vector（纯位图内容）时返回 `None`：那种 SVG 只是把位图包一层，没有意义。
pub(crate) fn export_svg(res: &mut Resolver, icon_path: &str) -> Option<String> {
    let export = resolve_export(res, icon_path).filter(Export::has_vector)?;
    match export {
        Export::Adaptive(data) => svg::adaptive_document(res, &data),
        Export::Single(Layer::Vector(v)) => Some(svg::vector_document(res, &v).0),
        Export::Single(layer) => svg::layer_document(res, &layer),
    }
}

/// XML 候选能导出成真正的（含矢量的）SVG。
pub(crate) fn has_vector(res: &mut Resolver, icon_path: &str) -> bool {
    resolve_export(res, icon_path).is_some_and(|e| e.has_vector())
}

/// XML 候选里各位图图层的最大边长（像素）；含 vector 时为 `None`（矢量无固有分辨率）。
/// 用于让纯位图的 adaptive-icon / inset 按原始分辨率导出，而不是放大到固定尺寸。
pub(crate) fn bitmap_native_size(res: &mut Resolver, icon_path: &str) -> Option<u32> {
    let export = resolve_export(res, icon_path)?;
    if export.has_vector() {
        return None;
    }
    let layers: Vec<Layer> = match export {
        Export::Adaptive(data) => [data.background, data.foreground]
            .into_iter()
            .flatten()
            .collect(),
        Export::Single(layer) => vec![layer],
    };
    layers
        .iter()
        .filter_map(|l| l.bitmap())
        .filter_map(super::bitmap_dimensions)
        .map(|(w, h)| w.max(h))
        .max()
}

enum Export {
    Adaptive(AdaptiveSvgData),
    Single(Layer),
}

impl Export {
    fn has_vector(&self) -> bool {
        match self {
            Export::Adaptive(data) => [&data.background, &data.foreground]
                .into_iter()
                .any(|l| l.as_ref().is_some_and(|l| l.vector().is_some())),
            Export::Single(layer) => layer.vector().is_some(),
        }
    }
}

fn resolve_export(res: &mut Resolver, icon_path: &str) -> Option<Export> {
    if let Some(data) = resolve_adaptive(res, icon_path) {
        return Some(Export::Adaptive(data));
    }
    let n = normalize_zip_path(icon_path);
    if !is_xml_path(&n) {
        return None;
    }
    let root = res.load_xml(&n)?;
    if tag(&root) == "vector" {
        return Some(Export::Single(Layer::Vector(root)));
    }
    element_layer(res, &root, 0).map(Export::Single)
}
