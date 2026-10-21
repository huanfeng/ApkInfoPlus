//! 重命名对话框：预设按钮 + 自由编辑 + 校验，成功后重新打开新路径。

use std::path::{Path, PathBuf};

use windui::core::EventCtx;
use windui::prelude::*;

use crate::actions;
use crate::state::AppState;

#[derive(Clone)]
pub struct RenameState {
    pub name: Signal<String>,
    pub error: Signal<String>,
    pub focus: Signal<u64>,
}

impl RenameState {
    pub fn new() -> Self {
        Self {
            name: signal(String::new()),
            error: signal(String::new()),
            focus: signal(0),
        }
    }
}

/// 当前文件的扩展名（含点，小写），缺省 `.apk`。
fn current_ext(state: &AppState) -> String {
    state
        .file_path
        .with(|p| {
            p.as_ref().and_then(|p| {
                p.extension()
                    .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            })
        })
        .unwrap_or_else(|| ".apk".into())
}

fn preset(state: &AppState, which: Preset) -> String {
    let Some(info) = state.current_info() else {
        return String::new();
    };
    let label = actions::sanitize_file_name(&state.display_label());
    let label = if label.is_empty() {
        info.meta
            .package_name
            .clone()
            .unwrap_or_else(|| "app".into())
    } else {
        label
    };
    let version = info.meta.version_name.clone().unwrap_or_default();
    let package = info.meta.package_name.clone().unwrap_or_default();
    let ext = current_ext(state);
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

pub fn open(state: &AppState) {
    let rn = &state.rename;
    rn.name.set(preset(state, Preset::Default));
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
    let dest = dir.join(trimmed);
    if dest != old && dest.exists() {
        return Err(tr!("rename.exists"));
    }
    Ok(dest)
}

fn do_rename(state: &AppState, ctx: &mut EventCtx) {
    let rn = &state.rename;
    let Some(old) = state.file_path.get() else {
        return;
    };
    let dir = old.parent().map(Path::to_path_buf).unwrap_or_default();
    match validate(&rn.name.get(), &dir, &old) {
        Err(e) => rn.error.set(e),
        Ok(dest) => {
            if dest == old {
                state.show_rename.set(false);
                return;
            }
            // 先释放对旧文件的占用：IconSession 持有打开的 zip，Windows 上改名会被拒绝。
            *state.session.borrow_mut() = None;
            match std::fs::rename(&old, &dest) {
                Ok(()) => {
                    state.show_rename.set(false);
                    ctx.toast_ok(tr!("rename.success"));
                    actions::open_file(state, ctx, dest);
                }
                Err(e) => {
                    rn.error.set(tr!("rename.failed", error = e.to_string()));
                    // 改名失败，恢复会话以便继续切换图标。
                    actions::open_file(state, ctx, old);
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
            .outline()
            .on_click(move |_| {
                st.rename.name.set(preset(&st, which));
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
                .outline()
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
    }
}
