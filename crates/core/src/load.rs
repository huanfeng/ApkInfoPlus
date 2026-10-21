//! 一站式加载：文件路径 → [`Loaded`]（界面模型 + 图标候选 + 可继续渲染图标的会话）。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::apk::ApkReader;
use crate::archive;
use crate::icon::{
    self, collect_candidates, IconCandidate, IconKind, IconRenderer, IconSource, RgbaImage,
};
use crate::info::{ApkInfo, ArchiveKind};
use crate::model::ResourceInfo;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Zip(String),
    #[error("AndroidManifest.xml not found or unreadable")]
    NoManifest,
    #[error("archive contains no base apk")]
    NoBaseApk,
    #[error("{0}")]
    Other(String),
}

/// 归档包外层附带的图标条目名（伪路径，`IconSession::read` 识别）。
pub const ARCHIVE_ICON_PATH: &str = "__archive__/icon.png";

/// 解析后仍然需要的一切：打开着的 APK（zip）+ 资源表索引，供切换图标 / 导出时复用。
pub struct IconSession {
    reader: ApkReader,
    /// Arc：渲染时要同时借 `&mut self`（作为 IconSource）与资源表，克隆句柄绕开借用冲突。
    resources: Arc<BTreeMap<u32, ResourceInfo>>,
    /// 归档包的外层图标（manifest.icon / icon.png）。
    archive_icon: Option<Vec<u8>>,
}

impl IconSource for IconSession {
    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        if path == ARCHIVE_ICON_PATH {
            return self.archive_icon.clone();
        }
        self.reader.read_file(path)
    }
    fn file_names(&mut self) -> Vec<String> {
        self.reader.list_files()
    }
}

impl IconSession {
    pub fn render(
        &mut self,
        c: &IconCandidate,
        size: u32,
        transparent_bg: bool,
    ) -> Result<RgbaImage, String> {
        if c.path == ARCHIVE_ICON_PATH {
            return self
                .archive_icon
                .as_deref()
                .and_then(icon::decode_bitmap)
                .ok_or_else(|| "archive icon cannot be decoded".to_string());
        }
        let (resources, me) = self.renderer();
        let mut renderer = IconRenderer::new(me, &resources);
        renderer
            .render_candidate(c, size, transparent_bg)
            .map_err(|e| e.to_string())
    }

    pub fn export_svg(&mut self, c: &IconCandidate) -> Result<String, String> {
        let (resources, me) = self.renderer();
        let mut renderer = IconRenderer::new(me, &resources);
        renderer.export_svg(c).map_err(|e| e.to_string())
    }

    pub fn raw_bytes(&mut self, c: &IconCandidate) -> Option<Vec<u8>> {
        self.read(&c.path)
    }

    pub fn resources(&self) -> &BTreeMap<u32, ResourceInfo> {
        &self.resources
    }

    fn renderer(&mut self) -> (Arc<BTreeMap<u32, ResourceInfo>>, &mut Self) {
        (self.resources.clone(), self)
    }
}

pub struct Loaded {
    pub info: ApkInfo,
    pub candidates: Vec<IconCandidate>,
    pub session: IconSession,
}

pub fn load(path: &Path) -> Result<Loaded, LoadError> {
    let size = std::fs::metadata(path)?.len();
    let kind = ArchiveKind::from_path(path);
    if kind.is_archive() {
        load_archive(path, size, kind)
    } else {
        load_apk(path, size)
    }
}

fn load_apk(path: &Path, size: u64) -> Result<Loaded, LoadError> {
    let reader = ApkReader::open(path).map_err(|e| LoadError::Zip(e.to_string()))?;
    finish(path, size, ArchiveKind::Apk, reader, None)
}

fn load_archive(path: &Path, size: u64, kind: ArchiveKind) -> Result<Loaded, LoadError> {
    let mut contents = archive::read_archive(path).map_err(LoadError::Zip)?;
    let kind = archive::effective_kind(kind, &contents).map_err(LoadError::Other)?;
    if kind == ArchiveKind::Apk {
        // 改了扩展名的 APK（zip 根目录就是 AndroidManifest.xml）。
        let reader = ApkReader::open(path).map_err(|e| LoadError::Zip(e.to_string()))?;
        return finish(path, size, ArchiveKind::Apk, reader, None);
    }
    // take 而不是 clone：base.apk 可以有上百 MB，解析期不该在内存里放两份。
    let base_bytes = contents.base_bytes.take().ok_or(LoadError::NoBaseApk)?;
    let reader = ApkReader::from_bytes(base_bytes).map_err(|e| LoadError::Zip(e.to_string()))?;
    finish(path, size, kind, reader, Some(contents))
}

fn finish(
    path: &Path,
    size: u64,
    kind: ArchiveKind,
    mut reader: ApkReader,
    contents: Option<archive::ArchiveContents>,
) -> Result<Loaded, LoadError> {
    let mut meta = match reader.parse() {
        Some(m) => m,
        None if contents
            .as_ref()
            .and_then(|c| c.manifest.as_ref())
            .is_some() =>
        {
            Default::default()
        }
        None => return Err(LoadError::NoManifest),
    };
    // 只留图标解析用得着的条目：drawable / mipmap / color 这几类，以及值里带 res/ 路径或引用的。
    // Chrome 这种带几十种语言的包，全量资源表（每条翻译都是一个 String）要占几十 MB。
    let resources = Arc::new(icon_resources(reader.resources_with(icon_string)));
    reader.release_resource_table();

    // 主图标：资源表里 density 最高的那条；没有资源表时退回原始引用值。
    let main_icon_path = meta
        .icon_paths
        .iter()
        .max_by_key(|(d, _)| **d)
        .map(|(_, p)| p.clone())
        .or_else(|| meta.application_icon.clone())
        .or_else(|| {
            meta.launchable_activities
                .iter()
                .find_map(|a| a.icon.clone())
        });

    let mut info;
    let mut archive_icon = None;
    match contents {
        Some(c) => {
            if let Some(m) = &c.manifest {
                archive::apply_manifest(&mut meta, m);
            }
            let split_names: Vec<String> = c.apks.iter().map(|(n, _)| n.clone()).collect();
            let (locales, abis) = archive::infer_from_splits(&split_names);
            let locales_placeholder = meta.locales.iter().all(|l| l == "--_--" || l == "--");
            if (meta.locales.is_empty() || locales_placeholder) && !locales.is_empty() {
                meta.locales = locales;
            }
            if meta.native_codes.is_empty() && !abis.is_empty() {
                meta.native_codes = abis;
            }
            info = ApkInfo::new(path.to_path_buf(), size, kind, meta);
            info.archive_app_name = c.manifest.as_ref().and_then(|m| m.name.clone());
            info.split_configs = c
                .manifest
                .as_ref()
                .map(|m| m.split_configs.clone())
                .unwrap_or_default();
            info.splits = archive::build_splits(&c.apks);
            info.obbs = archive::build_obbs(&c.obbs);
            info.total_size = c
                .manifest
                .as_ref()
                .and_then(|m| m.total_size)
                .or(Some(size));
            info.base_apk_entry = c.base_entry.clone();
            archive_icon = c.icon_bytes;
        }
        None => {
            info = ApkInfo::new(path.to_path_buf(), size, kind, meta);
        }
    }
    info.main_icon_path = main_icon_path.clone();
    info.refresh_text_info();

    let mut session = IconSession {
        reader,
        resources,
        archive_icon,
    };
    let icons = info.meta.icon_paths.clone();
    let mut candidates = {
        let (resources, me) = session.renderer();
        collect_candidates(me, &resources, main_icon_path.as_deref(), &icons)
    };
    if session.archive_icon.is_some() {
        // 归档包自带的图标优先展示：它是发布者挑好的最终样子。
        candidates.insert(
            0,
            IconCandidate {
                path: ARCHIVE_ICON_PATH.to_string(),
                kind: IconKind::Bitmap,
                dpi: None,
                dpi_label: None,
                display_label: format!("PNG icon ({})", kind.display_name()),
            },
        );
    }
    Ok(Loaded {
        info,
        candidates,
        session,
    })
}

/// 图标解析用得着的字符串值：drawable / mipmap 的全收（文件路径），其余类型只收
/// res/ 路径与颜色字面量——翻译文本等在汇总资源时就丢掉。
fn icon_string(type_name: &str, value: &str) -> bool {
    matches!(type_name, "drawable" | "mipmap")
        || value.starts_with("res/")
        || value.starts_with('#')
}

/// 过滤给图标渲染器的资源表，见 `finish` 里的说明。
fn icon_resources(all: BTreeMap<u32, ResourceInfo>) -> BTreeMap<u32, ResourceInfo> {
    all.into_iter()
        .filter(|(_, r)| {
            matches!(
                r.type_name.as_str(),
                "drawable"
                    | "mipmap"
                    | "color"
                    | "attr"
                    | "xml"
                    | "raw"
                    | "anim"
                    | "animator"
                    | "interpolator"
            ) || r.file_paths.iter().any(|p| p.starts_with("res/"))
                || !r.references.is_empty()
        })
        .collect()
}
