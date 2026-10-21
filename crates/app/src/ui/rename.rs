//! 重命名对话框：预设按钮 + 自由编辑 + 校验，成功后重新打开新路径。

use std::path::{Path, PathBuf};

use windui::core::EventCtx;
use windui::prelude::*;

use crate::actions;
use crate::state::{AppState, Doc};

#[derive(Clone)]
pub struct RenameState {
    pub name: Signal<String>,
    pub error: Signal<String>,
    pub focus: Signal<u64>,
    /// 正在改名的文档（打开对话框后用户可能切到别的文档，改的仍是这一个）。
    pub doc_id: std::rc::Rc<std::cell::Cell<u64>>,
}

impl RenameState {
    pub fn new() -> Self {
        Self {
            name: signal(String::new()),
            error: signal(String::new()),
            focus: signal(0),
            doc_id: Default::default(),
        }
    }
}

fn target_doc(state: &AppState) -> Option<std::rc::Rc<Doc>> {
    state.doc_by_id(state.rename.doc_id.get())
}

/// 预设用的扩展名：按解析出的容器类型给，`.apk.1` / `.zip` 这类原名不能照搬（过不了校验）。
fn current_ext(doc: &Doc) -> String {
    match doc.current_info().map(|i| i.kind) {
        Some(apkinfo_core::info::ArchiveKind::Xapk) => ".xapk",
        Some(apkinfo_core::info::ArchiveKind::Apkm) => ".apkm",
        Some(apkinfo_core::info::ArchiveKind::Apks) => ".apks",
        _ => ".apk",
    }
    .to_string()
}

fn preset(doc: &Doc, which: Preset) -> String {
    let Some(info) = doc.current_info() else {
        return String::new();
    };
    let label = actions::sanitize_file_name(&doc.display_label());
    let label = if label.is_empty() {
        info.meta
            .package_name
            .clone()
            .unwrap_or_else(|| "app".into())
    } else {
        label
    };
    let version = actions::sanitize_file_name(info.meta.version_name.as_deref().unwrap_or(""));
    let package = actions::sanitize_file_name(info.meta.package_name.as_deref().unwrap_or(""));
    let ext = current_ext(doc);
    match which {
        Preset::Default => format!("{}-{}{}", label, version, ext),
        Preset::NameOnly => format!("{}{}", label, ext),
        Preset::NameVersion => format!("{}-v{}{}", label, version, ext),
        Preset::PackageVersion => format!("{}-{}{}", package, version, ext),
    }
}

#[derive(Clone, Copy)]
enum Preset {
    Default,
    NameOnly,
    NameVersion,
    PackageVersion,
}

pub fn open(state: &AppState, doc: &Doc) {
    let rn = &state.rename;
    rn.doc_id.set(doc.id);
    rn.name.set(preset(doc, Preset::Default));
    rn.error.set(String::new());
    state.show_rename.set(true);
    rn.focus.update(|n| *n += 1);
}

/// 校验新文件名；返回错误文案。
pub fn validate(name: &str, dir: &Path, old: &Path) -> Result<PathBuf, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(tr!("rename.name_cannot_be_empty"));
    }
    if trimmed.chars().any(|c| {
        matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
    }) {
        return Err(tr!("rename.invalid_chars"));
    }
    let lower = trimmed.to_lowercase();
    if ![".apk", ".xapk", ".apkm", ".apks"]
        .iter()
        .any(|e| lower.ends_with(e))
    {
        return Err(tr!("rename.must_end_with_apk"));
    }
    // Windows 保留设备名（CON.apk 也不行）
    let stem_upper = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem_upper.as_str()) || trimmed.ends_with('.') {
        return Err(tr!("rename.invalid_chars"));
    }
    let dest = dir.join(trimmed);
    // 只改大小写时在不区分大小写的文件系统上 `exists()` 为真，但那正是用户想要的改名
    let same_ignoring_case = dest.file_name().zip(old.file_name()).is_some_and(|(a, b)| {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    });
    if dest != old && !same_ignoring_case && dest.exists() {
        return Err(tr!("rename.exists"));
    }
    Ok(dest)
}

fn do_rename(state: &AppState, ctx: &mut EventCtx) {
    let rn = &state.rename;
    let Some(doc) = target_doc(state) else {
        state.show_rename.set(false);
        return;
    };
    let old = doc.path.get();
    let dir = old.parent().map(Path::to_path_buf).unwrap_or_default();
    match validate(&rn.name.get(), &dir, &old) {
        Err(e) => rn.error.set(e),
        Ok(dest) => {
            if dest == old {
                state.show_rename.set(false);
                return;
            }
            // 放掉 IconSession 里打开的 zip：std 打开文件时带 FILE_SHARE_DELETE，改名本身能成功，
            // 但旧会话指向的是改名前的路径，留着只会让后续图标切换读到一个「不存在」的文件。
            *doc.session.borrow_mut() = None;
            match std::fs::rename(&old, &dest) {
                Ok(()) => {
                    state.show_rename.set(false);
                    ctx.toast_ok(tr!("rename.success"));
                    doc.path.set(dest);
                    state.touch_active();
                    actions::reparse_doc(state, ctx, &doc);
                }
                Err(e) => {
                    rn.error.set(tr!("rename.failed", error = e.to_string()));
                    // 改名失败，重新解析以恢复会话、继续切换图标。
                    actions::reparse_doc(state, ctx, &doc);
                }
            }
        }
    }
}

pub fn build_dialog(state: &AppState) -> Element {
    let st = state.clone();
    let rn = state.rename.clone();
    let show = state.show_rename;

    let preset_button = |label: &'static str, which: Preset| {
        let st = st.clone();
        Element::button(t!(label))
            .small()
            .neutral()
            .outline_soft()
            .on_click(move |_| {
                if let Some(doc) = target_doc(&st) {
                    st.rename.name.set(preset(&doc, which));
                }
                st.rename.error.set(String::new());
            })
    };

    let body = Element::col()
        .width_match()
        .spacing(12)
        .child(
            Element::row()
                .width_match()
                .spacing(8)
                .child(preset_button("rename.default_name", Preset::Default))
                .child(preset_button("rename.app_name_only", Preset::NameOnly))
                .child(preset_button(
                    "rename.name_with_version",
                    Preset::NameVersion,
                ))
                .child(preset_button(
                    "rename.package_with_version",
                    Preset::PackageVersion,
                )),
        )
        .child(
            Element::text_input(rn.name, t!("rename.new_file_name"))
                .width_match()
                .focus_select_all_when(rn.focus)
                .on_submit({
                    let st = st.clone();
                    move |ctx| do_rename(&st, ctx)
                }),
        )
        .child(
            Element::label(rn.error)
                .font_size(12.5)
                .fg_role(Role::Danger)
                .width_match()
                .visible_when({
                    let err = rn.error;
                    move || err.with(|e| !e.is_empty())
                }),
        );

    let footer = Element::row()
        .width_match()
        .spacing(8)
        .child(Element::flex_spacer())
        .child(
            Element::button(t!("action.cancel"))
                .small()
                .neutral()
                .outline_soft()
                .on_click(move |_| show.set(false)),
        )
        .child(Element::button(t!("action.confirm")).small().on_click({
            let st = st.clone();
            move |ctx| do_rename(&st, ctx)
        }));

    Element::dialog_panel(
        show,
        tr!("rename.title"),
        560,
        move |_| show.set(false),
        body,
        footer,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rules() {
        let dir = std::env::temp_dir();
        let old = dir.join("old.apk");
        assert!(validate("", &dir, &old).is_err());
        assert!(validate("a/b.apk", &dir, &old).is_err());
        assert!(validate("name.txt", &dir, &old).is_err());
        assert!(validate("ok-1.0.apk", &dir, &old).is_ok());
        assert!(validate("ok.XAPK", &dir, &old).is_ok());
        assert!(validate("CON.apk", &dir, &old).is_err());
        assert!(validate("lpt1.xapk", &dir, &old).is_err());
    }
}
