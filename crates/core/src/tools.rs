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

/// 索引里一条归档的描述：URL（可能是相对路径）+ 可选的 SHA-1 校验值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRef {
    pub url: String,
    pub sha1: Option<String>,
}

/// 从 repository2-3.xml 文本里找出 platform-tools 的归档 URL（可能是相对路径）。
pub fn find_platform_tools_url(xml: &str) -> Option<String> {
    find_platform_tools_archive(xml).map(|a| a.url)
}

/// 同上，连同校验值一起返回。官方索引的 `<complete>` 里带 `<checksum type="sha1">`。
pub fn find_platform_tools_archive(xml: &str) -> Option<ArchiveRef> {
    let pkg_re = regex::Regex::new(
        r#"(?s)<remotePackage[^>]*path="platform-tools"[^>]*>(.*?)</remotePackage>"#,
    )
    .ok()?;
    let body = pkg_re.captures(xml)?.get(1)?.as_str();
    let archive_re = regex::Regex::new(r"(?s)<archive>(.*?)</archive>").ok()?;
    let os_re = regex::Regex::new(r"<host-os>\s*([a-z]+)\s*</host-os>").ok()?;
    let url_re = regex::Regex::new(r"<url>\s*([^<\s]+)\s*</url>").ok()?;
    let sum_re = regex::Regex::new(r"<checksum[^>]*>\s*([0-9a-fA-F]{40})\s*</checksum>").ok()?;
    let mut fallback = None;
    for cap in archive_re.captures_iter(body) {
        let a = cap.get(1)?.as_str();
        let os = os_re.captures(a).and_then(|c| c.get(1)).map(|m| m.as_str());
        let url = url_re
            .captures(a)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());
        let Some(url) = url else { continue };
        let sha1 = sum_re
            .captures(a)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_ascii_lowercase());
        let archive = ArchiveRef { url, sha1 };
        if os == Some(host_os()) {
            return Some(archive);
        }
        // 没有 host-os 标签时按文件名猜。
        let hint = match host_os() {
            "windows" => "-win",
            "macosx" => "-darwin",
            _ => "-linux",
        };
        if os.is_none() && archive.url.contains(hint) {
            fallback = Some(archive);
        }
    }
    fallback
}

/// 下载产物按索引给的 SHA-1 校验；索引没给时放行（只有自建的本地索引会没有）。
pub fn verify_sha1(bytes: &[u8], expected: Option<&str>) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    use sha1::Digest as _;
    let actual = hex::encode(sha1::Sha1::digest(bytes));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "sha1 mismatch: expected {} got {}",
            expected, actual
        ))
    }
}

pub(crate) fn fetch_bytes(
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
    // 连接 20s、整体 15 分钟：没有超时的话网络卡住时取消键也救不回来（read 一直阻塞）。
    let agent: ureq::Agent = ureq::config::Config::builder()
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_global(Some(std::time::Duration::from_secs(15 * 60)))
        .build()
        .new_agent();
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
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
            let Some(archive) = find_platform_tools_archive(&xml) else {
                last_err = format!("{}: platform-tools not found in index", index_url);
                continue;
            };
            let full_url = if archive.url.contains("://") {
                archive.url.clone()
            } else {
                resolve_relative(index_url, &archive.url)
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
            if let Err(e) = verify_sha1(&bytes, archive.sha1.as_deref()) {
                // 校验失败是硬错误：下载下来的是会被执行的 adb，不能换个源再试就算了。
                return Err(format!("{}: {}", full_url, e));
            }
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
    let mut zip = crate::zip::open_archive(cursor).map_err(|e| e.to_string())?;
    // 先解到旁边的临时目录，全部成功后再替换：中途失败不会留下半套 platform-tools。
    let final_target = dir.join("platform-tools");
    let target = dir.join(format!("platform-tools.tmp-{}", std::process::id()));
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
                // 只保留普通权限位：zip 里带 setuid / setgid 的条目不该原样落地
                let mode = mode & 0o755;
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
    if final_target.exists() {
        std::fs::remove_dir_all(&final_target).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&target, &final_target).map_err(|e| e.to_string())?;
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
        let archive = find_platform_tools_archive(xml).unwrap();
        let url = archive.url.clone();
        assert!(archive.sha1.is_none());
        assert!(verify_sha1(b"abc", None).is_ok());
        assert!(verify_sha1(b"abc", Some("a9993e364706816aba3e25717850c26c9cd0d89d")).is_ok());
        assert!(verify_sha1(b"abd", Some("a9993e364706816aba3e25717850c26c9cd0d89d")).is_err());
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
