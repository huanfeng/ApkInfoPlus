//! 主窗口：工具栏 → [空状态拖放区 | 概览头 + 标签页] → 状态栏，外加模态对话框。

use std::rc::Rc;

use apkinfo_core::info::ApkInfo;
use windui::prelude::*;

use super::{header, install, pages, rename};
use crate::actions;
use crate::state::AppState;

pub fn build(state: &AppState) -> Element {
    let st = state.clone();
    let root = Element::col()
        .fill()
        .bg_role(Role::Bg)
        .child(toolbar(state))
        .child(Element::divider())
        .child(body(state).weight(1.0))
        .child(Element::divider())
        .child(status_bar(state))
        .on_drop_files(move |ctx, paths| {
            if let Some(p) = paths.first() {
                actions::open_dropped(&st, ctx, p.clone());
            }
        });

    Element::stack()
        .fill()
        .child(root)
        .child(install::build_dialog(state))
        .child(rename::build_dialog(state))
}

fn toolbar(state: &AppState) -> Element {
    let has_file = state.file_path;
    let has_info = state.info;
    let parsing = state.parsing;
    let st = state.clone();

    let tool = |label: &'static str, tip: String| {
        Element::button(t!(label))
            .small()
            .neutral()
            .outline()
            .tooltip(tip)
    };

    Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(8)
        .padding_xy(12, 8)
        .bg_role(Role::Surface)
        .child(
            Element::button(t!("action.open"))
                .small()
                .tooltip("Ctrl+O")
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::pick_file(&st, ctx)
                }),
        )
        .child(
            tool("action.reparse", "F5".into())
                .enabled_when(move || has_file.with(|p| p.is_some()) && !parsing.get())
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::reparse(&st, ctx)
                }),
        )
        .child(
            tool("action.install", "Ctrl+I".into())
                .enabled_when(move || has_info.with(|v| !v.is_empty()))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_install(&st, ctx)
                }),
        )
        .child(
            tool("action.rename", "F2".into())
                .enabled_when(move || has_info.with(|v| !v.is_empty()))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_rename(&st, ctx)
                }),
        )
        .child(
            tool("action.export_icon", tr!("action.export_icon"))
                .enabled_when({
                    let icon = st.icon;
                    move || icon.with(|v| !v.is_empty())
                })
                .on_click({
                    let st = st.clone();
                    move |ctx| {
                        let items = actions::icon_menu_items(&st);
                        let mut req = windui::event::MenuRequest::below(ctx.bounds(), items);
                        req.click_through = false;
                        ctx.show_menu_request(req);
                    }
                }),
        )
        .child(
            tool("action.close_file", "Ctrl+W".into())
                .enabled_when(move || has_file.with(|p| p.is_some()))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::close_file(&st, ctx)
                }),
        )
        .child(Element::flex_spacer())
        .child(
            Element::label(t!("parse.parsing"))
                .font_size(12.5)
                .fg_role(Role::Accent)
                .visible_when(move || parsing.get()),
        )
        .child(
            Element::button(t!("action.settings"))
                .small()
                .neutral()
                .outline()
                .tooltip("Ctrl+,")
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_settings(&st, ctx)
                }),
        )
}

fn body(state: &AppState) -> Element {
    let info = state.info;
    let st = state.clone();
    let content = Element::host_signal(state.info, move |info: Rc<ApkInfo>| {
        Element::col()
            .fill()
            .child(header::build(&st, info.clone()))
            .child(Element::divider())
            .child(pages::build_tabs(&st, info).weight(1.0))
    })
    .fill();

    Element::stack()
        .fill()
        .child(empty_state(state).visible_when(move || info.with(|v| v.is_empty())))
        .child(content)
}

/// 没打开文件时的大拖放区。
fn empty_state(state: &AppState) -> Element {
    let st = state.clone();
    let parse_error = state.parse_error;
    let parsing = state.parsing;
    let file_path = state.file_path;
    let file_name = Signal::map(file_path, |p| {
        p.as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default()
    });

    Element::col().fill().padding(40).child(
        Element::col()
            .fill()
            .cross(Align::Center)
            .spacing(10)
            .corner(16.0)
            .border_role(Role::Border, 2)
            .bg_role_alpha(Role::Accent, 0.04)
            .clickable()
            .on_click({
                let st = st.clone();
                move |ctx| actions::pick_file(&st, ctx)
            })
            .child(Element::flex_spacer())
            .child(
                Element::image_content(crate::icons::content("package", crate::icons::ICON_GRAY))
                    .size(72, 72),
            )
            .child(
                Element::label(t!("drop.hint"))
                    .font_size(18.0)
                    .font_weight(600)
                    .fg_role(Role::Text),
            )
            .child(
                Element::label(t!("drop.or_open"))
                    .font_size(13.5)
                    .fg_role(Role::TextMuted),
            )
            .child(
                Element::label(t!("drop.supported"))
                    .font_size(12.0)
                    .fg_role(Role::TextSubtle),
            )
            .child(
                Element::label(file_name)
                    .font_size(13.0)
                    .fg_role(Role::Accent)
                    .visible_when(move || parsing.get()),
            )
            .child(
                Element::label(t!("parse.parsing"))
                    .font_size(13.0)
                    .fg_role(Role::Accent)
                    .visible_when(move || parsing.get()),
            )
            .child(
                Element::label(parse_error)
                    .font_size(13.0)
                    .fg_role(Role::Danger)
                    .text_align(Align::Center)
                    .width_match()
                    .padding_xy(40, 0)
                    .visible_when(move || parse_error.with(|e| !e.is_empty())),
            )
            .child(Element::flex_spacer()),
    )
}

fn status_bar(state: &AppState) -> Element {
    let st = state.clone();
    let file_path = state.file_path;
    let path_text = Signal::map(file_path, |p| match p {
        Some(p) => p.display().to_string(),
        None => tr!("status.ready"),
    });
    Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(6)
        .padding_xy(12, 4)
        .bg_role(Role::Surface)
        .child(
            Element::label(path_text)
                .font_size(12.0)
                .fg_role(Role::TextMuted)
                .max_lines(1)
                .truncate(Truncate::Middle)
                .weight(1.0),
        )
        .child(
            crate::icons::button("copy", 22, tr!("action.copy_path"))
                .visible_when(move || file_path.with(|p| p.is_some()))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::copy_path(&st, ctx)
                }),
        )
        .child(
            crate::icons::button("folder", 22, tr!("action.open_dir"))
                .visible_when(move || file_path.with(|p| p.is_some()))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_in_file_manager(&st, ctx)
                }),
        )
}
