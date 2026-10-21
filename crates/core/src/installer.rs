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

/// 临时目录名前缀；`cleanup_stale_temp_dirs` 按它识别上次崩溃 / 强退留下的残骸。
const TEMP_PREFIX: &str = "apkinfoplus-install";

impl TempDir {
    fn new() -> io::Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p =
            std::env::temp_dir().join(format!("{}-{}-{}", TEMP_PREFIX, std::process::id(), nanos));
        std::fs::create_dir_all(&p)?;
        Ok(Self(p))
    }
}

/// 删除其它进程（通常是上次没正常退出的本程序）留下的安装临时目录。
/// 解压出来的 OBB 可能有几 GB，不清会一直占着磁盘。
pub fn cleanup_stale_temp_dirs() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let me = format!("{}-{}-", TEMP_PREFIX, std::process::id());
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(TEMP_PREFIX) && !name.starts_with(&me) && e.path().is_dir() {
            if let Err(err) = std::fs::remove_dir_all(e.path()) {
                log::debug!("清理残留临时目录失败 {}: {}", name, err);
            }
        }
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
        match prepare_archive(&req, &cancel, &emit) {
            Ok(p) => Some(p),
            Err(Stop::Cancelled) => {
                for d in &req.devices {
                    emit(InstallEvent::Device {
                        device: d.clone(),
                        phase: InstallPhase::Stopped,
                        message: String::new(),
                    });
                }
                emit(InstallEvent::Finished {
                    success: 0,
                    failed: 0,
                });
                return;
            }
            Err(Stop::Failed(e)) => {
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
    // adb install 只认 .apk 后缀：`.zip`、`.apk.1` 这类改了名的包先复制到临时目录改回去。
    let needs_rename = !req
        .file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("apk"));
    let _tmp;
    let apk_path = if needs_rename {
        let dir = TempDir::new().map_err(|e| Stop::Failed(e.to_string()))?;
        let stem = req
            .file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app".into());
        let dest = dir.0.join(format!("{}.apk", stem));
        if std::fs::hard_link(&req.file, &dest).is_err() {
            std::fs::copy(&req.file, &dest).map_err(|e| Stop::Failed(e.to_string()))?;
        }
        _tmp = Some(dir);
        dest
    } else {
        _tmp = None;
        req.file.clone()
    };
    let args = adb::install_args(device, &req.options, &apk_path);
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

fn prepare_archive(
    req: &InstallRequest,
    cancel: &Arc<AtomicBool>,
    emit: &impl Fn(InstallEvent),
) -> Result<Prepared, Stop> {
    let fail = |e: String| Stop::Failed(e);
    let file = File::open(&req.file).map_err(|e| fail(e.to_string()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| fail(e.to_string()))?;
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
        // 明确传了空选择就是「一个都不装」，不能悄悄退化成装全部
        Some(sel) if sel.is_empty() => return Err(fail("no apk entries selected".into())),
        Some(sel) => sel.clone(),
        None => names
            .iter()
            .filter(|n| n.to_lowercase().ends_with(".apk"))
            .cloned()
            .collect(),
    };
    if apks.is_empty() {
        return Err(fail("no apk entries selected".into()));
    }
    // base 排最前，其余按路径排序：install-multiple 对顺序不敏感，但日志更好读。
    apks.sort_by(|a, b| {
        let ab = is_base_split(&split_stem(a).to_lowercase());
        let bb = is_base_split(&split_stem(b).to_lowercase());
        bb.cmp(&ab).then_with(|| a.cmp(b))
    });

    let dir = TempDir::new().map_err(|e| fail(e.to_string()))?;
    let mut local_apks = Vec::new();
    for name in &apks {
        if cancel.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        let dest = safe_join(&dir.0, name)
            .ok_or_else(|| fail(format!("unsafe path in archive: {}", name)))?;
        extract_entry(&mut zip, name, &dest).map_err(fail)?;
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
        if cancel.load(Ordering::SeqCst) {
            return Err(Stop::Cancelled);
        }
        let Some(remote) = obb_remote_path(name, package.as_deref(), &expansion_paths) else {
            emit(InstallEvent::Log(format!(
                "skip obb (no safe target path): {}",
                name
            )));
            continue;
        };
        let dest = safe_join(&dir.0, name)
            .ok_or_else(|| fail(format!("unsafe path in archive: {}", name)))?;
        extract_entry(&mut zip, name, &dest).map_err(fail)?;
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
///
/// 路径来源（manifest.json 的 install_path、zip 条目名、包名）都是归档包作者给的，
/// 而它最终会出现在 adb 命令行里，所以只接受 `/sdcard/Android/obb/<包名>/<文件>` 这一种形状，
/// 其它一律返回 `None` 跳过——宁可少推一个 OBB，也不把任意字符串送上设备 shell。
pub fn obb_remote_path(
    entry: &str,
    package: Option<&str>,
    expansions: &BTreeMap<String, String>,
) -> Option<String> {
    let normalized = entry.replace('\\', "/");
    let base = normalized.rsplit('/').next().unwrap_or(&normalized);
    let candidate = if let Some(p) = expansions.get(base) {
        let p = p.trim_start_matches('/');
        let p = p
            .strip_prefix("storage/emulated/0/")
            .or_else(|| p.strip_prefix("sdcard/"))
            .unwrap_or(p);
        format!("/sdcard/{}", p)
    } else if let Some(idx) = normalized.find("Android/obb/") {
        format!("/sdcard/{}", &normalized[idx..])
    } else {
        format!("/sdcard/Android/obb/{}/{}", package?, base)
    };
    is_safe_obb_path(&candidate).then_some(candidate)
}

/// `/sdcard/Android/obb/<pkg>/<file>`：包名与文件名只允许字母数字 `._-`，不能有 `..`。
pub fn is_safe_obb_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/sdcard/Android/obb/") else {
        return false;
    };
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 2 {
        return false;
    }
    let ok = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    ok(parts[0]) && ok(parts[1]) && parts[1].to_ascii_lowercase().ends_with(".obb")
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
        // 不再单独 `adb shell mkdir -p`：adb push 自己会建父目录，
        // 也就不存在把路径拼进设备 shell 的机会。
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
        // 注入 / 越界的路径一律拒绝
        let mut evil = BTreeMap::new();
        evil.insert(
            "a.obb".to_string(),
            "Android/obb/com.x/a.obb; rm -rf /sdcard".to_string(),
        );
        assert!(obb_remote_path("a.obb", None, &evil).is_none());
        assert!(obb_remote_path("Android/obb/../../x.obb", None, &BTreeMap::new()).is_none());
        assert!(obb_remote_path("Android/obb/com.x/sub/x.obb", None, &BTreeMap::new()).is_none());
        assert!(obb_remote_path("b.obb", Some("com.x$(id)"), &BTreeMap::new()).is_none());
        assert!(is_safe_obb_path(
            "/sdcard/Android/obb/com.a.b/main.1.com.a.b.obb"
        ));
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
