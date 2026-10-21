//! APK 图标：候选收集、渲染（位图 / VectorDrawable / adaptive-icon / shape / layer-list）、
//! PNG 与 SVG 导出。移植自 Dart 版 `apk_info.dart`、`adaptive_icon_renderer.dart`、
//! `vector_to_svg.dart`。
//!
//! 为与 zip / 资源表实现解耦，文件访问通过 [`IconSource`] 注入，资源表以
//! `&BTreeMap<u32, ResourceInfo>` 传入（归档包等场景可以为空）。

mod candidates;
mod color;
mod export;
mod path;
mod render;
mod resolve;
mod svg;
mod xml;

use std::collections::BTreeMap;

use crate::model::ResourceInfo;

pub use candidates::{collect_candidates, IconCandidate, IconKind};

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
}

/// 解码 PNG / WebP / JPEG 为 RGBA（XAPK 的 `icon.png` 等直接用）。
pub fn decode_bitmap(bytes: &[u8]) -> Option<RgbaImage> {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            Some(RgbaImage {
                width,
                height,
                data: rgba.into_raw(),
            })
        }
        Err(e) => {
            log::debug!("icon: decode failed: {e}");
            None
        }
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
    let mut out = std::io::Cursor::new(Vec::new());
    if let Err(e) = buf.write_to(&mut out, image::ImageFormat::Png) {
        log::warn!("icon: encode_png failed: {e}");
        return Vec::new();
    }
    out.into_inner()
}

#[cfg(test)]
mod tests;
