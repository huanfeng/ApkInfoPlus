//! 用户动作与后台消息处理。所有「做事」的逻辑集中在这里，界面模块只负责接线。

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use apkinfo_core::icon::{encode_png, IconCandidate, IconKind, RgbaImage};
use apkinfo_core::info::ArchiveKind;
use windui::core::EventCtx;
use windui::event::MenuItem;
use windui::prelude::*;

use crate::state::{AppState, HashState, Msg, SigState};
use crate::ui::shorten;

/// 导出时渲染的尺寸。
const EXPORT_ICON_SIZE: u32 = 1024;
/// 预览渲染尺寸（高 DPI 下 96 逻辑像素最多需要 ~200 物理像素，取 256 留余量）。
const PREVIEW_ICON_SIZE: u32 = 256;

// ---------------------------------------------------------------------------
// 文件支持判定
// ---------------------------------------------------------------------------

/// 拖放 / 命令行传入的文件是否受支持（与原版规则一致，另接受 `.apk.1` 这类带数字后缀的）。
pub fn is_supported_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if [".apk", ".xapk", ".apkm", ".apks", ".zip"]
        .iter()
        .any(|e| name.ends_with(e))
    {
        return true;
    }
    // `.apk.1`、`.xapk.2.3`：浏览器重复下载产生的编号后缀
    let mut rest = name.as_str();
    while let Some((head, tail)) = rest.rsplit_once('.') {
        if tail.chars().all(|c| c.is_ascii_digit()) && !tail.is_empty() {
            rest = head;
        } else {
            break;
        }
    }
    [".apk", ".xapk", ".apkm", ".apks"]
        .iter()
        .any(|e| rest.ends_with(e))
}

// ---------------------------------------------------------------------------
// 打开 / 关闭
// ---------------------------------------------------------------------------

pub fn pick_file(state: &AppState, ctx: &mut EventCtx) {
    let st = state.clone();
    let mut dlg = windui::prelude::PickDialog::new()
        .title(tr!("open.select_apk_file"))
        .filter(
            tr!("open.filter_name"),
            &["apk", "xapk", "apkm", "apks", "zip"],
        )
        .filter(tr!("open.all_files"), &["*"]);
    let last = state.cfg(|c| c.last_dir.clone());
    if !last.is_empty() && Path::new(&last).is_dir() {
        dlg = dlg.directory(&last);
    }
    ctx.request_pick_file(dlg, move |picked| {
        if let Some(p) = picked {
            // 回调没有 ctx：经通道回到 UI 线程再开文件。
            let _ = st.tx.send(Msg::OpenPath(p));
        }
    });
}

pub fn open_dropped(state: &AppState, ctx: &mut EventCtx, path: PathBuf) {
    if !is_supported_file(&path) {
        ctx.toast_err(tr!("drop.invalid_file_type"));
        return;
    }
    open_file(state, ctx, path);
}

/// 打开文件：重置状态，启动解析 / 哈希 / 签名三条后台线程。
pub fn open_file(state: &AppState, ctx: &mut EventCtx, path: PathBuf) {
    if !path.is_file() {
        ctx.toast_err(tr!(
            "open.file_not_found",
            path = path.display().to_string()
        ));
        return;
    }
    let gen = state.next_generation();
    state.clear_file_state();
    state.file_path.set(Some(path.clone()));
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    state.file_size.set(size);
    state.parsing.set(true);
    state.window_title.set(format!(
        "{} - APK Info Plus",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    if let Some(dir) = path.parent() {
        let dir = dir.display().to_string();
        state.update_config(|c| c.last_dir = dir);
    }
    log::info!("打开文件: {}", path.display());

    // 解析线程
    {
        let tx = state.tx.clone();
        let p = path.clone();
        thread::spawn(move || {
            let result = apkinfo_core::load::load(&p)
                .map(Box::new)
                .map_err(|e| e.to_string());
            let _ = tx.send(Msg::Parsed { gen, result });
        });
    }

    // 哈希线程
    if state.cfg(|c| c.enable_hash) {
        state.hash_state.set(HashState::Computing);
        let tx = state.tx.clone();
        let p = path.clone();
        thread::spawn(move || {
            let result = apkinfo_core::hash::file_hashes(&p).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Hashes { gen, result });
        });
    } else {
        state.hash_state.set(HashState::Disabled);
    }

    // 签名线程：归档包没有整体签名，直接标记不适用。
    if !state.cfg(|c| c.enable_signature) {
        state.sig_state.set(SigState::Disabled);
    } else if ArchiveKind::from_path(&path).is_archive() {
        state.sig_state.set(SigState::NotApplicable);
    } else {
        state.sig_state.set(SigState::Computing);
        let tx = state.tx.clone();
        let p = path.clone();
        thread::spawn(move || {
            let result = apkinfo_core::signature::parse_file(&p).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Signature { gen, result });
        });
    }
}

pub fn reparse(state: &AppState, ctx: &mut EventCtx) {
    if let Some(p) = state.file_path.get() {
        open_file(state, ctx, p);
    }
}

pub fn close_file(state: &AppState, _ctx: &mut EventCtx) {
    state.next_generation();
    state.clear_file_state();
    state.file_path.set(None);
    state.file_size.set(0);
    state.parsing.set(false);
    state.window_title.set("APK Info Plus".into());
}

pub fn copy_path(state: &AppState, ctx: &mut EventCtx) {
    if let Some(p) = state.file_path.get() {
        let s = p.display().to_string();
        ctx.clipboard_set(&s);
        ctx.toast_ok(tr!("action.copied", content = shorten(&s, 60)));
    }
}

pub fn open_in_file_manager(state: &AppState, ctx: &mut EventCtx) {
    if let Some(p) = state.file_path.get() {
        if let Err(e) = reveal_in_file_manager(&p) {
            ctx.toast_err(e);
        }
    }
}

/// 在系统文件管理器里显示并选中文件。
pub fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    use std::process::Command;
    let r = if cfg!(target_os = "windows") {
        Command::new("explorer.exe")
            .arg("/select,")
            .arg(path)
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else {
        // Linux 没有通用的「选中文件」协议，退而打开所在目录。
        let dir = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(dir).spawn()
    };
    r.map(|_| ()).map_err(|e| e.to_string())
}

/// 打开目录。
pub fn open_directory(path: &Path) -> Result<(), String> {
    use std::process::Command;
    let r = if cfg!(target_os = "windows") {
        Command::new("explorer.exe").arg(path).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()
    } else {
        Command::new("xdg-open").arg(path).spawn()
    };
    r.map(|_| ()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// 图标
// ---------------------------------------------------------------------------

/// 切换候选图标（`delta` 为 ±1，循环）。
pub fn switch_icon(state: &AppState, _ctx: &mut EventCtx, delta: i32) {
    let count = state.icon_candidates.with(|c| c.len());
    if count <= 1 {
        return;
    }
    let cur = state.icon_index.get() as i32;
    let next = ((cur + delta).rem_euclid(count as i32)) as usize;
    select_icon(state, next);
}

pub fn select_icon(state: &AppState, index: usize) {
    let Some(candidate) = state.icon_candidates.with(|c| c.get(index).cloned()) else {
        return;
    };
    let Some(session) = state.session.borrow().clone() else {
        return;
    };
    state.icon_index.set(index);
    state.icon_loading.set(true);
    let gen = state.generation.get();
    let tx = state.tx.clone();
    thread::spawn(move || {
        let result = match session.lock() {
            Ok(mut s) => s.render(&candidate, PREVIEW_ICON_SIZE, false),
            Err(_) => Err("icon session poisoned".to_string()),
        };
        let _ = tx.send(Msg::Icon { gen, index, result });
    });
}

/// 图标右键 / 导出菜单。
pub fn icon_menu_items(state: &AppState) -> Vec<MenuItem> {
    let mut items = Vec::new();
    let candidates = state.icon_candidates.get();
    let current = state.icon_index.get();
    if candidates.len() > 1 {
        for (i, c) in candidates.iter().enumerate() {
            let st = state.clone();
            items.push(MenuItem::run(
                c.display_label.clone(),
                move |_ctx| select_icon(&st, i),
                i == current,
            ));
        }
        items.push(MenuItem::separator());
    }
    let has_icon = state.icon.with(|v| !v.is_empty());
    let is_vector = candidates
        .get(current)
        .map(|c| c.kind == IconKind::XmlVector)
        .unwrap_or(false);
    {
        let st = state.clone();
        items.push(
            MenuItem::run(
                tr!("action.export_png"),
                move |ctx| export_png(&st, ctx),
                false,
            )
            .enabled(has_icon),
        );
    }
    {
        let st = state.clone();
        items.push(
            MenuItem::run(
                tr!("action.export_svg"),
                move |ctx| export_svg(&st, ctx),
                false,
            )
            .enabled(has_icon && is_vector),
        );
    }
    {
        let st = state.clone();
        items.push(
            MenuItem::run(
                tr!("action.export_original"),
                move |ctx| export_original(&st, ctx),
                false,
            )
            .enabled(has_icon),
        );
    }
    items
}

fn export_base_name(state: &AppState) -> String {
    let label = state.display_label();
    let label = if label.trim().is_empty() {
        "icon".to_string()
    } else {
        label
    };
    sanitize_file_name(&label)
}

/// 去掉文件名里的非法字符。
pub fn sanitize_file_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn current_candidate(state: &AppState) -> Option<IconCandidate> {
    let idx = state.icon_index.get();
    state.icon_candidates.with(|c| c.get(idx).cloned())
}

fn session_of(state: &AppState) -> Option<Arc<Mutex<apkinfo_core::load::IconSession>>> {
    state.session.borrow().clone()
}

pub fn export_png(state: &AppState, ctx: &mut EventCtx) {
    let (Some(candidate), Some(session)) = (current_candidate(state), session_of(state)) else {
        return;
    };
    let fallback: Option<Rc<RgbaImage>> = state.icon.with(|v| v.first().cloned());
    let tx = state.tx.clone();
    let dlg = PickDialog::new()
        .title(tr!("action.export_icon"))
        .file_name(format!("{}_icon.png", export_base_name(state)))
        .filter("PNG", &["png"]);
    ctx.request_save_file(dlg, move |dest| {
        let Some(dest) = dest else { return };
        let fallback = fallback.map(|f| (*f).clone());
        thread::spawn(move || {
            let image = match session.lock() {
                Ok(mut s) => s.render(&candidate, EXPORT_ICON_SIZE, true).ok(),
                Err(_) => None,
            }
            .or(fallback);
            let result = match image {
                Some(img) => std::fs::write(&dest, encode_png(&img))
                    .map(|_| dest)
                    .map_err(|e| e.to_string()),
                None => Err(tr!("icon.render_failed")),
            };
            let _ = tx.send(Msg::Exported(result));
        });
    });
}

pub fn export_svg(state: &AppState, ctx: &mut EventCtx) {
    let (Some(candidate), Some(session)) = (current_candidate(state), session_of(state)) else {
        return;
    };
    if candidate.kind != IconKind::XmlVector {
        ctx.toast_err(tr!("icon.svg_only_vector"));
        return;
    }
    let tx = state.tx.clone();
    let dlg = PickDialog::new()
        .title(tr!("action.export_icon"))
        .file_name(format!("{}_icon.svg", export_base_name(state)))
        .filter("SVG", &["svg"]);
    ctx.request_save_file(dlg, move |dest| {
        let Some(dest) = dest else { return };
        thread::spawn(move || {
            let svg = match session.lock() {
                Ok(mut s) => s.export_svg(&candidate),
                Err(_) => Err("icon session poisoned".to_string()),
            };
            let result = svg.and_then(|text| {
                std::fs::write(&dest, text)
                    .map(|_| dest)
                    .map_err(|e| e.to_string())
            });
            let _ = tx.send(Msg::Exported(result));
        });
    });
}

pub fn export_original(state: &AppState, ctx: &mut EventCtx) {
    let (Some(candidate), Some(session)) = (current_candidate(state), session_of(state)) else {
        return;
    };
    let fallback: Option<Rc<RgbaImage>> = state.icon.with(|v| v.first().cloned());
    let ext = Path::new(&candidate.path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "png".into());
    let tx = state.tx.clone();
    let dlg = PickDialog::new()
        .title(tr!("action.export_icon"))
        .file_name(format!("{}_icon.{}", export_base_name(state), ext));
    ctx.request_save_file(dlg, move |dest| {
        let Some(dest) = dest else { return };
        let fallback = fallback.map(|f| (*f).clone());
        thread::spawn(move || {
            let bytes = match session.lock() {
                Ok(mut s) => s.raw_bytes(&candidate),
                Err(_) => None,
            }
            .or_else(|| fallback.map(|f| encode_png(&f)));
            let result = match bytes {
                Some(b) => std::fs::write(&dest, b)
                    .map(|_| dest)
                    .map_err(|e| e.to_string()),
                None => Err(tr!("icon.render_failed")),
            };
            let _ = tx.send(Msg::Exported(result));
        });
    });
}

// ---------------------------------------------------------------------------
// 应用名语言菜单
// ---------------------------------------------------------------------------

pub fn show_label_menu(state: &AppState, ctx: &mut EventCtx) {
    let Some(info) = state.current_info() else {
        return;
    };
    let current = state.label_locale.get();
    let mut items = Vec::new();
    {
        let st = state.clone();
        let default_label = info.meta.label.clone().unwrap_or_default();
        items.push(MenuItem::run(
            format!("{}  {}", tr!("action.default_label"), default_label),
            move |_ctx| st.label_locale.set(None),
            current.is_none(),
        ));
    }
    items.push(MenuItem::separator());
    for (loc, label) in info.alternative_labels() {
        let st = state.clone();
        let loc_s = loc.to_string();
        let checked = current.as_deref() == Some(loc);
        items.push(MenuItem::run(
            format!("{}  {}", loc, label),
            move |_ctx| st.label_locale.set(Some(loc_s.clone())),
            checked,
        ));
    }
    let mut req = windui::event::MenuRequest::below(ctx.bounds(), items);
    req.click_through = false;
    ctx.show_menu_request(req);
}

// ---------------------------------------------------------------------------
// 对话框 / 子窗口
// ---------------------------------------------------------------------------

pub fn open_install(state: &AppState, ctx: &mut EventCtx) {
    if state.current_info().is_none() {
        return;
    }
    crate::ui::install::open(state, ctx);
}

pub fn open_rename(state: &AppState, _ctx: &mut EventCtx) {
    if state.current_info().is_none() {
        return;
    }
    crate::ui::rename::open(state);
}

pub fn open_settings(state: &AppState, ctx: &mut EventCtx) {
    crate::ui::settings::open(state, ctx);
}

/// 把解析结果写进状态（不含图标渲染）。
pub fn apply_parsed(state: &AppState, loaded: apkinfo_core::load::Loaded) {
    log::info!(
        "解析完成: {} ({} 个图标候选)",
        loaded.info.meta.package_name.as_deref().unwrap_or("?"),
        loaded.candidates.len()
    );
    state.parse_error.set(String::new());
    state.icon_candidates.set(loaded.candidates);
    state.icon_index.set(0);
    *state.session.borrow_mut() = Some(Arc::new(Mutex::new(loaded.session)));
    state.info.set(vec![Rc::new(loaded.info)]);
}

/// 同步加载（离屏截图模式用）：解析、首个图标、哈希、签名全部在当前线程完成，
/// 这样 `--screenshot` 出的那一帧就是加载完成后的界面。
pub fn load_sync(state: &AppState, path: PathBuf) {
    state.next_generation();
    state.clear_file_state();
    state.file_path.set(Some(path.clone()));
    state
        .file_size
        .set(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
    match apkinfo_core::load::load(&path) {
        Ok(loaded) => {
            apply_parsed(state, loaded);
            let candidates = state.icon_candidates.get();
            if let Some(session) = state.session.borrow().clone() {
                if let Ok(mut s) = session.lock() {
                    for (i, c) in candidates.iter().enumerate() {
                        if let Ok(img) = s.render(c, PREVIEW_ICON_SIZE, false) {
                            state.icon_index.set(i);
                            state.icon.set(vec![Rc::new(img)]);
                            break;
                        }
                    }
                }
            }
        }
        Err(e) => state
            .parse_error
            .set(tr!("parse.failed_detail", error = e.to_string())),
    }
    if state.cfg(|c| c.enable_hash) {
        match apkinfo_core::hash::file_hashes(&path) {
            Ok((m, s)) => {
                state.md5.set(m);
                state.sha1.set(s);
                state.hash_state.set(HashState::Done);
            }
            Err(e) => {
                state.hash_error.set(e.to_string());
                state.hash_state.set(HashState::Failed);
            }
        }
    } else {
        state.hash_state.set(HashState::Disabled);
    }
    if !state.cfg(|c| c.enable_signature) {
        state.sig_state.set(SigState::Disabled);
    } else if ArchiveKind::from_path(&path).is_archive() {
        state.sig_state.set(SigState::NotApplicable);
    } else {
        match apkinfo_core::signature::parse_file(&path) {
            Ok(sig) => {
                state.signature.set(vec![Rc::new(sig)]);
                state.sig_state.set(SigState::Done);
            }
            Err(e) => {
                state.sig_error.set(e);
                state.sig_state.set(SigState::Failed);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 后台消息
// ---------------------------------------------------------------------------

pub fn handle_msg(state: &AppState, ctx: &mut EventCtx, msg: Msg) {
    match msg {
        Msg::OpenPath(p) => open_dropped(state, ctx, p),
        Msg::Parsed { gen, result } => {
            if !state.is_current(gen) {
                return;
            }
            state.parsing.set(false);
            match result {
                Ok(loaded) => {
                    apply_parsed(state, *loaded);
                    // 先渲染第一个候选；失败则依次尝试后面的（在 Msg::Icon 里接力）。
                    if state.icon_candidates.with(|c| !c.is_empty()) {
                        select_icon(state, 0);
                    }
                }
                Err(e) => {
                    log::warn!("解析失败: {}", e);
                    state
                        .parse_error
                        .set(tr!("parse.failed_detail", error = e.clone()));
                    ctx.toast_err(tr!("parse.failed"));
                }
            }
        }
        Msg::Icon { gen, index, result } => {
            if !state.is_current(gen) {
                return;
            }
            match result {
                Ok(img) => {
                    state.icon_loading.set(false);
                    state.icon_index.set(index);
                    state.icon.set(vec![Rc::new(img)]);
                }
                Err(e) => {
                    log::warn!("图标渲染失败 #{}: {}", index, e);
                    // 首次加载链：当前还没有任何图标时自动试下一个候选。
                    let count = state.icon_candidates.with(|c| c.len());
                    let none_yet = state.icon.with(|v| v.is_empty());
                    if none_yet && index + 1 < count {
                        select_icon(state, index + 1);
                    } else {
                        state.icon_loading.set(false);
                        if !none_yet {
                            ctx.toast_err(tr!("icon.render_failed"));
                        }
                    }
                }
            }
        }
        Msg::Hashes { gen, result } => {
            if !state.is_current(gen) {
                return;
            }
            match result {
                Ok((md5, sha1)) => {
                    state.md5.set(md5);
                    state.sha1.set(sha1);
                    state.hash_state.set(HashState::Done);
                }
                Err(e) => {
                    state.hash_error.set(e);
                    state.hash_state.set(HashState::Failed);
                }
            }
        }
        Msg::Signature { gen, result } => {
            if !state.is_current(gen) {
                return;
            }
            match result {
                Ok(sig) => {
                    state.signature.set(vec![Rc::new(sig)]);
                    state.sig_state.set(SigState::Done);
                }
                Err(e) => {
                    state.sig_error.set(e);
                    state.sig_state.set(SigState::Failed);
                }
            }
        }
        Msg::Exported(result) => match result {
            Ok(p) => ctx.toast_ok(tr!(
                "icon.export_success",
                path = shorten(&p.display().to_string(), 60)
            )),
            Err(e) => ctx.toast_err(tr!("icon.export_failed", error = e)),
        },
        Msg::Devices(result) => crate::ui::install::on_devices(state, ctx, result),
        Msg::Install(ev) => crate::ui::install::on_event(state, ctx, ev),
        Msg::Download(ev) => crate::ui::settings::on_download_event(state, ctx, ev),
        Msg::Shortcut(s) => match s {
            crate::Shortcut::Open => pick_file(state, ctx),
            crate::Shortcut::CloseFile => close_file(state, ctx),
            crate::Shortcut::Install => open_install(state, ctx),
            crate::Shortcut::Rename => open_rename(state, ctx),
            crate::Shortcut::Reparse => reparse(state, ctx),
            crate::Shortcut::Settings => open_settings(state, ctx),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions() {
        assert!(is_supported_file(Path::new("a.apk")));
        assert!(is_supported_file(Path::new("A.XAPK")));
        assert!(is_supported_file(Path::new("a.apk.1")));
        assert!(is_supported_file(Path::new("a.apkm.2.3")));
        assert!(is_supported_file(Path::new("a.zip")));
        assert!(!is_supported_file(Path::new("a.txt")));
        assert!(!is_supported_file(Path::new("apk")));
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_file_name("a/b:c*d"), "a_b_c_d");
        assert_eq!(sanitize_file_name("  ok name "), "ok name");
    }
}
