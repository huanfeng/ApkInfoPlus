//! 主窗口：工具栏 → [空状态拖放区 | 侧栏文件列表 + 当前文档（概览头 + 标签页）] → 状态栏。
//!
//! 多个文件同时打开：左侧侧栏列出全部，点一下切换；内容区只为当前文档建一棵树
//! （`host_signal(active)`），切换时重建，各文档自己的信号（选中的标签页、过滤词）保留。

use std::rc::Rc;

use apkinfo_core::info::ApkInfo;
use windui::prelude::*;

use super::{header, pages, rename};
use crate::actions;
use crate::icons;
use crate::state::{AppState, Doc, DocEntry};

/// 侧栏宽度。
const SIDEBAR_W: i32 = 272;

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
            actions::open_paths(&st, ctx, paths.to_vec());
        });

    Element::stack()
        .fill()
        .child(root)
        .child(rename::build_dialog(state))
}

fn toolbar(state: &AppState) -> Element {
    let docs = state.docs;
    let active = state.active;
    let st = state.clone();

    let has_doc = move || active.with(|v| !v.is_empty());
    let has_info = move || {
        active.with(|v| {
            v.first()
                .map(|d| d.info.with(|i| !i.is_empty()))
                .unwrap_or(false)
        })
    };
    let parsing = move || active.with(|v| v.first().map(|d| d.parsing.get()).unwrap_or(false));

    let tool = |label: &'static str, icon: &'static str, tip: String| {
        Element::button(t!(label))
            .icon_content(icons::gray(icon))
            .small()
            .neutral()
            .outline()
            .tooltip(tip)
    };
    let separator = || {
        Element::col()
            .width(1)
            .height(20)
            .bg_role(Role::Border)
            .margin_xy(4, 0)
    };

    Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(8)
        .padding_xy(12, 8)
        .bg_role(Role::Surface)
        .child(
            Element::button(t!("action.open"))
                .icon_content(icons::content("folder_open", icons::ICON_WHITE))
                .small()
                .tooltip("Ctrl+O")
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::pick_file(&st, ctx)
                }),
        )
        .child(separator())
        .child(
            tool("action.reparse", "refresh", "F5".into())
                .enabled_when(move || has_doc() && !parsing())
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::reparse(&st, ctx)
                }),
        )
        .child(
            tool("action.install", "smartphone", "Ctrl+I".into())
                .enabled_when(has_info)
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_install(&st, ctx)
                }),
        )
        .child(
            tool("action.rename", "pencil", "F2".into())
                .enabled_when(has_info)
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_rename(&st, ctx)
                }),
        )
        .child(
            tool(
                "action.export_icon",
                "image_down",
                tr!("action.export_icon"),
            )
            .enabled_when(move || {
                active.with(|v| {
                    v.first()
                        .map(|d| d.icon.with(|i| !i.is_empty()))
                        .unwrap_or(false)
                })
            })
            .on_click({
                let st = st.clone();
                move |ctx| {
                    let Some(doc) = st.active_doc() else { return };
                    let items = actions::icon_menu_items(&st, &doc);
                    let mut req = windui::event::MenuRequest::below(ctx.bounds(), items);
                    req.click_through = false;
                    ctx.show_menu_request(req);
                }
            }),
        )
        .child(separator())
        .child(
            tool("action.close_file", "x", "Ctrl+W".into())
                .enabled_when(has_doc)
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::close_file(&st, ctx)
                }),
        )
        .child(Element::flex_spacer())
        .child(
            Element::label(Signal::map(docs, |v| {
                if v.len() > 1 {
                    tr!("sidebar.open_count", count = v.len() as i64)
                } else {
                    String::new()
                }
            }))
            .font_size(12.0)
            .fg_role(Role::TextSubtle),
        )
        .child(
            Element::button(t!("action.settings"))
                .icon_content(icons::gray("settings"))
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
    let docs = state.docs;
    let workspace = Element::row()
        .fill()
        .cross(Align::Stretch)
        .child(sidebar(state))
        .child(Element::col().width(1).height_match().bg_role(Role::Border))
        .child(content(state).weight(1.0))
        .visible_when(move || docs.with(|v| !v.is_empty()));

    Element::stack()
        .fill()
        .child(empty_state(state).visible_when(move || docs.with(|v| v.is_empty())))
        .child(workspace)
}

// ---------------------------------------------------------------------------
// 侧栏：打开的文件列表
// ---------------------------------------------------------------------------

fn sidebar(state: &AppState) -> Element {
    let st = state.clone();
    let docs = state.docs;
    let filter = state.sidebar_filter;

    let head = Element::row()
        .width_match()
        .cross(Align::Center)
        .padding_edges(Insets::new(14, 12, 10, 6))
        .child(
            Element::label(Signal::map(docs, |v| {
                tr!("sidebar.title", count = v.len() as i64)
            }))
            .font_size(13.5)
            .font_weight(600)
            .fg_role(Role::Text)
            .weight(1.0),
        )
        .child(
            Element::button(t!("sidebar.close_all"))
                .small()
                .neutral()
                .outline()
                .enabled_when(move || docs.with(|v| v.len() > 1))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::close_all(&st, ctx)
                }),
        );

    let search = Element::text_input(filter, t!("sidebar.search"))
        .width_match()
        .margin_xy(10, 0);

    let list = Element::host_signal(state.entries, {
        let st = st.clone();
        move |e: DocEntry| doc_row(&st, e)
    })
    .width_match()
    .padding_edges(Insets::new(8, 6, 8, 8))
    .spacing(2);

    Element::col()
        .width(SIDEBAR_W)
        .height_match()
        .bg_role(Role::Surface)
        .spacing(6)
        .child(head)
        .child(search)
        .child(Element::scroll().width_match().weight(1.0).child(list))
}

fn doc_row(state: &AppState, entry: DocEntry) -> Element {
    let doc = entry.doc.clone();
    let st = state.clone();
    let filter = state.sidebar_filter;
    let name = doc.file_name();

    // 图标：有就画位图，没有（解析中 / 失败）用包图标占位。
    let icon_sig = doc.icon;
    let image = Element::host_signal(doc.icon, |img: Rc<apkinfo_core::icon::RgbaImage>| {
        Element::image_rgba(img.width, img.height, &img.data)
            .fit(Fit::Contain)
            .size(36, 36)
            .corner(8.0)
    });
    let placeholder = Element::stack()
        .size(36, 36)
        .corner(8.0)
        .bg_role(Role::SurfaceAlt)
        .child(
            Element::image_content(icons::gray("package"))
                .size(18, 18)
                .align_xy(Align::Center, Align::Center),
        )
        .visible_when(move || icon_sig.with(|v| v.is_empty()));
    let icon = Element::stack()
        .size(36, 36)
        .child(placeholder)
        .child(image);

    let texts = Element::col()
        .weight(1.0)
        .spacing(2)
        .child(
            Element::label(name.clone())
                .font_size(13.0)
                .font_weight(if entry.active { 600 } else { 400 })
                .fg_role(Role::Text)
                .max_lines(1)
                .truncate(Truncate::Middle)
                .width_match(),
        )
        .child(
            Element::label(doc.subtitle)
                .font_size(11.5)
                .fg_role(Role::TextMuted)
                .max_lines(1)
                .truncate(Truncate::End)
                .width_match(),
        );

    let close = icons::button("x", 22, tr!("action.close_file")).on_click({
        let st = st.clone();
        let d = doc.clone();
        move |ctx| actions::close_doc(&st, ctx, &d)
    });

    let mut row = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .padding_edges(Insets::new(8, 6, 4, 6))
        .corner(8.0)
        .clickable()
        .on_click({
            let st = st.clone();
            let id = doc.id;
            move |_| st.set_active(id)
        })
        .child(icon)
        .child(texts)
        .child(close);
    if entry.active {
        row = row.bg_role_alpha(Role::Accent, 0.12);
    }
    let name_lower = name.to_lowercase();
    let subtitle = doc.subtitle;
    row.visible_when(move || {
        filter.with(|f| {
            let f = f.trim().to_lowercase();
            f.is_empty()
                || name_lower.contains(&f)
                || subtitle.with(|s| s.to_lowercase().contains(&f))
        })
    })
}

// ---------------------------------------------------------------------------
// 内容区：当前文档
// ---------------------------------------------------------------------------

fn content(state: &AppState) -> Element {
    let st = state.clone();
    Element::host_signal(state.active, move |doc: Rc<Doc>| {
        let st = st.clone();
        let info_sig = doc.info;
        let pending =
            pending_panel(&st, doc.clone()).visible_when(move || info_sig.with(|v| v.is_empty()));
        let loaded = Element::host_signal(doc.info, {
            let st = st.clone();
            let doc = doc.clone();
            move |info: Rc<ApkInfo>| {
                Element::col()
                    .fill()
                    .child(header::build(&st, doc.clone(), info.clone()))
                    .child(pages::build_tabs(&st, doc.clone(), info).weight(1.0))
            }
        })
        .fill();
        Element::stack().fill().child(pending).child(loaded)
    })
    .fill()
}

/// 解析中 / 解析失败时的内容区。
fn pending_panel(state: &AppState, doc: Rc<Doc>) -> Element {
    let parsing = doc.parsing;
    let error = doc.parse_error;
    let st = state.clone();
    Element::col()
        .fill()
        .cross(Align::Center)
        .spacing(10)
        .padding(40)
        .child(Element::flex_spacer())
        .child(Element::image_content(icons::gray("package")).size(56, 56))
        .child(
            Element::label(doc.file_name())
                .font_size(16.0)
                .font_weight(600)
                .fg_role(Role::Text)
                .max_lines(1)
                .truncate(Truncate::Middle),
        )
        .child(
            Element::label(t!("parse.parsing"))
                .font_size(13.0)
                .fg_role(Role::Accent)
                .visible_when(move || parsing.get()),
        )
        .child(
            Element::label(error)
                .font_size(13.0)
                .fg_role(Role::Danger)
                .text_align(Align::Center)
                .width_match()
                .padding_xy(40, 0)
                .visible_when(move || error.with(|e| !e.is_empty())),
        )
        .child(
            Element::button(t!("action.reparse"))
                .small()
                .neutral()
                .outline()
                .visible_when(move || !parsing.get())
                .on_click({
                    let doc = doc.clone();
                    move |ctx| actions::reparse_doc(&st, ctx, &doc)
                }),
        )
        .child(Element::flex_spacer())
}

/// 没打开文件时的大拖放区。
fn empty_state(state: &AppState) -> Element {
    let st = state.clone();
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
            .child(Element::image_content(icons::gray("package")).size(72, 72))
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
            .child(Element::flex_spacer()),
    )
}

fn status_bar(state: &AppState) -> Element {
    let st = state.clone();
    let active = state.active;
    let path_text = Signal::map(active, |v| match v.first() {
        Some(d) => d.path.with(|p| p.display().to_string()),
        None => tr!("status.ready"),
    });
    let has_doc = move || active.with(|v| !v.is_empty());
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
            icons::button("copy", 22, tr!("action.copy_path"))
                .visible_when(has_doc)
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::copy_path(&st, ctx)
                }),
        )
        .child(
            icons::button("folder", 22, tr!("action.open_dir"))
                .visible_when(has_doc)
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_in_file_manager(&st, ctx)
                }),
        )
}
