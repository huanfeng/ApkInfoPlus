//! 外部工具查找与 platform-tools（adb）下载。

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const OFFICIAL_REPO: &str = "https://dl.google.com/android/repository/repository2-3.xml";
const MIRROR_REPO: &str =
    "https://github.com/huanfeng/apk_info_tool_binary/releases/download/latest/repository2-3.xml";

pub fn adb_file_name() -> &'static str {
    if cfg!(windows) {
        "adb.exe"
    } else {
        "adb"
    }
}

/// 下载目录里的 adb（`<dir>/platform-tools/adb`）。
pub fn downloaded_adb(download_dir: &Path) -> Option<PathBuf> {
    let p = download_dir.join("platform-tools").join(adb_file_name());
    p.is_file().then_some(p)
}

/// 在 PATH 里找可执行文件；Windows 按 PATHEXT 补扩展名。
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) && !name.contains('.') {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.BAT;.CMD".into())
            .split(';')
            .map(|s| s.to_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{}{}", name, ext));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// 默认下载源顺序：下载目录里的 `repository_mirrors.txt` → 镜像 → 官方。
pub fn default_download_sources() -> Vec<String> {
    vec![MIRROR_REPO.to_string(), OFFICIAL_REPO.to_string()]
}

/// 读用户自定义的额外源（每行一个 URL，`#` 注释）。
pub fn extra_download_sources(download_dir: &Path) -> Vec<String> {
    std::fs::read_to_string(download_dir.join("repository_mirrors.txt"))
        .map(|t| {
            t.lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Progress { stage: String, percent: Option<f32> },
    Done(PathBuf),
    Failed(String),
}

/// 当前平台在仓库 XML 里的 host-os 名。
fn host_os() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macosx"
    } else {
        "linux"
    }
}

/// 从 repository2-3.xml 文本里找出 platform-tools 的归档 URL（可能是相对路径）。
pub fn find_platform_tools_url(xml: &str) -> Option<String> {
    let pkg_re = regex::Regex::new(
        r#"(?s)<remotePackage[^>]*path="platform-tools"[^>]*>(.*?)</remotePackage>"#,
    )
    .ok()?;
    let body = pkg_re.captures(xml)?.get(1)?.as_str();
    let archive_re = regex::Regex::new(r"(?s)<archive>(.*?)</archive>").ok()?;
    let os_re = regex::Regex::new(r"<host-os>\s*([a-z]+)\s*</host-os>").ok()?;
    let url_re = regex::Regex::new(r"<url>\s*([^<\s]+)\s*</url>").ok()?;
    let mut fallback = None;
    for cap in archive_re.captures_iter(body) {
        let a = cap.get(1)?.as_str();
        let os = os_re.captures(a).and_then(|c| c.get(1)).map(|m| m.as_str());
        let url = url_re
            .captures(a)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());
        let Some(url) = url else { continue };
        if os == Some(host_os()) {
            return Some(url);
        }
        // 没有 host-os 标签时按文件名猜。
        let hint = match host_os() {
            "windows" => "-win",
            "macosx" => "-darwin",
            _ => "-linux",
        };
        if os.is_none() && url.contains(hint) {
            fallback = Some(url);
        }
    }
    fallback
}

fn fetch_bytes(
    url: &str,
    cancel: &Arc<AtomicBool>,
    mut progress: impl FnMut(Option<f32>),
) -> Result<Vec<u8>, String> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(path).map_err(|e| e.to_string());
    }
    if !url.contains("://") && Path::new(url).is_file() {
        return std::fs::read(url).map_err(|e| e.to_string());
    }
    let resp = ureq::get(url).call().map_err(|e| e.to_string())?;
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let mut reader = resp.into_body().into_reader();
    let mut out = Vec::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        progress(total.map(|t| (out.len() as f32 / t as f32).min(1.0)));
    }
    Ok(out)
}

/// 下载并解压 platform-tools 到 `<dir>/platform-tools/`。
pub fn download_platform_tools(
    dir: &Path,
    sources: &[String],
    cancel: Arc<AtomicBool>,
    emit: impl Fn(DownloadEvent),
) {
    let result = (|| -> Result<PathBuf, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut all_sources = extra_download_sources(dir);
        all_sources.extend(sources.iter().cloned());
        if all_sources.is_empty() {
            all_sources = default_download_sources();
        }

        let mut last_err = String::from("no sources");
        for index_url in &all_sources {
            emit(DownloadEvent::Progress {
                stage: "repository".into(),
                percent: None,
            });
            let xml = match fetch_bytes(index_url, &cancel, |_| {}) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => {
                    last_err = format!("{}: {}", index_url, e);
                    if cancel.load(Ordering::SeqCst) {
                        return Err("cancelled".into());
                    }
                    continue;
                }
            };
            let Some(archive_url) = find_platform_tools_url(&xml) else {
                last_err = format!("{}: platform-tools not found in index", index_url);
                continue;
            };
            let full_url = if archive_url.contains("://") {
                archive_url
            } else {
                resolve_relative(index_url, &archive_url)
            };
            emit(DownloadEvent::Progress {
                stage: "platform-tools".into(),
                percent: Some(0.0),
            });
            let bytes = match fetch_bytes(&full_url, &cancel, |p| {
                emit(DownloadEvent::Progress {
                    stage: "platform-tools".into(),
                    percent: p,
                })
            }) {
                Ok(b) => b,
                Err(e) => {
                    last_err = format!("{}: {}", full_url, e);
                    if cancel.load(Ordering::SeqCst) {
                        return Err("cancelled".into());
                    }
                    continue;
                }
            };
            emit(DownloadEvent::Progress {
                stage: "extract".into(),
                percent: None,
            });
            extract_platform_tools(&bytes, dir)?;
            let adb = downloaded_adb(dir).ok_or_else(|| "adb missing after extract".to_string())?;
            return Ok(adb);
        }
        Err(last_err)
    })();
    match result {
        Ok(p) => emit(DownloadEvent::Done(p)),
        Err(e) => emit(DownloadEvent::Failed(e)),
    }
}

/// `base` 的目录 + 相对路径。
fn resolve_relative(base: &str, rel: &str) -> String {
    match base.rfind('/') {
        Some(i) => format!("{}/{}", &base[..i], rel),
        None => rel.to_string(),
    }
}

fn extract_platform_tools(bytes: &[u8], dir: &Path) -> Result<(), String> {
    let cursor = io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(cursor).map_err(|e| e.to_string())?;
    let target = dir.join("platform-tools");
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().map_err(|e| e.to_string())?.replace('\\', "/");
        let rel = name.strip_prefix("platform-tools/").unwrap_or(&name);
        if rel.is_empty() {
            continue;
        }
        let Some(dest) = crate::installer::safe_join(&target, rel) else {
            continue;
        };
        if entry.is_dir() || name.ends_with('/') {
            std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut f = File::create(&dest).map_err(|e| e.to_string())?;
        io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        f.flush().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = entry.unix_mode() {
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode));
            } else if dest.extension().is_none() {
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let adb = target.join("adb");
        if adb.is_file() {
            let _ = std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

pub fn delete_platform_tools(dir: &Path) -> Result<(), String> {
    let target = dir.join("platform-tools");
    if target.is_dir() {
        std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_repository_index() {
        let xml = r#"<sdk:sdk-repository>
<remotePackage path="build-tools;34.0.0"><archives><archive><host-os>linux</host-os><complete><url>build-tools_r34-linux.zip</url></complete></archive></archives></remotePackage>
<remotePackage path="platform-tools"><archives>
<archive><complete><url>platform-tools_r35.0.2-darwin.zip</url></complete><host-os>macosx</host-os></archive>
<archive><complete><url>platform-tools_r35.0.2-linux.zip</url></complete><host-os>linux</host-os></archive>
<archive><complete><url>platform-tools_r35.0.2-win.zip</url></complete><host-os>windows</host-os></archive>
</archives></remotePackage></sdk:sdk-repository>"#;
        let url = find_platform_tools_url(xml).unwrap();
        let expect = match host_os() {
            "windows" => "platform-tools_r35.0.2-win.zip",
            "macosx" => "platform-tools_r35.0.2-darwin.zip",
            _ => "platform-tools_r35.0.2-linux.zip",
        };
        assert_eq!(url, expect);
        assert_eq!(
            resolve_relative("https://x/y/repository2-3.xml", "a.zip"),
            "https://x/y/a.zip"
        );
    }
}

#[cfg(test)]
mod manual {
    //! 联网对照：`APKINFO_DL_DIR=/tmp/x cargo test -p apkinfo-core download_dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn download_dump() {
        let Ok(dir) = std::env::var("APKINFO_DL_DIR") else {
            return;
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        super::download_platform_tools(
            std::path::Path::new(&dir),
            &super::default_download_sources(),
            cancel,
            |ev| match ev {
                super::DownloadEvent::Progress { stage, percent } => {
                    if percent
                        .map(|p| ((p * 100.0) as u32).is_multiple_of(25))
                        .unwrap_or(true)
                    {
                        println!("{stage} {percent:?}")
                    }
                }
                other => println!("{other:?}"),
            },
        );
    }
}
