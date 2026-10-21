//! 外部工具（adb）的路径解析：把配置里的「来源 + 自定义路径 + 下载目录」交给 core 解析。

use std::path::PathBuf;

use crate::config::ToolSource;
use crate::state::AppState;

/// 按配置解析 adb 可执行文件；找不到返回 `None`（不回退裸命令名，避免给用户一个假路径）。
pub fn resolve_adb(state: &AppState) -> Option<PathBuf> {
    state.cfg(|c| resolve_adb_with(c.adb_source, &c.adb_path, &c.resolved_download_dir()))
}

pub fn resolve_adb_with(
    source: ToolSource,
    custom: &str,
    download_dir: &std::path::Path,
) -> Option<PathBuf> {
    let downloaded = apkinfo_core::tools::downloaded_adb(download_dir);
    let system = apkinfo_core::tools::find_in_path("adb");
    let custom_path = {
        let p = PathBuf::from(custom.trim());
        if !custom.trim().is_empty() && p.is_file() {
            Some(p)
        } else {
            None
        }
    };
    match source {
        ToolSource::Builtin => downloaded.or(system).or(custom_path),
        ToolSource::System => system.or(downloaded).or(custom_path),
        ToolSource::Custom => custom_path.or(system).or(downloaded),
    }
}

/// 自动检测：哪个来源有可用的 adb 就选哪个（下载目录 → 系统 → 自定义）。
pub fn auto_detect_source(custom: &str, download_dir: &std::path::Path) -> ToolSource {
    if apkinfo_core::tools::downloaded_adb(download_dir).is_some() {
        ToolSource::Builtin
    } else if apkinfo_core::tools::find_in_path("adb").is_some() {
        ToolSource::System
    } else if !custom.trim().is_empty() && std::path::Path::new(custom.trim()).is_file() {
        ToolSource::Custom
    } else {
        ToolSource::Builtin
    }
}
