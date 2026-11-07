//! APK 图标：候选收集、渲染（位图 / VectorDrawable / adaptive-icon / shape / layer-list）、
//! PNG 与 SVG 导出。移植自 Dart 版 `apk_info.dart`、`adaptive_icon_renderer.dart`、
//! `vector_to_svg.dart`。
//!
//! 为与 zip / 资源表实现解耦，文件访问通过 [`IconSource`] 注入，资源表以
//! `&BTreeMap<u32, ResourceInfo>` 传入（归档包等场景可以为空）。

mod candidates;
mod color;
mod export;
mod original;
mod path;
mod render;
mod resolve;
mod svg;
mod xml;

use std::collections::BTreeMap;

use crate::model::ResourceInfo;

pub use candidates::{banner_candidates, collect_candidates, IconCandidate, IconKind};
pub use original::{zip_files, OriginalFile};

/// 图标文件访问。
pub trait IconSource {
    /// 读取 zip 条目（调用方已实现大小写不敏感等回退）。
    fn read(&mut self, path: &str) -> Option<Vec<u8>>;
    /// zip 内全部文件名。
    fn file_names(&mut self) -> Vec<String>;
}

/// 非预乘 RGBA8 图像，交给 UI 的统一格式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// 行优先，每像素 4 字节。
    pub data: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum IconError {
    #[error("icon file not found: {0}")]
    NotFound(String),
    #[error("icon cannot be decoded: {0}")]
    Decode(String),
    #[error("nothing could be rendered for: {0}")]
    NothingDrawn(String),
    #[error("svg export not supported for: {0}")]
    Unsupported(String),
}

/// 默认渲染尺寸（与 Dart 版画布一致）。
pub const DEFAULT_ICON_SIZE: u32 = 432;

/// 图标渲染器：持有文件访问、资源表索引与 XML / 位图缓存，
/// 同一 APK 的多次渲染应复用同一个实例。
pub struct IconRenderer<'a> {
    res: resolve::Resolver<'a>,
}

impl<'a> IconRenderer<'a> {
    pub fn new(source: &'a mut dyn IconSource, resources: &BTreeMap<u32, ResourceInfo>) -> Self {
        Self {
            res: resolve::Resolver::new(source, resources),
        }
    }

    /// 渲染候选。位图候选按原始分辨率解码返回（与 Dart 一致，便于无损导出），
    /// `size` / `transparent_bg` 只作用于 XML 候选与资源引用。
    pub fn render_candidate(
        &mut self,
        c: &IconCandidate,
        size: u32,
        transparent_bg: bool,
    ) -> Result<RgbaImage, IconError> {
        if c.kind == IconKind::Bitmap || resolve::is_bitmap_path(&c.path) {
            if let Some(img) = self.res.load_bitmap(&c.path) {
                return Ok((*img).clone());
            }
        }
        self.render_path(&c.path, size, transparent_bg)
    }

    /// 渲染 zip 路径或资源引用（`@res/0x...`、`@mipmap/ic_launcher`）到 size×size 画布。
    /// 非透明模式先铺白底；adaptive-icon 在圆角遮罩内始终保留白底兜底。
    pub fn render_path(
        &mut self,
        path_or_ref: &str,
        size: u32,
        transparent_bg: bool,
    ) -> Result<RgbaImage, IconError> {
        render::render(&mut self.res, path_or_ref, size, transparent_bg)
            .ok_or_else(|| IconError::NothingDrawn(path_or_ref.to_string()))
    }

    /// 导出 SVG（仅 XML 候选）：adaptive-icon 输出带圆角裁剪的完整分层，否则输出单个 vector。
    pub fn export_svg(&mut self, c: &IconCandidate) -> Result<String, IconError> {
        if c.kind != IconKind::XmlVector {
            return Err(IconError::Unsupported(c.path.clone()));
        }
        export::export_svg(&mut self.res, &c.path)
            .ok_or_else(|| IconError::Unsupported(c.path.clone()))
    }

    /// 候选文件的原始字节（导出原图用）。
    pub fn raw_bytes(&mut self, c: &IconCandidate) -> Option<Vec<u8>> {
        self.res.read_file(&c.path)
    }

    /// 填写候选的导出能力与尺寸信息（见 [`IconCandidate`] 的对应字段），并把尺寸写进标签。
    pub fn annotate(&mut self, c: &mut IconCandidate) {
        let bytes = self.res.read_file(&c.path);
        c.file_size = bytes.as_ref().map(|b| b.len() as u64);
        if c.kind == IconKind::Bitmap {
            c.pixel_size = bytes.as_deref().and_then(bitmap_dimensions);
        } else {
            (c.svg_exportable, c.native_size) = export::inspect(&mut self.res, &c.path);
            c.original_file_count = original::original_paths(&mut self.res, &c.path).len();
        }
        c.append_details();
    }

    /// 「导出原始文件」的内容：位图原样返回；XML 反编译为文本；
    /// 引用了其它文件（adaptive-icon 的前景 / 背景等）时连同它们一起打成 zip。
    pub fn export_original(&mut self, c: &IconCandidate) -> Option<Vec<u8>> {
        if c.kind == IconKind::Bitmap {
            return self.res.read_file(&c.path);
        }
        let mut files = original::original_files(&mut self.res, &c.path);
        match files.len() {
            0 => None,
            1 => files.pop().map(|f| f.bytes),
            _ => zip_files(&files)
                .map_err(|e| log::warn!("icon: zip original files failed: {e}"))
                .ok(),
        }
    }
}

/// 只读文件头取位图尺寸，不解码像素。
pub(crate) fn bitmap_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// 导出 PNG：源文件本身就是 PNG 的位图候选直接返回原字节（无损且不变大），
/// 其余渲染后编码；位图与由位图组成的 XML 候选按自身分辨率渲染，矢量用 `default_size`。
pub fn export_png_bytes(
    c: &IconCandidate,
    raw: Option<Vec<u8>>,
    default_size: u32,
    render: impl FnOnce(u32) -> Option<RgbaImage>,
) -> Option<Vec<u8>> {
    if c.kind == IconKind::Bitmap {
        if let Some(bytes) = raw.filter(|b| b.starts_with(PNG_MAGIC)) {
            return Some(bytes);
        }
    }
    let own = c.native_size.or(c.pixel_size.map(|(w, h)| w.max(h)));
    let img = render(own.unwrap_or(default_size))?;
    let png = encode_png(&img);
    (!png.is_empty()).then_some(png)
}

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// 解码 PNG / WebP / JPEG 为 RGBA（XAPK 的 `icon.png` 等直接用）。
pub fn decode_bitmap(bytes: &[u8]) -> Option<RgbaImage> {
    // 归档包外层的 icon.png 可以是任意大的图：按边长封顶，而不只是 image 默认的分配上限。
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    let decoded = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|mut r| {
            r.limits(limits);
            r.decode()
                .map_err(|e| log::debug!("icon: decode failed: {e}"))
                .ok()
        });
    match decoded.ok_or(()) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            Some(RgbaImage {
                width,
                height,
                data: rgba.into_raw(),
            })
        }
        Err(()) => None,
    }
}

/// 编码为 PNG。尺寸与数据长度不符时返回空 Vec（并记录日志）。
pub fn encode_png(img: &RgbaImage) -> Vec<u8> {
    let Some(buf) = image::RgbaImage::from_raw(img.width, img.height, img.data.clone()) else {
        log::warn!(
            "icon: encode_png size mismatch {}x{} len={}",
            img.width,
            img.height,
            img.data.len()
        );
        return Vec::new();
    };
    // 最高压缩 + 自适应滤波：导出文件不至于比 APK 里的原图大出几倍（默认档位偏向速度）。
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::ImageEncoder as _;
    let mut out = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);
    if let Err(e) = encoder.write_image(
        buf.as_raw(),
        img.width,
        img.height,
        image::ExtendedColorType::Rgba8,
    ) {
        log::warn!("icon: encode_png failed: {e}");
        return Vec::new();
    }
    out
}

#[cfg(test)]
mod tests;
