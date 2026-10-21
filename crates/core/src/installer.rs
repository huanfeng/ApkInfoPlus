//! 安装流程：普通 APK 直接 `install`；归档包解压选中分包后 `install-multiple`，再推送 OBB。
//!
//! 在调用方的后台线程里同步运行，进度经 `emit` 回调送出；`cancel` 置位后在每个步骤间停下。

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::adb::{self, InstallOptions};
use crate::info::{is_base_split, split_stem, ArchiveKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    Installing,
    PushingObb,
    Success,
    Failed,
    Stopped,
}

#[derive(Debug, Clone)]
pub enum InstallEvent {
    Device {
        device: String,
        phase: InstallPhase,
        message: String,
    },
    Log(String),
    Finished {
        success: usize,
        failed: usize,
    },
}

#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub adb: PathBuf,
    pub file: PathBuf,
    pub kind: ArchiveKind,
    pub devices: Vec<String>,
    pub options: InstallOptions,
    /// 归档包：要安装的分包（zip 内路径）；`None` 表示全部。
    pub selected_splits: Option<Vec<String>>,
    /// 已解析出的包名，用于 OBB 目标路径。
    pub package_name: Option<String>,
}

/// 临时目录守卫：离开作用域即删除。
struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), nanos));
        std::fs::create_dir_all(&p)?;
        Ok(Self(p))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn run(req: InstallRequest, cancel: Arc<AtomicBool>, emit: impl Fn(InstallEvent)) {
    let mut success = 0;
    let mut failed = 0;

    let prepared = if req.kind.is_archive() {
        match prepare_archive(&req, &emit) {
            Ok(p) => Some(p),
            Err(e) => {
                emit(InstallEvent::Log(e.clone()));
                for d in &req.devices {
                    emit(InstallEvent::Device {
                        device: d.clone(),
                        phase: InstallPhase::Failed,
                        message: e.clone(),
                    });
                }
                emit(InstallEvent::Finished {
                    success: 0,
                    failed: req.devices.len(),
                });
                return;
            }
        }
    } else {
        None
    };

    for device in &req.devices {
        if cancel.load(Ordering::SeqCst) {
            emit(InstallEvent::Device {
                device: device.clone(),
                phase: InstallPhase::Stopped,
                message: String::new(),
            });
            continue;
        }
        emit(InstallEvent::Device {
            device: device.clone(),
            phase: InstallPhase::Installing,
            message: String::new(),
        });
        let result = match &prepared {
            None => install_single(&req, device, &cancel, &emit),
            Some(p) => install_archive(&req, p, device, &cancel, &emit),
        };
        match result {
            Ok(()) => {
                success += 1;
                emit(InstallEvent::Device {
                    device: device.clone(),
                    phase: InstallPhase::Success,
                    message: String::new(),
                });
            }
            Err(Stop::Cancelled) => {
                emit(InstallEvent::Device {
                    device: device.clone(),
                    phase: InstallPhase::Stopped,
                    message: String::new(),
                });
                break;
            }
            Err(Stop::Failed(msg)) => {
                failed += 1;
                emit(InstallEvent::Device {
                    device: device.clone(),
                    phase: InstallPhase::Failed,
                    message: msg,
                });
            }
        }
    }
    emit(InstallEvent::Finished { success, failed });
}

enum Stop {
    Cancelled,
    Failed(String),
}

fn check(out: adb::Output) -> Result<adb::Output, Stop> {
    if out.cancelled {
        return Err(Stop::Cancelled);
    }
    if let Some(reason) = out.failure_reason() {
        return Err(Stop::Failed(reason));
    }
    Ok(out)
}

fn install_single(
    req: &InstallRequest,
    device: &str,
    cancel: &Arc<AtomicBool>,
    emit: &impl Fn(InstallEvent),
) -> Result<(), Stop> {
    let args = adb::install_args(device, &req.options, &req.file);
    emit(InstallEvent::Log(format!("adb {}", args.join(" "))));
    let out = adb::run(&req.adb, &args, cancel).map_err(Stop::Failed)?;
    log_output(&out, emit);
    check(out).map(|_| ())
}

/// 归档包解压后的素材。
struct Prepared {
    _dir: TempDir,
    apks: Vec<PathBuf>,
    /// (本地路径, 设备路径)
    obbs: Vec<(PathBuf, String)>,
}

fn prepare_archive(req: &InstallRequest, emit: &impl Fn(InstallEvent)) -> Result<Prepared, String> {
    let file = File::open(&req.file).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let names: Vec<String> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|c| c.into_owned()))
        .collect();

    // manifest.json：包名与 OBB 安装路径映射。
    let manifest: Option<serde_json::Value> = ["manifest.json", "info.json", "meta.json"]
        .iter()
        .find_map(|n| {
            let mut f = zip.by_name(n).ok()?;
            let mut s = String::new();
            io::Read::read_to_string(&mut f, &mut s).ok()?;
            serde_json::from_str(&s).ok()
        });

    let mut apks: Vec<String> = match &req.selected_splits {
        Some(sel) if !sel.is_empty() => sel.clone(),
        _ => names
            .iter()
            .filter(|n| n.to_lowercase().ends_with(".apk"))
            .cloned()
            .collect(),
    };
    if apks.is_empty() {
        return Err("no apk entries selected".into());
    }
    // base 排最前，其余按路径排序：install-multiple 对顺序不敏感，但日志更好读。
    apks.sort_by(|a, b| {
        let ab = is_base_split(&split_stem(a).to_lowercase());
        let bb = is_base_split(&split_stem(b).to_lowercase());
        bb.cmp(&ab).then_with(|| a.cmp(b))
    });

    let dir = TempDir::new("apkinfoplus-install").map_err(|e| e.to_string())?;
    let mut local_apks = Vec::new();
    for name in &apks {
        let dest =
            safe_join(&dir.0, name).ok_or_else(|| format!("unsafe path in archive: {}", name))?;
        extract_entry(&mut zip, name, &dest)?;
        emit(InstallEvent::Log(format!("extracted {}", name)));
        local_apks.push(dest);
    }

    // OBB：全部条目（不受分包勾选影响，与原版一致）。
    let package = req.package_name.clone().or_else(|| {
        manifest
            .as_ref()
            .and_then(|m| m.get("package_name").or_else(|| m.get("packageName")))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });
    let expansion_paths = expansion_paths(manifest.as_ref());
    let mut obbs = Vec::new();
    for name in names.iter().filter(|n| n.to_lowercase().ends_with(".obb")) {
        let Some(remote) = obb_remote_path(name, package.as_deref(), &expansion_paths) else {
            emit(InstallEvent::Log(format!(
                "skip obb (no target path): {}",
                name
            )));
            continue;
        };
        let dest =
            safe_join(&dir.0, name).ok_or_else(|| format!("unsafe path in archive: {}", name))?;
        extract_entry(&mut zip, name, &dest)?;
        obbs.push((dest, remote));
    }

    Ok(Prepared {
        _dir: dir,
        apks: local_apks,
        obbs,
    })
}

/// manifest 的 `expansions` / `obb_files`：`basename(install_path)` → 设备路径。
fn expansion_paths(manifest: Option<&serde_json::Value>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some(m) = manifest else { return map };
    let list = m.get("expansions").or_else(|| m.get("obb_files"));
    if let Some(arr) = list.and_then(|v| v.as_array()) {
        for item in arr {
            let path = item
                .get("install_path")
                .or_else(|| item.get("path"))
                .or_else(|| item.get("installPath"))
                .and_then(|v| v.as_str());
            if let Some(p) = path {
                let p = p.replace('\\', "/");
                let base = p.rsplit('/').next().unwrap_or(&p).to_string();
                map.insert(base, p);
            }
        }
    }
    map
}

/// OBB 在设备上的目标路径（优先级与原版一致）。
pub fn obb_remote_path(
    entry: &str,
    package: Option<&str>,
    expansions: &BTreeMap<String, String>,
) -> Option<String> {
    let normalized = entry.replace('\\', "/");
    let base = normalized.rsplit('/').next().unwrap_or(&normalized);
    if let Some(p) = expansions.get(base) {
        let p = p.trim_start_matches('/');
        return Some(if p.starts_with("sdcard/") || p.starts_with("storage/") {
            format!("/{}", p)
        } else {
            format!("/sdcard/{}", p)
        });
    }
    if let Some(idx) = normalized.find("Android/obb/") {
        return Some(format!("/sdcard/{}", &normalized[idx..]));
    }
    package.map(|pkg| format!("/sdcard/Android/obb/{}/{}", pkg, base))
}

fn install_archive(
    req: &InstallRequest,
    p: &Prepared,
    device: &str,
    cancel: &Arc<AtomicBool>,
    emit: &impl Fn(InstallEvent),
) -> Result<(), Stop> {
    let args = adb::install_multiple_args(device, &req.options, &p.apks);
    emit(InstallEvent::Log(format!("adb {}", args.join(" "))));
    let out = adb::run(&req.adb, &args, cancel).map_err(Stop::Failed)?;
    log_output(&out, emit);
    check(out)?;

    for (local, remote) in &p.obbs {
        if cancel.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        emit(InstallEvent::Device {
            device: device.to_string(),
            phase: InstallPhase::PushingObb,
            message: String::new(),
        });
        let dir = match remote.rfind('/') {
            Some(i) if i > 0 => &remote[..i],
            _ => "/sdcard",
        };
        let mk = adb::shell_mkdir_args(device, dir);
        emit(InstallEvent::Log(format!("adb {}", mk.join(" "))));
        let out = adb::run(&req.adb, &mk, cancel).map_err(Stop::Failed)?;
        log_output(&out, emit);
        if out.cancelled {
            return Err(Stop::Cancelled);
        }
        if !out.success {
            return Err(Stop::Failed(
                out.failure_reason()
                    .unwrap_or_else(|| "mkdir failed".into()),
            ));
        }
        let push = adb::push_args(device, local, remote);
        emit(InstallEvent::Log(format!("adb {}", push.join(" "))));
        let out = adb::run(&req.adb, &push, cancel).map_err(Stop::Failed)?;
        log_output(&out, emit);
        if out.cancelled {
            return Err(Stop::Cancelled);
        }
        if !out.success {
            return Err(Stop::Failed(
                out.failure_reason().unwrap_or_else(|| "push failed".into()),
            ));
        }
    }
    Ok(())
}

fn log_output(out: &adb::Output, emit: &impl Fn(InstallEvent)) {
    for line in out.stdout.lines().chain(out.stderr.lines()) {
        let l = line.trim();
        if !l.is_empty() {
            emit(InstallEvent::Log(l.to_string()));
        }
    }
}

/// 拒绝 `..` 与绝对路径（zip-slip）。
pub fn safe_join(root: &Path, entry: &str) -> Option<PathBuf> {
    let normalized = entry.replace('\\', "/");
    let mut out = root.to_path_buf();
    for part in normalized.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    if out == root {
        return None;
    }
    Some(out)
}

fn extract_entry<R: io::Read + io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    dest: &Path,
) -> Result<(), String> {
    let mut entry = zip.by_name(name).map_err(|e| format!("{}: {}", name, e))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut f = File::create(dest).map_err(|e| e.to_string())?;
    io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obb_paths() {
        let mut exp = BTreeMap::new();
        exp.insert(
            "main.1.com.x.obb".to_string(),
            "Android/obb/com.x/main.1.com.x.obb".to_string(),
        );
        assert_eq!(
            obb_remote_path("Android/obb/com.x/main.1.com.x.obb", None, &BTreeMap::new()).unwrap(),
            "/sdcard/Android/obb/com.x/main.1.com.x.obb"
        );
        assert_eq!(
            obb_remote_path("main.1.com.x.obb", None, &exp).unwrap(),
            "/sdcard/Android/obb/com.x/main.1.com.x.obb"
        );
        assert_eq!(
            obb_remote_path("data/main.obb", Some("com.y"), &BTreeMap::new()).unwrap(),
            "/sdcard/Android/obb/com.y/main.obb"
        );
        assert!(obb_remote_path("x.obb", None, &BTreeMap::new()).is_none());
    }

    #[test]
    fn zip_slip_guard() {
        let root = Path::new("/tmp/r");
        assert!(safe_join(root, "../evil.apk").is_none());
        assert!(safe_join(root, "/abs.apk").is_some());
        assert_eq!(
            safe_join(root, "a/b.apk").unwrap(),
            root.join("a").join("b.apk")
        );
        assert!(safe_join(root, "").is_none());
    }
}

#[cfg(test)]
mod manual {
    //! 真机对照：`APKINFO_INSTALL_APK=/x.apk APKINFO_INSTALL_DEV=emulator-5554 cargo test -p apkinfo-core install_dump -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn install_dump() {
        let (Ok(apk), Ok(dev)) = (
            std::env::var("APKINFO_INSTALL_APK"),
            std::env::var("APKINFO_INSTALL_DEV"),
        ) else {
            return;
        };
        let adb = crate::tools::find_in_path("adb").expect("adb in PATH");
        let path = std::path::PathBuf::from(&apk);
        let req = super::InstallRequest {
            adb,
            kind: crate::info::ArchiveKind::from_path(&path),
            file: path,
            devices: vec![dev],
            options: crate::adb::InstallOptions {
                replace: true,
                ..Default::default()
            },
            selected_splits: None,
            package_name: Some("com.kidstime.manager".into()),
        };
        super::run(
            req,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            |ev| println!("{:?}", ev),
        );
    }
}
