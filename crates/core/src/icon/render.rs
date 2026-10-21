//! Drawable 渲染。移植自 `adaptive_icon_renderer.dart` 的 `_renderDrawable` 系列。
//!
//! 画布是 tiny-skia 的 `Pixmap`：
//! - vector / shape 先生成 SVG（与导出共用 [`super::svg`]），再用 resvg 光栅化到目标矩形；
//! - 位图用 image 解码后等比居中（contain）绘制；
//! - adaptive-icon 在离屏图层上合成，再通过圆角遮罩贴回画布（等价于 Dart 的 clipPath）。

use std::fmt::Write as _;

use tiny_skia::{
    FillRule, FilterQuality, Mask, Paint as SkPaint, PathBuilder, Pixmap, PixmapPaint, Transform,
};

use crate::axml::XmlElement;

use super::color::{self, is_color_literal, parse_dimension};
use super::path::Bounds;
use super::resolve::{
    find_drawable_like_value, is_bitmap_path, is_xml_path, looks_like_zip_path, normalize_zip_path,
    Resolver, MAX_DEPTH,
};
use super::svg::{
    self, gradient_from_element, GradientSpace, Paint, StrokeStyle, SvgBuilder, FG_SCALE_BITMAP,
    FG_SCALE_VECTOR,
};
use super::xml::{attr, attr_opt, children_named, find_child, tag, text};
use super::RgbaImage;

/// Dart 版的默认画布边长；尺寸类属性（dp）在该画布上按 1dp = 1px 处理，
/// 其它画布尺寸按比例缩放，保证不同输出尺寸下视觉一致。
pub(crate) const BASE_CANVAS: f32 = 432.0;

const WHITE: u32 = 0xffff_ffff;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    fn inset(&self, l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect {
            x: self.x + l,
            y: self.y + t,
            w: self.w - l - r,
            h: self.h - t - b,
        }
    }
    fn scale_around_center(&self, s: f32) -> Rect {
        if s == 1.0 || self.is_empty() {
            return *self;
        }
        let (w, h) = (self.w * s, self.h * s);
        Rect {
            x: self.x + (self.w - w) / 2.0,
            y: self.y + (self.h - h) / 2.0,
            w,
            h,
        }
    }
    /// 等比缩放到目标内并居中。
    fn fit_contain(&self, sw: f32, sh: f32) -> Rect {
        if sw <= 0.0 || sh <= 0.0 || self.is_empty() {
            return *self;
        }
        let s = (self.w / sw).min(self.h / sh);
        let (w, h) = (sw * s, sh * s);
        Rect {
            x: self.x + (self.w - w) / 2.0,
            y: self.y + (self.h - h) / 2.0,
            w,
            h,
        }
    }
    fn bounds(&self) -> Bounds {
        Bounds {
            x0: self.x,
            y0: self.y,
            x1: self.x + self.w,
            y1: self.y + self.h,
        }
    }
}

/// 画布：像素 + “是否真正画了东西”标记（白底不算，对应 Dart `_didDraw`）。
pub(crate) struct Canvas {
    pixmap: Pixmap,
    pub drew: bool,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Option<Self> {
        Some(Self {
            pixmap: Pixmap::new(w, h)?,
            drew: false,
        })
    }

    fn full_rect(&self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: self.pixmap.width() as f32,
            h: self.pixmap.height() as f32,
        }
    }

    fn fill_rect(&mut self, r: Rect, argb: u32) {
        let Some(rect) = tiny_skia::Rect::from_xywh(r.x, r.y, r.w, r.h) else {
            return;
        };
        let mut paint = SkPaint::default();
        paint.set_color_rgba8(
            (argb >> 16) as u8,
            (argb >> 8) as u8,
            argb as u8,
            (argb >> 24) as u8,
        );
        paint.anti_alias = true;
        self.pixmap
            .fill_rect(rect, &paint, Transform::identity(), None);
    }

    /// 位图绘制到 dst（已由调用方做 contain）。大幅缩小时先用 image 的三角滤波
    /// 预缩放：tiny-skia 没有 mipmap，直接双三次缩小会有明显锯齿 / 摩尔纹。
    fn draw_image(&mut self, img: &RgbaImage, dst: Rect) {
        if img.width == 0 || img.height == 0 || dst.is_empty() {
            return;
        }
        let mut premul = img.data.clone();
        for px in premul.as_chunks_mut::<4>().0 {
            let a = px[3] as u16;
            for c in &mut px[..3] {
                *c = ((*c as u16 * a + 127) / 255) as u8;
            }
        }
        let (mut w, mut h) = (img.width, img.height);
        let tw = dst.w.round().max(1.0) as u32;
        let th = dst.h.round().max(1.0) as u32;
        if (tw as f32) < w as f32 * 0.75 {
            if let Some(buf) = image::RgbaImage::from_raw(w, h, premul.clone()) {
                let small =
                    image::imageops::resize(&buf, tw, th, image::imageops::FilterType::Triangle);
                premul = small.into_raw();
                // 预乘数据经滤波后理论上仍满足 c ≤ a，四舍五入可能越界，夹一下
                for px in premul.as_chunks_mut::<4>().0 {
                    let a = px[3];
                    for c in &mut px[..3] {
                        *c = (*c).min(a);
                    }
                }
                w = tw;
                h = th;
            }
        }
        let Some(size) = tiny_skia::IntSize::from_wh(w, h) else {
            return;
        };
        let Some(src) = Pixmap::from_vec(premul, size) else {
            return;
        };
        let ts = Transform::from_row(dst.w / w as f32, 0.0, 0.0, dst.h / h as f32, dst.x, dst.y);
        let paint = PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..Default::default()
        };
        self.pixmap
            .draw_pixmap(0, 0, src.as_ref(), &paint, ts, None);
        self.drew = true;
    }

    /// SVG 光栅化到 dst（SVG 尺寸非等比拉伸到 dst，与 Dart 的 canvas.scale 一致）。
    fn draw_svg(&mut self, svg_text: &str, dst: Rect) -> bool {
        let opt = resvg::usvg::Options::default();
        let tree = match resvg::usvg::Tree::from_str(svg_text, &opt) {
            Ok(t) => t,
            Err(e) => {
                log::debug!("icon: svg parse failed: {e}");
                return false;
            }
        };
        let size = tree.size();
        let ts = Transform::from_row(
            dst.w / size.width(),
            0.0,
            0.0,
            dst.h / size.height(),
            dst.x,
            dst.y,
        );
        resvg::render(&tree, ts, &mut self.pixmap.as_mut());
        self.drew = true;
        true
    }

    /// 把图层按圆角矩形遮罩合成到本画布。
    fn composite_masked(&mut self, layer: &Canvas, r: Rect, radius: f32) {
        let Some(mut mask) = Mask::new(self.pixmap.width(), self.pixmap.height()) else {
            return;
        };
        let Some(path) = rrect_path(r, radius) else {
            return;
        };
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        self.pixmap.draw_pixmap(
            0,
            0,
            layer.pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            Some(&mask),
        );
    }

    /// 转为非预乘 RGBA。
    pub fn into_rgba(self) -> RgbaImage {
        let (width, height) = (self.pixmap.width(), self.pixmap.height());
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for p in self.pixmap.pixels() {
            let c = p.demultiply();
            data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        RgbaImage {
            width,
            height,
            data,
        }
    }
}

/// 圆角矩形路径（三次贝塞尔近似四分之一圆）。
fn rrect_path(r: Rect, radius: f32) -> Option<tiny_skia::Path> {
    let rad = radius.clamp(0.0, r.w.min(r.h) / 2.0);
    let k = rad * 0.552_284_8;
    let (l, t, rt, b) = (r.x, r.y, r.x + r.w, r.y + r.h);
    let mut pb = PathBuilder::new();
    pb.move_to(l + rad, t);
    pb.line_to(rt - rad, t);
    pb.cubic_to(rt - rad + k, t, rt, t + rad - k, rt, t + rad);
    pb.line_to(rt, b - rad);
    pb.cubic_to(rt, b - rad + k, rt - rad + k, b, rt - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
    pb.finish()
}

/// 用 SVG arc 描述圆角矩形（四角半径可不同）。
fn rrect_d(r: Rect, radii: [f32; 4]) -> String {
    let max = r.w.min(r.h) / 2.0;
    let [tl, tr, br, bl] = radii.map(|v| v.clamp(0.0, max));
    let (l, t, rt, b) = (r.x, r.y, r.x + r.w, r.y + r.h);
    let mut d = String::new();
    let _ = write!(d, "M{} {}H{}", l + tl, t, rt - tr);
    if tr > 0.0 {
        let _ = write!(d, "A{tr} {tr} 0 0 1 {} {}", rt, t + tr);
    }
    let _ = write!(d, "V{}", b - br);
    if br > 0.0 {
        let _ = write!(d, "A{br} {br} 0 0 1 {} {}", rt - br, b);
    }
    let _ = write!(d, "H{}", l + bl);
    if bl > 0.0 {
        let _ = write!(d, "A{bl} {bl} 0 0 1 {} {}", l, b - bl);
    }
    let _ = write!(d, "V{}", t + tl);
    if tl > 0.0 {
        let _ = write!(d, "A{tl} {tl} 0 0 1 {} {}", l + tl, t);
    }
    d.push('Z');
    d
}

fn oval_d(r: Rect) -> String {
    let (rx, ry) = (r.w / 2.0, r.h / 2.0);
    let (cx, cy) = (r.x + rx, r.y + ry);
    format!(
        "M{} {cy}A{rx} {ry} 0 1 0 {} {cy}A{rx} {ry} 0 1 0 {} {cy}Z",
        cx - rx,
        cx + rx,
        cx - rx
    )
}

/// 渲染上下文：解析器 + dp 缩放系数。
pub(crate) struct Ctx<'r, 'a> {
    pub res: &'r mut Resolver<'a>,
    /// 每 dp 对应的像素数（画布 / 432）。
    pub dp: f32,
}

impl Ctx<'_, '_> {
    fn dim(&self, raw: Option<&str>) -> f32 {
        parse_dimension(raw, 0.0) * self.dp
    }

    /// 尺寸可能是百分比（相对 `basis`）。
    fn dim_or_fraction(&self, raw: Option<&str>, basis: f32, fallback: f32) -> f32 {
        if raw.is_none() {
            return fallback;
        }
        if color::is_fraction(raw) {
            return parse_dimension(raw, 0.0) / 100.0 * basis;
        }
        self.dim(raw)
    }

    /// 任意来源（颜色字面量 / zip 路径 / 资源引用）绘制到 rect。
    pub fn render_drawable(&mut self, canvas: &mut Canvas, rect: Rect, source: &str, depth: u32) {
        if depth > MAX_DEPTH || rect.is_empty() {
            return;
        }
        let t = source.trim();
        if t.is_empty() {
            return;
        }
        if is_color_literal(t) {
            if let Some(c) = color::parse_color(t) {
                canvas.fill_rect(rect, c);
                canvas.drew = true;
            }
            return;
        }
        if looks_like_zip_path(t) {
            self.render_drawable_file(canvas, rect, t, depth + 1);
            return;
        }
        if !t.starts_with('@') {
            let p = normalize_zip_path(t);
            if looks_like_zip_path(&p) {
                self.render_drawable_file(canvas, rect, &p, depth + 1);
            }
            return;
        }
        let Some(resolved) = self.res.resolve_reference(t, depth + 1) else {
            return;
        };
        if resolved != t {
            self.render_drawable(canvas, rect, &resolved, depth + 1);
        }
    }

    fn render_drawable_file(&mut self, canvas: &mut Canvas, rect: Rect, file: &str, depth: u32) {
        let n = normalize_zip_path(file);
        if n.is_empty() {
            return;
        }
        if is_bitmap_path(&n) || !is_xml_path(&n) {
            // 非 xml 的未知扩展名也按位图尝试（混淆包里可能没有扩展名）
            if let Some(img) = self.res.load_bitmap(&n) {
                let dst = rect.fit_contain(img.width as f32, img.height as f32);
                canvas.draw_image(&img, dst);
            }
            return;
        }
        let Some(root) = self.res.load_xml(&n) else {
            return;
        };
        self.render_element(canvas, rect, &root, depth + 1);
    }

    fn render_element(&mut self, canvas: &mut Canvas, rect: Rect, e: &XmlElement, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        match tag(e).as_str() {
            "adaptive-icon" | "monochrome" => self.render_adaptive(canvas, rect, e, depth + 1),
            "vector" => self.render_vector(canvas, rect, e),
            "layer-list" => self.render_layer_list(canvas, rect, e, depth + 1),
            "inset" => self.render_inset(canvas, rect, e, depth + 1),
            "shape" => self.render_shape(canvas, rect, e, depth + 1),
            "bitmap" | "nine-patch" => {
                if let Some(src) = attr(e, "src")
                    .or_else(|| attr(e, "drawable"))
                    .map(str::to_string)
                {
                    self.render_drawable(canvas, rect, &src, depth + 1);
                }
            }
            "selector" | "level-list" => {
                if let Some(item) = find_child(e, "item") {
                    self.render_container(canvas, rect, item, depth + 1);
                }
            }
            // ColorDrawable：Dart 版不支持，这里顺手补上
            "color" => {
                let raw = attr(e, "color").or_else(|| text(e)).map(str::to_string);
                if let Some(c) = self.res.resolve_color_ref(raw.as_deref(), depth + 1) {
                    canvas.fill_rect(rect, c);
                    canvas.drew = true;
                }
            }
            _ => {
                if let Some(d) = attr(e, "drawable").map(str::to_string) {
                    self.render_drawable(canvas, rect, &d, depth + 1);
                } else if let Some(child) = e.children.first() {
                    self.render_element(canvas, rect, child, depth + 1);
                }
            }
        }
    }

    /// adaptive-icon：圆角遮罩（半径 min*0.22）内先铺白底兜底（背景色解析失败时不至于
    /// 透明 / 发黑），再画 background，最后按缩放系数画 foreground（缺失时画 monochrome）。
    fn render_adaptive(&mut self, canvas: &mut Canvas, rect: Rect, root: &XmlElement, depth: u32) {
        let bg = find_child(root, "background");
        let fg = find_child(root, "foreground");
        let mono = find_child(root, "monochrome");
        let full = canvas.full_rect();
        let Some(mut layer) = Canvas::new(full.w as u32, full.h as u32) else {
            return;
        };
        layer.fill_rect(rect, WHITE);
        let scale = self.adaptive_fg_scale(fg.or(mono), depth + 1);
        let fg_rect = rect.scale_around_center(scale);
        if let Some(bg) = bg {
            self.render_container(&mut layer, rect, bg, depth + 1);
        }
        if let Some(fg) = fg.or(mono) {
            self.render_container(&mut layer, fg_rect, fg, depth + 1);
        }
        canvas.composite_masked(&layer, rect, rect.w.min(rect.h) * 0.22);
        canvas.drew |= layer.drew;
    }

    /// 前景是 vector 用 1.28，其它（位图等）用 1.60。
    fn adaptive_fg_scale(&mut self, container: Option<&XmlElement>, depth: u32) -> f32 {
        let Some(c) = container else {
            return FG_SCALE_BITMAP;
        };
        if depth > MAX_DEPTH {
            return FG_SCALE_BITMAP;
        }
        let raw = attr(c, "drawable")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| find_drawable_like_value(c));
        // 内联 <vector> 子元素：Dart 版在这里回退 1.60，但内联矢量与引用矢量应同等对待
        let Some(raw) = raw else {
            return if c.children.first().is_some_and(|ch| tag(ch) == "vector") {
                FG_SCALE_VECTOR
            } else {
                FG_SCALE_BITMAP
            };
        };
        let Some(path) = self.res.resolve_drawable_to_path(&raw, depth + 1) else {
            return FG_SCALE_BITMAP;
        };
        if is_xml_path(&path) {
            if let Some(root) = self.res.load_xml(&path) {
                if tag(&root) == "vector" {
                    return FG_SCALE_VECTOR;
                }
            }
        }
        FG_SCALE_BITMAP
    }

    fn render_container(&mut self, canvas: &mut Canvas, rect: Rect, c: &XmlElement, depth: u32) {
        if let Some(d) = attr(c, "drawable")
            .filter(|d| !d.is_empty())
            .map(str::to_string)
        {
            self.render_drawable(canvas, rect, &d, depth + 1);
            return;
        }
        if let Some(d) = find_drawable_like_value(c) {
            self.render_drawable(canvas, rect, &d, depth + 1);
            return;
        }
        if let Some(child) = c.children.first() {
            self.render_element(canvas, rect, child, depth + 1);
        }
    }

    /// inset 系列属性（`inset` + `insetLeft/Top/Right/Bottom`）。
    fn insets(&self, e: &XmlElement, rect: Rect) -> (f32, f32, f32, f32) {
        let all = self.dim_or_fraction(attr(e, "inset"), rect.w.min(rect.h), 0.0);
        let side = |n: &str, basis: f32| self.dim_or_fraction(attr(e, n), basis, all);
        (
            side("insetLeft", rect.w),
            side("insetTop", rect.h),
            side("insetRight", rect.w),
            side("insetBottom", rect.h),
        )
    }

    fn render_layer_list(
        &mut self,
        canvas: &mut Canvas,
        rect: Rect,
        root: &XmlElement,
        depth: u32,
    ) {
        for item in children_named(root, "item") {
            // layer-list 的 item 用 left/top/right/bottom（start/end）；Dart 版误读 inset*，
            // 这里两种都认，inset* 作为回退。
            let (il, it, ir, ib) = self.insets(item, rect);
            let l = self.dim_or_fraction(
                attr(item, "left").or_else(|| attr(item, "start")),
                rect.w,
                il,
            );
            let t = self.dim_or_fraction(attr(item, "top"), rect.h, it);
            let r = self.dim_or_fraction(
                attr(item, "right").or_else(|| attr(item, "end")),
                rect.w,
                ir,
            );
            let b = self.dim_or_fraction(attr(item, "bottom"), rect.h, ib);
            let inner = rect.inset(l, t, r, b);
            if inner.is_empty() {
                continue;
            }
            self.render_container(canvas, inner, item, depth + 1);
        }
    }

    fn render_inset(&mut self, canvas: &mut Canvas, rect: Rect, root: &XmlElement, depth: u32) {
        let (l, t, r, b) = self.insets(root, rect);
        let inner = rect.inset(l, t, r, b);
        if !inner.is_empty() {
            self.render_container(canvas, inner, root, depth + 1);
        }
    }

    /// `<shape>`：oval / rectangle（含 corners 圆角），solid 或 gradient 填充，stroke 描边。
    /// 生成 SVG 后交给 resvg，与 vector 共用渐变 / sweep 近似逻辑。
    fn render_shape(&mut self, canvas: &mut Canvas, rect: Rect, root: &XmlElement, depth: u32) {
        let oval = matches!(
            attr(root, "shape")
                .unwrap_or("rectangle")
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "oval" | "1"
        );
        let solid = find_child(root, "solid");
        let gradient = find_child(root, "gradient");
        let stroke = find_child(root, "stroke");
        let corners = find_child(root, "corners");

        let radius = self.dim(attr_opt(corners, "radius"));
        let corner = |n: &str| match attr_opt(corners, n) {
            Some(v) => parse_dimension(Some(v), 0.0) * self.dp,
            None => radius,
        };
        let radii = [
            corner("topLeftRadius"),
            corner("topRightRadius"),
            corner("bottomRightRadius"),
            corner("bottomLeftRadius"),
        ];
        let geom = |r: Rect, shrink: f32| -> String {
            if oval {
                oval_d(r)
            } else {
                rrect_d(r, radii.map(|v| (v - shrink).max(0.0)))
            }
        };

        let fill_paint = match gradient {
            Some(g) => {
                let dp = self.dp;
                let res = &mut *self.res;
                gradient_from_element(g, &mut |s| res.resolve_color_ref(Some(s), depth + 1)).map(
                    |mut spec| {
                        if !spec.radius_fraction {
                            spec.radius = spec.radius.map(|r| r * dp);
                        }
                        Paint::Gradient(spec)
                    },
                )
            }
            None => None,
        }
        .or_else(|| {
            let raw = attr_opt(solid, "color").map(str::to_string);
            self.res
                .resolve_color_ref(raw.as_deref(), depth + 1)
                .map(Paint::Color)
        });

        let stroke_info = stroke.and_then(|s| {
            let raw = attr(s, "color").map(str::to_string);
            let c = self.res.resolve_color_ref(raw.as_deref(), depth + 1)?;
            let w = self.dim(attr(s, "width"));
            (w > 0.0).then_some((c, w))
        });
        if fill_paint.is_none() && stroke_info.is_none() {
            return;
        }

        let full = canvas.full_rect();
        let mut body = String::new();
        let painted = {
            let mut b = SvgBuilder::new(&mut *self.res);
            if let Some(p) = &fill_paint {
                b.emit_fill(
                    &mut body,
                    &geom(rect, 0.0),
                    false,
                    p,
                    1.0,
                    Some(rect.bounds()),
                    GradientSpace::Shape,
                );
            }
            if let Some((c, w)) = stroke_info {
                let r = rect.inset(w / 2.0, w / 2.0, w / 2.0, w / 2.0);
                let style = StrokeStyle {
                    width: w,
                    ..Default::default()
                };
                b.emit_stroke(
                    &mut body,
                    &geom(r, w / 2.0),
                    &Paint::Color(c),
                    1.0,
                    &style,
                    Some(r.bounds()),
                    GradientSpace::Shape,
                );
            }
            let doc = b.document(full.w, full.h, &body);
            (b.painted, doc)
        };
        if painted.0 > 0 {
            canvas.draw_svg(&painted.1, full);
        }
    }

    /// VectorDrawable：生成 SVG（视口坐标）并拉伸到 rect。
    fn render_vector(&mut self, canvas: &mut Canvas, rect: Rect, vector: &XmlElement) {
        let (svg_text, painted) = svg::vector_document(&mut *self.res, vector);
        if painted > 0 {
            canvas.draw_svg(&svg_text, rect);
        }
    }
}

/// 渲染入口：白底（除非透明）→ 绘制 → 没有任何绘制则返回 `None`。
pub(crate) fn render(
    res: &mut Resolver,
    source: &str,
    size: u32,
    transparent_bg: bool,
) -> Option<RgbaImage> {
    let n = normalize_zip_path(source);
    if n.is_empty() || size == 0 {
        return None;
    }
    let mut canvas = Canvas::new(size, size)?;
    let full = canvas.full_rect();
    if !transparent_bg {
        // 白底：避免透明区域在深色主题下显示为黑色
        canvas.fill_rect(full, WHITE);
    }
    let mut ctx = Ctx {
        res,
        dp: size as f32 / BASE_CANVAS,
    };
    ctx.render_drawable(&mut canvas, full, &n, 0);
    if !canvas.drew {
        log::debug!("icon: nothing drawn for {n}");
        return None;
    }
    Some(canvas.into_rgba())
}
