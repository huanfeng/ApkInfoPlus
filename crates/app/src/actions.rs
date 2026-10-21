//! 用户动作与后台消息处理。所有「做事」的逻辑集中在这里，界面模块只负责接线。
//!
//! 作用在文件上的动作都显式收一个 `Rc<Doc>`；工具栏那种「对当前文档」的入口
//! 先取 `state.active_doc()` 再调过来。

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use apkinfo_core::icon::{encode_png, IconCandidate, IconKind, RgbaImage};
use apkinfo_core::info::ArchiveKind;
use windui::core::EventCtx;
use windui::event::MenuItem;
use windui::prelude::*;

use crate::state::{AppState, Doc, HashState, IconThumb, Msg, SigState};
use crate::ui::shorten;

/// 导出时渲染的尺寸。
const EXPORT_ICON_SIZE: u32 = 1024;
/// 预览渲染尺寸（高 DPI 下 96 逻辑像素最多需要 ~200 物理像素，取 256 留余量）。
const PREVIEW_ICON_SIZE: u32 = 256;
/// 概览页图标条的缩略图尺寸。
const THUMB_ICON_SIZE: u32 = 96;

// ---------------------------------------------------------------------------
// 后台线程：panic 兜底
// ---------------------------------------------------------------------------

/// 在后台线程里跑 `f`，panic 转成 `Err`。否则线程一 panic 就没有消息发回 UI，
/// 「正在解析…」会永远挂着。
fn guarded<T>(what: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".into());
            log::error!("{} 线程 panic: {}", what, msg);
            Err(format!("internal error: {}", msg))
        }
    }
}

/// 正在给这个文档安装时，阻止会动它的操作（关闭 / 重新解析 / 改名），并提示。
fn blocked_by_install(state: &AppState, ctx: &mut EventCtx, doc: &Doc) -> bool {
    if state.install.installing.get() && state.install.doc_id.get() == doc.id {
        ctx.toast(tr!("install.installing"));
        true
    } else {
        false
    }
}

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
            let _ = st.tx.send(Msg::OpenPaths(vec![p]));
        }
    });
}

/// 拖放 / 命令行 / 第二实例送来的一批路径：不支持的提示一次，其余逐个打开，
/// 批次里第一个成为当前文档。
pub fn open_paths(state: &AppState, ctx: &mut EventCtx, paths: Vec<PathBuf>) {
    let mut first: Option<u64> = None;
    let mut rejected = false;
    for p in paths {
        if !is_supported_file(&p) {
            rejected = true;
            continue;
        }
        if let Some(doc) = open_file(state, ctx, p) {
            first.get_or_insert(doc.id);
        }
    }
    if rejected {
        ctx.toast_err(tr!("drop.invalid_file_type"));
    }
    if let Some(id) = first {
        state.set_active(id);
    }
}

/// 打开文件：已经开着就切过去；否则新建文档并启动解析 / 哈希 / 签名三条后台线程。
pub fn open_file(state: &AppState, ctx: &mut EventCtx, path: PathBuf) -> Option<Rc<Doc>> {
    if !path.is_file() {
        ctx.toast_err(tr!(
            "open.file_not_found",
            path = path.display().to_string()
        ));
        return None;
    }
    if let Some(doc) = state.doc_by_path(&path) {
        state.set_active(doc.id);
        return Some(doc);
    }
    if let Some(dir) = path.parent() {
        let dir = dir.display().to_string();
        state.update_config(|c| c.last_dir = dir);
    }
    log::info!("打开文件: {}", path.display());
    let doc = Doc::new(state.next_generation(), path);
    state.add_doc(doc.clone());
    start_parse(state, &doc);
    Some(doc)
}

/// （重新）解析文档：换新代次，清旧结果，三条后台线程各自带着代次回报。
fn start_parse(state: &AppState, doc: &Rc<Doc>) {
    let gen = state.next_generation();
    doc.gen.set(gen);
    doc.reset();
    let path = doc.path.get();
    doc.file_size
        .set(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
    doc.parsing.set(true);
    doc.subtitle.set(tr!("parse.parsing"));
    state.refresh_entries();

    // 解析线程
    {
        let tx = state.tx.clone();
        let p = path.clone();
        thread::spawn(move || {
            let result = guarded("解析", || {
                apkinfo_core::load::load(&p)
                    .map(Box::new)
                    .map_err(|e| e.to_string())
            });
            let _ = tx.send(Msg::Parsed { gen, result });
        });
    }

    // 哈希线程
    if state.cfg(|c| c.enable_hash) {
        doc.hash_state.set(HashState::Computing);
        let tx = state.tx.clone();
        let p = path.clone();
        thread::spawn(move || {
            let result = guarded("哈希", || {
                apkinfo_core::hash::file_hashes(&p).map_err(|e| e.to_string())
            });
            let _ = tx.send(Msg::Hashes { gen, result });
        });
    } else {
        doc.hash_state.set(HashState::Disabled);
    }

    // 签名线程：归档包没有整体签名，直接标记不适用。
    if !state.cfg(|c| c.enable_signature) {
        doc.sig_state.set(SigState::Disabled);
    } else if ArchiveKind::from_path(&path).is_archive() {
        doc.sig_state.set(SigState::NotApplicable);
    } else {
        doc.sig_state.set(SigState::Computing);
        let tx = state.tx.clone();
        let p = path.clone();
        thread::spawn(move || {
            let result = guarded("签名", || {
                apkinfo_core::signature::parse_file(&p).map_err(|e| e.to_string())
            });
            let _ = tx.send(Msg::Signature { gen, result });
        });
    }
}

pub fn reparse_doc(state: &AppState, ctx: &mut EventCtx, doc: &Rc<Doc>) {
    if blocked_by_install(state, ctx, doc) {
        return;
    }
    start_parse(state, doc);
}

pub fn reparse(state: &AppState, ctx: &mut EventCtx) {
    if let Some(doc) = state.active_doc() {
        reparse_doc(state, ctx, &doc);
    }
}

pub fn close_doc(state: &AppState, ctx: &mut EventCtx, doc: &Rc<Doc>) {
    if blocked_by_install(state, ctx, doc) {
        return;
    }
    state.remove_doc(doc.id);
}

pub fn close_file(state: &AppState, ctx: &mut EventCtx) {
    if let Some(doc) = state.active_doc() {
        close_doc(state, ctx, &doc);
    }
}

/// 关闭全部（正在安装的那个留下）。
pub fn close_all(state: &AppState, ctx: &mut EventCtx) {
    for doc in state.docs.get() {
        close_doc(state, ctx, &doc);
    }
}

/// 切到相邻文档（Ctrl+Tab / Ctrl+Shift+Tab），循环。
pub fn activate_neighbor(state: &AppState, delta: i32) {
    let docs = state.docs.get();
    if docs.len() < 2 {
        return;
    }
    let cur = docs
        .iter()
        .position(|d| d.id == state.active_id.get())
        .unwrap_or(0) as i32;
    let next = (cur + delta).rem_euclid(docs.len() as i32) as usize;
    state.set_active(docs[next].id);
}

pub fn copy_path(state: &AppState, ctx: &mut EventCtx) {
    if let Some(doc) = state.active_doc() {
        let s = doc.path.with(|p| p.display().to_string());
        ctx.clipboard_set(&s);
        ctx.toast_ok(tr!("action.copied", content = shorten(&s, 60)));
    }
}

pub fn open_in_file_manager(state: &AppState, ctx: &mut EventCtx) {
    if let Some(doc) = state.active_doc() {
        if let Err(e) = reveal_in_file_manager(&doc.path.get()) {
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
pub fn switch_icon(state: &AppState, doc: &Rc<Doc>, delta: i32) {
    let count = doc.icon_candidates.with(|c| c.len());
    if count <= 1 {
        return;
    }
    let cur = doc.icon_index.get() as i32;
    let next = ((cur + delta).rem_euclid(count as i32)) as usize;
    select_icon(state, doc, next);
}

pub fn select_icon(state: &AppState, doc: &Rc<Doc>, index: usize) {
    let Some(candidate) = doc.icon_candidates.with(|c| c.get(index).cloned()) else {
        return;
    };
    let Some(session) = doc.session.borrow().clone() else {
        return;
    };
    doc.icon_index.set(index);
    doc.icon_loading.set(true);
    let gen = doc.gen.get();
    let token = doc.icon_req.get() + 1;
    doc.icon_req.set(token);
    let tx = state.tx.clone();
    thread::spawn(move || {
        let result = guarded("图标渲染", || match session.lock() {
            Ok(mut s) => s.render(&candidate, PREVIEW_ICON_SIZE, false),
            // 上一次渲染 panic 过：取回锁继续用，比让所有图标永远报错好
            Err(poisoned) => poisoned
                .into_inner()
                .render(&candidate, PREVIEW_ICON_SIZE, false),
        });
        let _ = tx.send(Msg::Icon {
            gen,
            token,
            index,
            result,
        });
    });
}

/// 把全部候选渲染成缩略图（同步，在调用线程上跑）。渲染失败的候选跳过。
fn render_thumbs(
    session: &Arc<Mutex<apkinfo_core::load::IconSession>>,
    candidates: &[IconCandidate],
) -> Vec<IconThumb> {
    let mut out = Vec::new();
    let mut guard = match session.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    for (index, c) in candidates.iter().enumerate() {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            guard.render(c, THUMB_ICON_SIZE, false)
        }));
        if let Ok(Ok(img)) = r {
            out.push(IconThumb {
                index,
                label: thumb_label(c),
                image: img,
            });
        }
    }
    out
}

/// 缩略图下方的小字：密度名；没有密度的按类型给。
fn thumb_label(c: &IconCandidate) -> String {
    if let Some(d) = &c.dpi_label {
        return d.clone();
    }
    if c.path == apkinfo_core::load::ARCHIVE_ICON_PATH {
        return "png".into();
    }
    match c.kind {
        IconKind::XmlVector => "xml".into(),
        IconKind::Bitmap => "png".into(),
    }
}

/// 后台渲染缩略图条，结果经 `Msg::IconThumbs` 回来。
fn spawn_thumbs(state: &AppState, doc: &Rc<Doc>) {
    let Some(session) = doc.session.borrow().clone() else {
        return;
    };
    let candidates = doc.icon_candidates.get();
    if candidates.is_empty() {
        return;
    }
    let gen = doc.gen.get();
    let tx = state.tx.clone();
    thread::spawn(move || {
        let thumbs = render_thumbs(&session, &candidates);
        let _ = tx.send(Msg::IconThumbs { gen, thumbs });
    });
}

/// 图标右键 / 导出菜单。
pub fn icon_menu_items(state: &AppState, doc: &Rc<Doc>) -> Vec<MenuItem> {
    let mut items = Vec::new();
    let candidates = doc.icon_candidates.get();
    let current = doc.icon_index.get();
    if candidates.len() > 1 {
        for (i, c) in candidates.iter().enumerate() {
            let st = state.clone();
            let d = doc.clone();
            items.push(MenuItem::run(
                c.display_label.clone(),
                move |_ctx| select_icon(&st, &d, i),
                i == current,
            ));
        }
        items.push(MenuItem::separator());
    }
    let has_icon = doc.icon.with(|v| !v.is_empty());
    let is_vector = candidates
        .get(current)
        .map(|c| c.kind == IconKind::XmlVector)
        .unwrap_or(false);
    {
        let st = state.clone();
        let d = doc.clone();
        items.push(
            MenuItem::run(
                tr!("action.export_png"),
                move |ctx| export_png(&st, &d, ctx),
                false,
            )
            .enabled(has_icon),
        );
    }
    {
        let st = state.clone();
        let d = doc.clone();
        items.push(
            MenuItem::run(
                tr!("action.export_svg"),
                move |ctx| export_svg(&st, &d, ctx),
                false,
            )
            .enabled(has_icon && is_vector),
        );
    }
    {
        let st = state.clone();
        let d = doc.clone();
        items.push(
            MenuItem::run(
                tr!("action.export_original"),
                move |ctx| export_original(&st, &d, ctx),
                false,
            )
            .enabled(has_icon),
        );
    }
    items
}

fn export_base_name(doc: &Doc) -> String {
    let label = doc.display_label();
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

fn current_candidate(doc: &Doc) -> Option<IconCandidate> {
    let idx = doc.icon_index.get();
    doc.icon_candidates.with(|c| c.get(idx).cloned())
}

fn session_of(doc: &Doc) -> Option<Arc<Mutex<apkinfo_core::load::IconSession>>> {
    doc.session.borrow().clone()
}

pub fn export_png(state: &AppState, doc: &Rc<Doc>, ctx: &mut EventCtx) {
    let (Some(candidate), Some(session)) = (current_candidate(doc), session_of(doc)) else {
        return;
    };
    let fallback: Option<Rc<RgbaImage>> = doc.icon.with(|v| v.first().cloned());
    let tx = state.tx.clone();
    let dlg = PickDialog::new()
        .title(tr!("action.export_icon"))
        .file_name(format!("{}_icon.png", export_base_name(doc)))
        .filter("PNG", &["png"]);
    ctx.request_save_file(dlg, move |dest| {
        let Some(dest) = dest else { return };
        let fallback = fallback.map(|f| (*f).clone());
        thread::spawn(move || {
            let image = match session.lock() {
                Ok(mut s) => s.render(&candidate, EXPORT_ICON_SIZE, true).ok(),
                Err(p) => p
                    .into_inner()
                    .render(&candidate, EXPORT_ICON_SIZE, true)
                    .ok(),
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

pub fn export_svg(state: &AppState, doc: &Rc<Doc>, ctx: &mut EventCtx) {
    let (Some(candidate), Some(session)) = (current_candidate(doc), session_of(doc)) else {
        return;
    };
    if candidate.kind != IconKind::XmlVector {
        ctx.toast_err(tr!("icon.svg_only_vector"));
        return;
    }
    let tx = state.tx.clone();
    let dlg = PickDialog::new()
        .title(tr!("action.export_icon"))
        .file_name(format!("{}_icon.svg", export_base_name(doc)))
        .filter("SVG", &["svg"]);
    ctx.request_save_file(dlg, move |dest| {
        let Some(dest) = dest else { return };
        thread::spawn(move || {
            let svg = match session.lock() {
                Ok(mut s) => s.export_svg(&candidate),
                Err(p) => p.into_inner().export_svg(&candidate),
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

pub fn export_original(state: &AppState, doc: &Rc<Doc>, ctx: &mut EventCtx) {
    let (Some(candidate), Some(session)) = (current_candidate(doc), session_of(doc)) else {
        return;
    };
    let fallback: Option<Rc<RgbaImage>> = doc.icon.with(|v| v.first().cloned());
    let ext = Path::new(&candidate.path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "png".into());
    let tx = state.tx.clone();
    let dlg = PickDialog::new()
        .title(tr!("action.export_icon"))
        .file_name(format!("{}_icon.{}", export_base_name(doc), ext));
    ctx.request_save_file(dlg, move |dest| {
        let Some(dest) = dest else { return };
        let fallback = fallback.map(|f| (*f).clone());
        thread::spawn(move || {
            let bytes = match session.lock() {
                Ok(mut s) => s.raw_bytes(&candidate),
                Err(p) => p.into_inner().raw_bytes(&candidate),
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

pub fn show_label_menu(doc: &Rc<Doc>, ctx: &mut EventCtx) {
    let Some(info) = doc.current_info() else {
        return;
    };
    let current = doc.label_locale.get();
    let mut items = Vec::new();
    {
        let locale = doc.label_locale;
        let default_label = info.meta.label.clone().unwrap_or_default();
        items.push(MenuItem::run(
            format!("{}  {}", tr!("action.default_label"), default_label),
            move |_ctx| locale.set(None),
            current.is_none(),
        ));
    }
    items.push(MenuItem::separator());
    for (loc, label) in info.alternative_labels() {
        let locale = doc.label_locale;
        let loc_s = loc.to_string();
        let checked = current.as_deref() == Some(loc);
        items.push(MenuItem::run(
            format!("{}  {}", loc, label),
            move |_ctx| locale.set(Some(loc_s.clone())),
            checked,
        ));
    }
    let mut req = windui::event::MenuRequest::below(ctx.bounds(), items);
    req.click_through = false;
    ctx.show_menu_request(req);
}

// ---------------------------------------------------------------------------
// 目标设备选择（概览头）
// ---------------------------------------------------------------------------

/// 「选择设备」菜单：列出 adb 设备，勾选即设为默认目标；列表为空时先去取一次。
pub fn show_device_menu(state: &AppState, ctx: &mut EventCtx) {
    let ins = &state.install;
    let rows = ins.devices.get();
    if rows.is_empty() && !ins.loading_devices.get() {
        crate::ui::install::refresh_devices(state, ctx);
    }
    let current = state.selected_device.get().map(|(id, _)| id);
    let mut items = Vec::new();
    if ins.loading_devices.get() && rows.is_empty() {
        items.push(MenuItem::run(tr!("install.loading_devices"), |_| {}, false).enabled(false));
    } else if rows.is_empty() {
        let msg = ins.device_error.get();
        let msg = if msg.is_empty() {
            tr!("install.no_devices")
        } else {
            msg
        };
        items.push(MenuItem::run(msg, |_| {}, false).enabled(false));
    }
    for r in &rows {
        let d = &r.device;
        let usable = !d.is_offline() && !d.is_unauthorized();
        let name = d.display_name();
        let label = if usable {
            name.clone()
        } else if d.is_offline() {
            format!("{}  ({})", name, tr!("install.offline"))
        } else {
            format!("{}  ({})", name, tr!("install.unauthorized"))
        };
        let sel = state.selected_device;
        let id = d.id.clone();
        let checked = current.as_deref() == Some(&d.id);
        items.push(
            MenuItem::run(
                label,
                move |_| sel.set(Some((id.clone(), name.clone()))),
                checked,
            )
            .enabled(usable),
        );
    }
    items.push(MenuItem::separator());
    {
        let sel = state.selected_device;
        items.push(MenuItem::run(
            tr!("install.any_device"),
            move |_| sel.set(None),
            current.is_none(),
        ));
    }
    {
        let st = state.clone();
        items.push(MenuItem::run(
            tr!("install.refresh_devices"),
            move |ctx| crate::ui::install::refresh_devices(&st, ctx),
            false,
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

pub fn open_rename(state: &AppState, ctx: &mut EventCtx) {
    let Some(doc) = state.active_doc() else {
        return;
    };
    if doc.current_info().is_none() || blocked_by_install(state, ctx, &doc) {
        return;
    }
    crate::ui::rename::open(state, &doc);
}

pub fn open_settings(state: &AppState, ctx: &mut EventCtx) {
    crate::ui::settings::open(state, ctx);
}

/// 把解析结果写进文档（不含图标渲染）。
pub fn apply_parsed(state: &AppState, doc: &Rc<Doc>, loaded: apkinfo_core::load::Loaded) {
    log::info!(
        "解析完成: {} ({} 个图标候选)",
        loaded.info.meta.package_name.as_deref().unwrap_or("?"),
        loaded.candidates.len()
    );
    doc.parse_error.set(String::new());
    doc.icon_candidates.set(loaded.candidates);
    doc.icon_index.set(0);
    *doc.session.borrow_mut() = Some(Arc::new(Mutex::new(loaded.session)));
    doc.subtitle.set(
        loaded
            .info
            .meta
            .package_name
            .clone()
            .unwrap_or_else(|| loaded.info.kind.display_name().to_string()),
    );
    doc.info.set(vec![Rc::new(loaded.info)]);
    state.refresh_entries();
}

fn apply_parse_error(state: &AppState, doc: &Rc<Doc>, e: String) {
    log::warn!("解析失败: {}", e);
    doc.parse_error
        .set(tr!("parse.failed_detail", error = e.clone()));
    doc.subtitle.set(tr!("parse.failed"));
    state.refresh_entries();
}

/// 同步加载（离屏截图模式用）：解析、首个图标、哈希、签名全部在当前线程完成，
/// 这样 `--screenshot` 出的那一帧就是加载完成后的界面。
pub fn load_sync(state: &AppState, path: PathBuf) -> u64 {
    let doc = Doc::new(state.next_generation(), path.clone());
    state.add_doc(doc.clone());
    match apkinfo_core::load::load(&path) {
        Ok(loaded) => {
            apply_parsed(state, &doc, loaded);
            let candidates = doc.icon_candidates.get();
            if let Some(session) = doc.session.borrow().clone() {
                if let Ok(mut s) = session.lock() {
                    for (i, c) in candidates.iter().enumerate() {
                        if let Ok(img) = s.render(c, PREVIEW_ICON_SIZE, false) {
                            doc.icon_index.set(i);
                            doc.icon.set(vec![Rc::new(img)]);
                            break;
                        }
                    }
                }
                let thumbs = render_thumbs(&session, &candidates);
                doc.icon_thumbs.set(vec![Rc::new(thumbs)]);
            }
        }
        Err(e) => apply_parse_error(state, &doc, e.to_string()),
    }
    if state.cfg(|c| c.enable_hash) {
        match apkinfo_core::hash::file_hashes(&path) {
            Ok((m, s)) => {
                doc.md5.set(m);
                doc.sha1.set(s);
                doc.hash_state.set(HashState::Done);
            }
            Err(e) => {
                doc.hash_error.set(e.to_string());
                doc.hash_state.set(HashState::Failed);
            }
        }
    } else {
        doc.hash_state.set(HashState::Disabled);
    }
    if !state.cfg(|c| c.enable_signature) {
        doc.sig_state.set(SigState::Disabled);
    } else if ArchiveKind::from_path(&path).is_archive() {
        doc.sig_state.set(SigState::NotApplicable);
    } else {
        match apkinfo_core::signature::parse_file(&path) {
            Ok(sig) => {
                doc.signature.set(vec![Rc::new(sig)]);
                doc.sig_state.set(SigState::Done);
            }
            Err(e) => {
                doc.sig_error.set(e);
                doc.sig_state.set(SigState::Failed);
            }
        }
    }
    doc.id
}

// ---------------------------------------------------------------------------
// 后台消息
// ---------------------------------------------------------------------------

pub fn handle_msg(state: &AppState, ctx: &mut EventCtx, msg: Msg) {
    match msg {
        Msg::OpenPaths(paths) => open_paths(state, ctx, paths),
        Msg::Parsed { gen, result } => {
            let Some(doc) = state.doc_by_gen(gen) else {
                return;
            };
            doc.parsing.set(false);
            match result {
                Ok(loaded) => {
                    apply_parsed(state, &doc, *loaded);
                    // 先渲染第一个候选；失败则依次尝试后面的（在 Msg::Icon 里接力）。
                    if doc.icon_candidates.with(|c| !c.is_empty()) {
                        select_icon(state, &doc, 0);
                        spawn_thumbs(state, &doc);
                    }
                }
                Err(e) => {
                    apply_parse_error(state, &doc, e);
                    ctx.toast_err(tr!("parse.failed"));
                }
            }
        }
        Msg::Icon {
            gen,
            token,
            index,
            result,
        } => {
            let Some(doc) = state.doc_by_gen(gen) else {
                return;
            };
            if token != doc.icon_req.get() {
                return;
            }
            match result {
                Ok(img) => {
                    doc.icon_loading.set(false);
                    doc.icon_index.set(index);
                    doc.icon.set(vec![Rc::new(img)]);
                }
                Err(e) => {
                    log::warn!("图标渲染失败 #{}: {}", index, e);
                    // 首次加载链：当前还没有任何图标时自动试下一个候选。
                    let count = doc.icon_candidates.with(|c| c.len());
                    let none_yet = doc.icon.with(|v| v.is_empty());
                    if none_yet && index + 1 < count {
                        select_icon(state, &doc, index + 1);
                    } else {
                        doc.icon_loading.set(false);
                        if !none_yet {
                            ctx.toast_err(tr!("icon.render_failed"));
                        }
                    }
                }
            }
        }
        Msg::IconThumbs { gen, thumbs } => {
            if let Some(doc) = state.doc_by_gen(gen) {
                doc.icon_thumbs.set(vec![Rc::new(thumbs)]);
            }
        }
        Msg::Hashes { gen, result } => {
            let Some(doc) = state.doc_by_gen(gen) else {
                return;
            };
            match result {
                Ok((md5, sha1)) => {
                    doc.md5.set(md5);
                    doc.sha1.set(sha1);
                    doc.hash_state.set(HashState::Done);
                }
                Err(e) => {
                    doc.hash_error.set(e);
                    doc.hash_state.set(HashState::Failed);
                }
            }
        }
        Msg::Signature { gen, result } => {
            let Some(doc) = state.doc_by_gen(gen) else {
                return;
            };
            match result {
                Ok(sig) => {
                    doc.signature.set(vec![Rc::new(sig)]);
                    doc.sig_state.set(SigState::Done);
                }
                Err(e) => {
                    doc.sig_error.set(e);
                    doc.sig_state.set(SigState::Failed);
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
            crate::Shortcut::NextDoc => activate_neighbor(state, 1),
            crate::Shortcut::PrevDoc => activate_neighbor(state, -1),
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
