//! VectorDrawable / shape → SVG 生成器。移植并修正自 `lib/utils/vector_to_svg.dart`。
//!
//! 渲染与导出共用这里：渲染 = 生成 SVG + resvg 光栅化，导出 = 直接输出 SVG 字符串，
//! 因此两者在 clip-path、渐变、trimPath 上的行为完全一致。相对 Dart 版的修正：
//! - clip-path 真正生效：作用于其后的兄弟节点（含后代），group 结束即失效，
//!   多个 clip-path 相互求交（嵌套 `<g clip-path>` 实现）。
//! - sweep 渐变：SVG 无锥形渐变，用 N 个扇形楔块按角度取色、再用路径做 mask 近似。
//! - trimPath：展平后按长度截取生成新的 pathData（见 `path.rs`）。
//! - path 无 fillColor 时不填充（Android 默认透明，Dart 导出误用了 SVG 默认黑色）。

use std::fmt::Write as _;
use std::rc::Rc;

use crate::axml::XmlElement;

use super::color::{self, parse_dimension};
use super::path::{self, fmt, Bounds};
use super::xml::{attr, children_named, find_child, tag};

/// 渐变类型（Android `type`：0/linear、1/radial、2/sweep）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GradientKind {
    Linear,
    Radial,
    Sweep,
}

/// 渐变平铺模式（Android `tileMode`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TileMode {
    Clamp,
    Repeat,
    Mirror,
}

/// 已解析颜色的渐变描述。坐标保留原始值（缺省时用 `None`），
/// 具体位置在拿到路径包围盒后由 [`resolve_geometry`] 计算。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct GradientSpec {
    pub kind: GradientKind,
    pub start_x: Option<f32>,
    pub start_y: Option<f32>,
    pub end_x: Option<f32>,
    pub end_y: Option<f32>,
    pub center_x: Option<f32>,
    pub center_y: Option<f32>,
    pub radius: Option<f32>,
    /// radius 写成百分比（仅 shape 渐变有意义）
    pub radius_fraction: bool,
    pub angle: Option<f32>,
    pub colors: Vec<u32>,
    /// 规范化后的 stop 位置；`None` 表示均匀分布（与 Flutter `stops: null` 一致）
    pub offsets: Option<Vec<f32>>,
    pub tile: TileMode,
}

/// 填充 / 描边的颜料。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Paint {
    Color(u32),
    Gradient(GradientSpec),
}

/// 渐变坐标的语义：VectorDrawable 用视口绝对坐标；shape 用比例 + angle。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GradientSpace {
    Vector,
    Shape,
}

/// 生成 SVG 过程中需要的资源解析能力（颜色引用、渐变引用）。
pub(crate) trait PaintResolver {
    /// 解析 `fillColor` / `strokeColor` 的原始值（字面量、`@res/...` 引用 → 颜色或渐变）。
    fn paint(&mut self, raw: &str) -> Option<Paint>;
    /// 解析纯颜色（渐变 item 等）。
    fn color(&mut self, raw: &str) -> Option<u32>;
}

/// 只认字面量的解析器（单元测试用）。
#[cfg(test)]
pub(crate) struct LiteralResolver;

#[cfg(test)]
impl PaintResolver for LiteralResolver {
    fn paint(&mut self, raw: &str) -> Option<Paint> {
        color::parse_any_color_literal(raw).map(Paint::Color)
    }
    fn color(&mut self, raw: &str) -> Option<u32> {
        color::parse_any_color_literal(raw)
    }
}

fn parse_kind(raw: Option<&str>) -> GradientKind {
    match raw.unwrap_or("linear").trim().to_ascii_lowercase().as_str() {
        "1" | "radial" => GradientKind::Radial,
        "2" | "sweep" => GradientKind::Sweep,
        _ => GradientKind::Linear,
    }
}

fn parse_tile(raw: Option<&str>) -> TileMode {
    // decal（3）在 SVG 中无对应，退化为 clamp
    match raw.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "1" | "repeat" => TileMode::Repeat,
        "2" | "mirror" => TileMode::Mirror,
        _ => TileMode::Clamp,
    }
}

fn opt_dim(e: &XmlElement, name: &str) -> Option<f32> {
    let raw = attr(e, name)?;
    let v = parse_dimension(Some(raw), f32::NAN);
    if v.is_nan() {
        None
    } else {
        Some(v)
    }
}

/// 对应 Dart `_normalizeGradientStops`：位置非法（越界 / 非单调 / 数量不符）时返回 `None`，
/// 由调用方均匀分布。
fn normalize_stops(offsets: &[f32], count: usize) -> Option<Vec<f32>> {
    if count < 2 || offsets.len() != count {
        return None;
    }
    let mut last = -1.0f32;
    let mut out = Vec::with_capacity(count);
    for &v in offsets {
        if v.is_nan() || !(0.0..=1.0).contains(&v) || v < last {
            return None;
        }
        out.push(v);
        last = v;
    }
    Some(out)
}

/// 从 `<gradient>` 元素构造 [`GradientSpec`]，颜色通过回调解析（可能是资源引用）。
/// stop 优先取 `<item color offset>`，否则取 start/center/endColor；少于 2 个颜色返回 `None`。
pub(crate) fn gradient_from_element(
    e: &XmlElement,
    color_of: &mut dyn FnMut(&str) -> Option<u32>,
) -> Option<GradientSpec> {
    let mut colors = Vec::new();
    let mut offsets = Vec::new();
    for item in children_named(e, "item") {
        let Some(c) = attr(item, "color").and_then(&mut *color_of) else {
            continue;
        };
        colors.push(c);
        offsets.push(parse_dimension(attr(item, "offset"), 0.0));
    }
    if colors.is_empty() {
        let start = attr(e, "startColor").and_then(&mut *color_of);
        let center = attr(e, "centerColor").and_then(&mut *color_of);
        let end = attr(e, "endColor").and_then(&mut *color_of);
        if let (Some(s), Some(en)) = (start, end) {
            colors.push(s);
            offsets.push(0.0);
            if let Some(c) = center {
                colors.push(c);
                offsets.push(0.5);
            }
            colors.push(en);
            offsets.push(1.0);
        }
    }
    if colors.len() < 2 {
        return None;
    }
    let offsets = normalize_stops(&offsets, colors.len());
    Some(GradientSpec {
        kind: parse_kind(attr(e, "type")),
        start_x: opt_dim(e, "startX"),
        start_y: opt_dim(e, "startY"),
        end_x: opt_dim(e, "endX"),
        end_y: opt_dim(e, "endY"),
        center_x: opt_dim(e, "centerX"),
        center_y: opt_dim(e, "centerY"),
        radius: opt_dim(e, "gradientRadius"),
        radius_fraction: color::is_fraction(attr(e, "gradientRadius")),
        angle: opt_dim(e, "angle"),
        colors,
        offsets,
        tile: parse_tile(attr(e, "tileMode")),
    })
}

/// 渐变在用户坐标系中的几何。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GradientGeometry {
    Linear { x1: f32, y1: f32, x2: f32, y2: f32 },
    Radial { cx: f32, cy: f32, r: f32 },
    Sweep { cx: f32, cy: f32 },
}

/// shape 的 centerX/centerY 是比例（可能写成 `50%`）。
fn fraction(v: Option<f32>, default: f32) -> f32 {
    match v {
        Some(v) if v > 1.0 => v / 100.0,
        Some(v) => v,
        None => default,
    }
}

/// 结合包围盒计算渐变几何。
/// - Vector：缺省坐标按 Dart 版取路径包围盒（左上→右下、中心、max(w,h)/2），
///   起止点重合时按 `angle` 过中心取方向。
/// - Shape：按 Android GradientDrawable 语义，linear 用 angle（45° 的倍数），
///   radial / sweep 的中心是比例。
pub(crate) fn resolve_geometry(
    spec: &GradientSpec,
    bounds: Option<Bounds>,
    space: GradientSpace,
) -> GradientGeometry {
    let b = bounds.unwrap_or(Bounds {
        x0: 0.0,
        y0: 0.0,
        x1: 0.0,
        y1: 0.0,
    });
    let empty = b.is_empty();
    match space {
        GradientSpace::Vector => match spec.kind {
            GradientKind::Radial => GradientGeometry::Radial {
                cx: spec.center_x.unwrap_or(if empty { 0.0 } else { b.cx() }),
                cy: spec.center_y.unwrap_or(if empty { 0.0 } else { b.cy() }),
                r: spec.radius.unwrap_or(if empty {
                    1.0
                } else {
                    b.width().max(b.height()) / 2.0
                }),
            },
            GradientKind::Sweep => GradientGeometry::Sweep {
                cx: spec.center_x.unwrap_or(if empty { 0.0 } else { b.cx() }),
                cy: spec.center_y.unwrap_or(if empty { 0.0 } else { b.cy() }),
            },
            GradientKind::Linear => {
                let mut x1 = spec.start_x.unwrap_or(if empty { 0.0 } else { b.x0 });
                let mut y1 = spec.start_y.unwrap_or(if empty { 0.0 } else { b.y0 });
                let mut x2 = spec.end_x.unwrap_or(if empty { 1.0 } else { b.x1 });
                let mut y2 = spec.end_y.unwrap_or(if empty { 0.0 } else { b.y1 });
                if x1 == x2 && y1 == y2 && !empty {
                    let rad = spec.angle.unwrap_or(0.0).to_radians();
                    let half = b.width().max(b.height()) * 0.5;
                    let (dx, dy) = (rad.cos() * half, rad.sin() * half);
                    x1 = b.cx() - dx;
                    y1 = b.cy() - dy;
                    x2 = b.cx() + dx;
                    y2 = b.cy() + dy;
                }
                GradientGeometry::Linear { x1, y1, x2, y2 }
            }
        },
        GradientSpace::Shape => {
            let cx = b.x0 + b.width() * fraction(spec.center_x, 0.5);
            let cy = b.y0 + b.height() * fraction(spec.center_y, 0.5);
            match spec.kind {
                GradientKind::Radial => {
                    let r = match spec.radius {
                        Some(r) if spec.radius_fraction => b.width().min(b.height()) * r / 100.0,
                        Some(r) => r,
                        None => b.width().max(b.height()) / 2.0,
                    };
                    GradientGeometry::Radial { cx, cy, r }
                }
                GradientKind::Sweep => GradientGeometry::Sweep { cx, cy },
                GradientKind::Linear => {
                    // Android 只接受 45° 的倍数，0 = 左→右，逆时针递增
                    let a = ((spec.angle.unwrap_or(0.0) / 45.0).round() as i32).rem_euclid(8);
                    let (l, t, r, bt) = (b.x0, b.y0, b.x1, b.y1);
                    let (x1, y1, x2, y2) = match a {
                        0 => (l, t, r, t),
                        1 => (l, bt, r, t),
                        2 => (l, bt, l, t),
                        3 => (r, bt, l, t),
                        4 => (r, t, l, t),
                        5 => (r, t, l, bt),
                        6 => (l, t, l, bt),
                        _ => (l, t, r, bt),
                    };
                    GradientGeometry::Linear { x1, y1, x2, y2 }
                }
            }
        }
    }
}

/// 在 stop 序列上按 t 取色（非预乘线性插值，与 Skia 默认一致）。
fn sample(spec: &GradientSpec, t: f32) -> u32 {
    let n = spec.colors.len();
    let pos: Vec<f32> = match &spec.offsets {
        Some(o) => o.clone(),
        None => (0..n).map(|i| i as f32 / (n - 1) as f32).collect(),
    };
    if t <= pos[0] {
        return spec.colors[0];
    }
    for i in 1..n {
        if t <= pos[i] {
            let span = pos[i] - pos[i - 1];
            let f = if span <= 0.0 {
                1.0
            } else {
                (t - pos[i - 1]) / span
            };
            return lerp_color(spec.colors[i - 1], spec.colors[i], f);
        }
    }
    spec.colors[n - 1]
}

fn lerp_color(a: u32, b: u32, f: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as f32;
    let mix = |s: u32| -> u32 {
        (ch(a, s) + (ch(b, s) - ch(a, s)) * f)
            .round()
            .clamp(0.0, 255.0) as u32
    };
    (mix(24) << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// 转义 XML 属性值。
pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn f(v: f32) -> String {
    fmt(v as f64)
}

/// 描边样式。
#[derive(Debug, Clone, Default)]
pub(crate) struct StrokeStyle {
    pub width: f32,
    pub cap: Option<&'static str>,
    pub join: Option<&'static str>,
    pub miter: Option<f32>,
}

impl StrokeStyle {
    fn attrs(&self) -> String {
        let mut s = format!(" stroke-width=\"{}\"", f(self.width));
        if let Some(c) = self.cap {
            let _ = write!(s, " stroke-linecap=\"{c}\"");
        }
        if let Some(j) = self.join {
            let _ = write!(s, " stroke-linejoin=\"{j}\"");
        }
        if let Some(m) = self.miter {
            let _ = write!(s, " stroke-miterlimit=\"{}\"", f(m.max(1.0)));
        }
        s
    }
}

/// 二进制 XML 里枚举是整数：strokeLineCap butt=0 round=1 square=2。
fn parse_cap(raw: Option<&str>) -> Option<&'static str> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "round" | "1" => Some("round"),
        "square" | "2" => Some("square"),
        "butt" | "0" => Some("butt"),
        _ => None,
    }
}

/// strokeLineJoin miter=0 round=1 bevel=2。
fn parse_join(raw: Option<&str>) -> Option<&'static str> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "round" | "1" => Some("round"),
        "bevel" | "2" => Some("bevel"),
        "miter" | "0" => Some("miter"),
        _ => None,
    }
}

/// fillType evenOdd 的各种写法（文本 `evenOdd`、二进制枚举 `1` / `0x1`）。
pub(crate) fn is_even_odd(raw: Option<&str>) -> bool {
    matches!(
        raw.unwrap_or("").trim().to_ascii_lowercase().as_str(),
        "evenodd" | "1" | "0x1"
    )
}

/// VectorDrawable 视口尺寸：viewportWidth/Height，缺省回退 width/height，至少为 1。
pub(crate) fn vector_viewport(v: &XmlElement) -> (f32, f32) {
    let vw = parse_dimension(attr(v, "viewportWidth"), 0.0);
    let vh = parse_dimension(attr(v, "viewportHeight"), 0.0);
    let w = if vw > 0.0 {
        vw
    } else {
        parse_dimension(attr(v, "width"), 0.0).max(1.0)
    };
    let h = if vh > 0.0 {
        vh
    } else {
        parse_dimension(attr(v, "height"), 0.0).max(1.0)
    };
    (w, h)
}

/// SVG 生成器。`defs` 与 id 计数器在同一文档内共享，便于把多个 vector 拼进一个 SVG。
pub(crate) struct SvgBuilder<'r> {
    res: &'r mut dyn PaintResolver,
    defs: String,
    next_id: usize,
    /// 实际产生的填充 / 描边数量；为 0 说明什么都没画（对应 Dart 的 `_didDraw`）。
    pub painted: usize,
}

impl<'r> SvgBuilder<'r> {
    pub fn new(res: &'r mut dyn PaintResolver) -> Self {
        Self {
            res,
            defs: String::new(),
            next_id: 0,
            painted: 0,
        }
    }

    fn new_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}_{}", self.next_id);
        self.next_id += 1;
        id
    }

    /// vector 的内容（不含外层 `<svg>`）。vector 的 alpha 作为整层不透明度
    /// （Android 以 layer 方式应用），这样重叠路径不会被重复减淡。
    pub fn vector_body(&mut self, vector: &XmlElement) -> String {
        let alpha = parse_dimension(attr(vector, "alpha"), 1.0).clamp(0.0, 1.0);
        let mut out = String::new();
        if alpha < 1.0 {
            let _ = writeln!(out, "<g opacity=\"{}\">", f(alpha));
        }
        self.emit_children(&mut out, &vector.children, 1.0);
        if alpha < 1.0 {
            out.push_str("</g>\n");
        }
        out
    }

    /// 按 Android 语义处理兄弟序列：clip-path 裁剪其后的所有兄弟，直到父 group 结束。
    fn emit_children(&mut self, out: &mut String, children: &[XmlElement], alpha: f32) {
        let mut open = 0;
        for child in children {
            match tag(child).as_str() {
                "group" => self.emit_group(out, child, alpha),
                "path" => self.emit_path(out, child, alpha),
                "clip-path" => {
                    let Some(d) = attr(child, "pathData").filter(|d| !d.trim().is_empty()) else {
                        continue;
                    };
                    let id = self.new_id("clip");
                    let rule = if is_even_odd(attr(child, "fillType")) {
                        " clip-rule=\"evenodd\""
                    } else {
                        ""
                    };
                    let _ = writeln!(
                        self.defs,
                        "<clipPath id=\"{id}\"><path d=\"{}\"{rule}/></clipPath>",
                        esc(d)
                    );
                    let _ = writeln!(out, "<g clip-path=\"url(#{id})\">");
                    open += 1;
                }
                _ => {}
            }
        }
        for _ in 0..open {
            out.push_str("</g>\n");
        }
    }

    fn emit_group(&mut self, out: &mut String, g: &XmlElement, alpha: f32) {
        let d = |n: &str, fb: f32| parse_dimension(attr(g, n), fb);
        let (rot, px, py) = (d("rotation", 0.0), d("pivotX", 0.0), d("pivotY", 0.0));
        let (sx, sy) = (d("scaleX", 1.0), d("scaleY", 1.0));
        let (tx, ty) = (d("translateX", 0.0), d("translateY", 0.0));
        // Android 的 group 本身没有 alpha 属性，但 Dart 版支持；这里沿用 Dart 的
        // “乘进子路径不透明度”的做法以保持一致。
        let a = (alpha * d("alpha", 1.0).clamp(0.0, 1.0)).clamp(0.0, 1.0);
        // 变换顺序：translate(tx,ty)·translate(pivot)·rotate·scale·translate(-pivot)
        let mut t = Vec::new();
        if tx != 0.0 || ty != 0.0 {
            t.push(format!("translate({} {})", f(tx), f(ty)));
        }
        let has_rs = rot != 0.0 || sx != 1.0 || sy != 1.0;
        if has_rs && (px != 0.0 || py != 0.0) {
            t.push(format!("translate({} {})", f(px), f(py)));
        }
        if rot != 0.0 {
            t.push(format!("rotate({})", f(rot)));
        }
        if sx != 1.0 || sy != 1.0 {
            t.push(format!("scale({} {})", f(sx), f(sy)));
        }
        if has_rs && (px != 0.0 || py != 0.0) {
            t.push(format!("translate({} {})", f(-px), f(-py)));
        }
        if t.is_empty() {
            out.push_str("<g>\n");
        } else {
            let _ = writeln!(out, "<g transform=\"{}\">", t.join(" "));
        }
        self.emit_children(out, &g.children, a);
        out.push_str("</g>\n");
    }

    /// `<aapt:attr name="android:fillColor"><gradient/></aapt:attr>` 内联渐变。
    fn inline_gradient(&mut self, p: &XmlElement, attr_name: &str) -> Option<Paint> {
        let holder = children_named(p, "attr")
            .find(|a| attr(a, "name").is_some_and(|n| n.rsplit(':').next() == Some(attr_name)))?;
        let g = find_child(holder, "gradient")?;
        let res = &mut *self.res;
        gradient_from_element(g, &mut |s| res.color(s)).map(Paint::Gradient)
    }

    fn emit_path(&mut self, out: &mut String, p: &XmlElement, alpha: f32) {
        let Some(raw_d) = attr(p, "pathData").map(str::trim).filter(|d| !d.is_empty()) else {
            return;
        };
        let ts = parse_dimension(attr(p, "trimPathStart"), 0.0);
        let te = parse_dimension(attr(p, "trimPathEnd"), 1.0);
        let to = parse_dimension(attr(p, "trimPathOffset"), 0.0);
        let d = if path::needs_trim(ts, te, to) {
            path::trim(raw_d, ts, te, to)
        } else {
            raw_d.to_string()
        };
        if d.is_empty() {
            return;
        }
        let bounds = path::bounds(&d);
        let even_odd = is_even_odd(attr(p, "fillType"));

        let fill = self
            .inline_gradient(p, "fillColor")
            .or_else(|| attr(p, "fillColor").and_then(|raw| self.res.paint(raw)));
        let fill_alpha = (parse_dimension(attr(p, "fillAlpha"), 1.0) * alpha).clamp(0.0, 1.0);
        if let Some(paint) = fill {
            if fill_alpha > 0.0 {
                self.emit_fill(
                    out,
                    &d,
                    even_odd,
                    &paint,
                    fill_alpha,
                    bounds,
                    GradientSpace::Vector,
                );
            }
        }

        let stroke = self
            .inline_gradient(p, "strokeColor")
            .or_else(|| attr(p, "strokeColor").and_then(|raw| self.res.paint(raw)));
        let width = parse_dimension(attr(p, "strokeWidth"), 0.0);
        let stroke_alpha = (parse_dimension(attr(p, "strokeAlpha"), 1.0) * alpha).clamp(0.0, 1.0);
        if let Some(paint) = stroke {
            if width > 0.0 && stroke_alpha > 0.0 {
                let miter = parse_dimension(attr(p, "strokeMiterLimit"), 0.0);
                let style = StrokeStyle {
                    width,
                    cap: parse_cap(attr(p, "strokeLineCap")),
                    join: parse_join(attr(p, "strokeLineJoin")),
                    miter: (miter > 0.0).then_some(miter),
                };
                self.emit_stroke(
                    out,
                    &d,
                    &paint,
                    stroke_alpha,
                    &style,
                    bounds,
                    GradientSpace::Vector,
                );
            }
        }
    }

    /// 输出一个填充图元。
    #[allow(clippy::too_many_arguments)]
    pub fn emit_fill(
        &mut self,
        out: &mut String,
        d: &str,
        even_odd: bool,
        paint: &Paint,
        opacity: f32,
        bounds: Option<Bounds>,
        space: GradientSpace,
    ) {
        let rule = if even_odd {
            " fill-rule=\"evenodd\""
        } else {
            ""
        };
        let d = esc(d);
        match paint {
            Paint::Color(c) => {
                let (hex, a) = color::svg_color(*c);
                let _ = writeln!(
                    out,
                    "<path d=\"{d}\" fill=\"{hex}\"{}{rule}/>",
                    opacity_attr("fill-opacity", a * opacity)
                );
            }
            Paint::Gradient(spec) => {
                let geo = resolve_geometry(spec, bounds, space);
                if let GradientGeometry::Sweep { cx, cy } = geo {
                    let mask_shape = format!("<path d=\"{d}\" fill=\"#ffffff\"{rule}/>");
                    self.emit_sweep(out, &mask_shape, spec, cx, cy, bounds, 0.0, opacity);
                } else {
                    let id = self.gradient_def(spec, geo);
                    let _ = writeln!(
                        out,
                        "<path d=\"{d}\" fill=\"url(#{id})\"{}{rule}/>",
                        opacity_attr("fill-opacity", opacity)
                    );
                }
            }
        }
        self.painted += 1;
    }

    /// 输出一个描边图元。
    #[allow(clippy::too_many_arguments)]
    pub fn emit_stroke(
        &mut self,
        out: &mut String,
        d: &str,
        paint: &Paint,
        opacity: f32,
        style: &StrokeStyle,
        bounds: Option<Bounds>,
        space: GradientSpace,
    ) {
        let d = esc(d);
        let sa = style.attrs();
        match paint {
            Paint::Color(c) => {
                let (hex, a) = color::svg_color(*c);
                let _ = writeln!(
                    out,
                    "<path d=\"{d}\" fill=\"none\" stroke=\"{hex}\"{}{sa}/>",
                    opacity_attr("stroke-opacity", a * opacity)
                );
            }
            Paint::Gradient(spec) => {
                let geo = resolve_geometry(spec, bounds, space);
                if let GradientGeometry::Sweep { cx, cy } = geo {
                    let mask_shape =
                        format!("<path d=\"{d}\" fill=\"none\" stroke=\"#ffffff\"{sa}/>");
                    self.emit_sweep(out, &mask_shape, spec, cx, cy, bounds, style.width, opacity);
                } else {
                    let id = self.gradient_def(spec, geo);
                    let _ = writeln!(
                        out,
                        "<path d=\"{d}\" fill=\"none\" stroke=\"url(#{id})\"{}{sa}/>",
                        opacity_attr("stroke-opacity", opacity)
                    );
                }
            }
        }
        self.painted += 1;
    }

    /// sweep 渐变近似：以中心为顶点画 N 个扇形楔块，每块取中点角度处的颜色，
    /// 再用目标形状做 mask。角度 0 指向 +x、顺时针（y 轴向下）递增，与 Android
    /// SweepGradient 一致。相邻楔块略微重叠，避免抗锯齿接缝。
    #[allow(clippy::too_many_arguments)]
    fn emit_sweep(
        &mut self,
        out: &mut String,
        mask_shape: &str,
        spec: &GradientSpec,
        cx: f32,
        cy: f32,
        bounds: Option<Bounds>,
        pad: f32,
        opacity: f32,
    ) {
        const N: usize = 90;
        let reach = match bounds {
            Some(b) => {
                let corners = [(b.x0, b.y0), (b.x1, b.y0), (b.x0, b.y1), (b.x1, b.y1)];
                corners
                    .iter()
                    .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
                    .fold(0.0f32, f32::max)
            }
            None => 1000.0,
        };
        // 弦与弧之间留足余量
        let r = (reach + pad) * 1.5 + 1.0;
        let id = self.new_id("sweep");
        let _ = writeln!(self.defs, "<mask id=\"{id}\">{mask_shape}</mask>");
        let _ = writeln!(
            out,
            "<g mask=\"url(#{id})\"{}>",
            opacity_attr("opacity", opacity)
        );
        let step = std::f32::consts::TAU / N as f32;
        for i in 0..N {
            let a0 = i as f32 * step;
            // 重叠只向后延伸，最后一块不能越过 360° 盖到起点（那里是真实的颜色断层）
            let a1 = (a0 + step * 1.5).min(std::f32::consts::TAU);
            let c = sample(spec, (i as f32 + 0.5) / N as f32);
            let (hex, a) = color::svg_color(c);
            let _ = writeln!(
                out,
                "<path d=\"M{} {}L{} {}L{} {}Z\" fill=\"{hex}\"{}/>",
                f(cx),
                f(cy),
                f(cx + r * a0.cos()),
                f(cy + r * a0.sin()),
                f(cx + r * a1.cos()),
                f(cy + r * a1.sin()),
                opacity_attr("fill-opacity", a)
            );
        }
        out.push_str("</g>\n");
    }

    /// 线性 / 径向渐变写入 defs，返回 id。
    fn gradient_def(&mut self, spec: &GradientSpec, geo: GradientGeometry) -> String {
        let id = self.new_id("gradient");
        let spread = match spec.tile {
            TileMode::Clamp => "",
            TileMode::Repeat => " spreadMethod=\"repeat\"",
            TileMode::Mirror => " spreadMethod=\"reflect\"",
        };
        let (open, close) = match geo {
            GradientGeometry::Linear { x1, y1, x2, y2 } => (
                format!("<linearGradient id=\"{id}\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" gradientUnits=\"userSpaceOnUse\"{spread}>", f(x1), f(y1), f(x2), f(y2)),
                "</linearGradient>",
            ),
            // sweep 不会走到这里（调用方单独处理），保险起见退化为中心径向
            GradientGeometry::Radial { cx, cy, r } => (radial_open(&id, cx, cy, r, spread), "</radialGradient>"),
            GradientGeometry::Sweep { cx, cy } => (radial_open(&id, cx, cy, 1.0, spread), "</radialGradient>"),
        };
        let n = spec.colors.len();
        let _ = writeln!(self.defs, "{open}");
        for (i, &c) in spec.colors.iter().enumerate() {
            let off = match &spec.offsets {
                Some(o) => o[i],
                None => i as f32 / (n - 1) as f32,
            };
            let (hex, a) = color::svg_color(c);
            let _ = writeln!(
                self.defs,
                "<stop offset=\"{}\" stop-color=\"{hex}\"{}/>",
                f(off),
                opacity_attr("stop-opacity", a)
            );
        }
        let _ = writeln!(self.defs, "{close}");
        id
    }

    /// 包装成完整 SVG 文档。
    pub fn document(&self, width: f32, height: f32, body: &str) -> String {
        let (w, h) = (f(width.max(1e-3)), f(height.max(1e-3)));
        let mut s = String::with_capacity(body.len() + self.defs.len() + 256);
        s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = writeln!(s, "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">");
        if !self.defs.is_empty() {
            let _ = write!(s, "<defs>\n{}</defs>\n", self.defs);
        }
        s.push_str(body);
        s.push_str("</svg>\n");
        s
    }

    /// 追加任意 defs（如 adaptive 的圆角裁剪）。
    pub fn push_def(&mut self, def: &str) {
        self.defs.push_str(def);
        self.defs.push('\n');
    }
}

fn radial_open(id: &str, cx: f32, cy: f32, r: f32, spread: &str) -> String {
    // r ≤ 0 时 SVG 视为无效渐变而不填充，Android 也无法构造；给极小正值保住颜色
    format!("<radialGradient id=\"{id}\" cx=\"{}\" cy=\"{}\" r=\"{}\" gradientUnits=\"userSpaceOnUse\"{spread}>", f(cx), f(cy), f(r.max(1e-3)))
}

fn opacity_attr(name: &str, v: f32) -> String {
    if v < 1.0 {
        format!(" {name}=\"{}\"", f(v.max(0.0)))
    } else {
        String::new()
    }
}

/// 单个 vector → 完整 SVG（尺寸取视口）。返回 (svg, 绘制图元数)。
pub(crate) fn vector_document(res: &mut dyn PaintResolver, vector: &XmlElement) -> (String, usize) {
    let (vw, vh) = vector_viewport(vector);
    let mut b = SvgBuilder::new(res);
    let body = b.vector_body(vector);
    (b.document(vw, vh, &body), b.painted)
}

/// adaptive-icon 导出时的一层。
#[derive(Debug, Clone)]
pub(crate) enum Layer {
    Vector(Rc<XmlElement>),
    /// 位图层按 data URI 内嵌（Dart 版会直接丢掉位图前景）。
    Bitmap {
        mime: &'static str,
        bytes: Vec<u8>,
    },
}

/// adaptive-icon 导出数据（对应 Dart `AdaptiveIconSvgData`）。
#[derive(Debug, Clone, Default)]
pub(crate) struct AdaptiveSvgData {
    pub background_color: Option<u32>,
    pub background: Option<Layer>,
    pub foreground: Option<Layer>,
}

/// 渲染时 vector 前景的放大系数（与 Android 实际显示接近：108dp 画布裁 72dp 可见区，
/// Dart 版经验值）。
pub(crate) const FG_SCALE_VECTOR: f32 = 1.28;
/// 位图前景的放大系数。
pub(crate) const FG_SCALE_BITMAP: f32 = 1.60;

/// adaptive-icon → SVG：圆角裁剪 `#adaptive_clip`（半径 min*0.22），背景纯色 rect /
/// 背景层，前景按 1.28（vector）或 1.60（位图）以中心放大。
pub(crate) fn adaptive_document(
    res: &mut dyn PaintResolver,
    data: &AdaptiveSvgData,
) -> Option<String> {
    if data.background_color.is_none() && data.background.is_none() && data.foreground.is_none() {
        return None;
    }
    let ref_vec = [&data.foreground, &data.background]
        .into_iter()
        .find_map(|l| match l {
            Some(Layer::Vector(v)) => Some(v.clone()),
            _ => None,
        });
    let (vw, vh) = match &ref_vec {
        Some(v) => {
            let vw = parse_dimension(attr(v, "viewportWidth"), 0.0);
            let vh = parse_dimension(attr(v, "viewportHeight"), 0.0);
            (
                if vw > 0.0 { vw } else { 108.0 },
                if vh > 0.0 { vh } else { 108.0 },
            )
        }
        None => (108.0, 108.0),
    };
    let radius = vw.min(vh) * 0.22;
    let mut b = SvgBuilder::new(res);
    b.push_def(&format!(
        "<clipPath id=\"adaptive_clip\"><rect width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\"/></clipPath>",
        f(vw),
        f(vh),
        f(radius),
        f(radius)
    ));
    let mut body = String::from("<g clip-path=\"url(#adaptive_clip)\">\n");
    if let Some(c) = data.background_color {
        let (hex, a) = color::svg_color(c);
        let _ = writeln!(
            body,
            "<rect width=\"{}\" height=\"{}\" fill=\"{hex}\"{}/>",
            f(vw),
            f(vh),
            opacity_attr("fill-opacity", a)
        );
    }
    if let Some(layer) = &data.background {
        emit_layer(&mut b, &mut body, layer, vw, vh, 1.0, 1.0);
    }
    if let Some(layer) = &data.foreground {
        emit_layer(
            &mut b,
            &mut body,
            layer,
            vw,
            vh,
            FG_SCALE_VECTOR,
            FG_SCALE_BITMAP,
        );
    }
    body.push_str("</g>\n");
    Some(b.document(vw, vh, &body))
}

fn emit_layer(
    b: &mut SvgBuilder,
    body: &mut String,
    layer: &Layer,
    vw: f32,
    vh: f32,
    vec_scale: f32,
    bmp_scale: f32,
) {
    let scale = match layer {
        Layer::Vector(_) => vec_scale,
        Layer::Bitmap { .. } => bmp_scale,
    };
    let scaled = (scale - 1.0).abs() > 1e-6;
    if scaled {
        let (cx, cy) = (vw / 2.0, vh / 2.0);
        let _ = writeln!(
            body,
            "<g transform=\"translate({} {}) scale({}) translate({} {})\">",
            f(cx),
            f(cy),
            f(scale),
            f(-cx),
            f(-cy)
        );
    }
    match layer {
        Layer::Vector(v) => {
            let (lw, lh) = vector_viewport(v);
            let vp_scaled = lw != vw || lh != vh;
            if vp_scaled {
                let _ = writeln!(
                    body,
                    "<g transform=\"scale({} {})\">",
                    f(vw / lw),
                    f(vh / lh)
                );
            }
            body.push_str(&b.vector_body(v));
            if vp_scaled {
                body.push_str("</g>\n");
            }
        }
        Layer::Bitmap { mime, bytes } => {
            let _ = writeln!(
                body,
                "<image width=\"{}\" height=\"{}\" preserveAspectRatio=\"xMidYMid meet\" xlink:href=\"data:{mime};base64,{}\"/>",
                f(vw),
                f(vh),
                base64(bytes)
            );
            b.painted += 1;
        }
    }
    if scaled {
        body.push_str("</g>\n");
    }
}

/// 标准 base64（只在导出内嵌位图时用，不值得为此引入依赖）。
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}
