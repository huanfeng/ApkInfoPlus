//! 主窗口：自绘标题栏 → [空状态拖放区 | 侧栏文件列表 ‖ 当前文档（概览头 + 标签页）]。
//!
//! 多个文件同时打开：左侧侧栏列出全部，点一下切换；内容区只为当前文档建一棵树
//! （`host_signal(active)`），切换时重建，各文档自己的信号（选中的标签页、过滤词）保留。
//! 侧栏与内容区之间是可拖的分隔条，比例存在 `state.sidebar_ratio`，退出时写回配置。

use std::rc::Rc;

use apkinfo_core::info::ApkInfo;
use windui::prelude::*;

use super::{header, pages, rename};
use crate::actions;
use crate::icons;
use crate::state::{AppState, Doc, DocEntry};

/// 标题栏高度。
const TITLE_H: i32 = 48;

pub fn build(state: &AppState) -> Element {
    let st = state.clone();
    let root = Element::col()
        .fill()
        .bg_role(Role::Bg)
        .child(title_bar(state))
        .child(body(state).weight(1.0))
        .on_drop_files(move |ctx, paths| {
            actions::open_paths(&st, ctx, paths.to_vec());
        });

    Element::stack()
        .fill()
        .child(root)
        .child(rename::build_dialog(state))
}

/// 自绘标题栏：logo + 应用名 | 搜索框 | 打开 | … | 设置 · 明暗 | 窗口按钮。
/// macOS 保留系统标题栏（红绿灯在左上），这一条就只当工具栏用，不放窗口按钮。
fn title_bar(state: &AppState) -> Element {
    let st = state.clone();
    let filter = state.sidebar_filter;
    let docs = state.docs;

    let left = Element::row()
        .cross(Align::Center)
        .spacing(10)
        .padding_edges(Insets::new(14, 0, 0, 0))
        .child(
            Element::image_content(ImageContent::from_bytes(include_bytes!(
                "../../assets/icon.png"
            )))
            .size(22, 22),
        )
        .child(
            Element::label(crate::state::APP_TITLE)
                .font_size(15.0)
                .font_weight(600)
                .fg_role(Role::Text),
        )
        .child(
            Element::text_input(filter, t!("sidebar.search"))
                .width(300)
                .margin_xy(12, 0)
                .enabled_when(move || docs.with(|v| !v.is_empty())),
        )
        .child(
            Element::button(t!("action.open_apk"))
                .icon_content(icons::gray("folder_open"))
                .small()
                .neutral()
                .outline()
                .tooltip("Ctrl+O")
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::pick_file(&st, ctx)
                }),
        )
        .weight(1.0);

    let dark = state.cfg(|c| crate::theme::is_dark(c, windui::event::system_prefers_dark()));
    let right = Element::row()
        .cross(Align::Center)
        .spacing(2)
        .padding_edges(Insets::new(0, 0, 6, 0))
        .child(
            icons::button("settings", 32, tr!("action.settings")).on_click({
                let st = st.clone();
                move |ctx| actions::open_settings(&st, ctx)
            }),
        )
        .child(
            icons::button(
                if dark { "sun" } else { "moon" },
                32,
                tr!("action.toggle_theme"),
            )
            .on_click({
                let st = st.clone();
                move |_| actions::toggle_theme(&st)
            }),
        );

    let mut bar = Element::row()
        .width_match()
        .height(TITLE_H)
        .cross(Align::Stretch)
        .bg_role(Role::Bg)
        .window_drag()
        .child(left)
        .child(right);
    if !cfg!(target_os = "macos") {
        bar = bar
            .child(
                Element::col()
                    .width(1)
                    .height(18)
                    .bg_role(Role::Border)
                    .align(Align::Center)
                    .margin_xy(6, 0),
            )
            .child(Element::window_button(WindowButtonKind::Minimize).fg_role(Role::TextMuted))
            .child(Element::window_button(WindowButtonKind::Maximize).fg_role(Role::TextMuted))
            .child(Element::window_button(WindowButtonKind::Close).fg_role(Role::TextMuted));
    }
    bar
}

fn body(state: &AppState) -> Element {
    let docs = state.docs;
    let workspace = Element::split_opts(
        Axis::Horizontal,
        sidebar(state),
        content(state),
        state.sidebar_ratio,
        SplitOpts {
            thickness: 6,
            min: 0.14,
            max: 0.45,
        },
    )
    .fill()
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

    let head = Element::row()
        .width_match()
        .cross(Align::Center)
        .padding_edges(Insets::new(16, 12, 12, 4))
        .child(
            Element::label(Signal::map(docs, |v| {
                tr!("sidebar.title", count = v.len() as i64)
            }))
            .font_size(13.0)
            .font_weight(600)
            .fg_role(Role::TextMuted)
            .weight(1.0),
        )
        .child(
            Element::link(t!("sidebar.close_all"))
                .font_size(12.5)
                .enabled_when(move || docs.with(|v| v.len() > 1))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::close_all(&st, ctx)
                }),
        );

    let list = Element::host_signal(state.entries, {
        let st = st.clone();
        move |e: DocEntry| doc_row(&st, e)
    })
    .width_match()
    .padding_edges(Insets::new(8, 2, 8, 8))
    .spacing(2);

    Element::col()
        .height_match()
        .bg_role(Role::Surface)
        .child(head)
        .child(Element::scroll().width_match().weight(1.0).child(list))
}

fn doc_row(state: &AppState, entry: DocEntry) -> Element {
    let doc = entry.doc.clone();
    let st = state.clone();
    let filter = state.sidebar_filter;
    let file_name = doc.file_name();
    // 解析完显示应用名 + 版本，解析中 / 失败显示文件名 + 状态。
    let info = doc.current_info();
    let (title, subtitle) = match &info {
        Some(i) => {
            let label = doc.display_label();
            let name = if label.trim().is_empty() {
                file_name.clone()
            } else {
                label
            };
            let version = match (&i.meta.version_name, i.meta.version_code) {
                (Some(n), Some(c)) => format!("{} ({})", n, c),
                (Some(n), None) => n.clone(),
                (None, Some(c)) => c.to_string(),
                (None, None) => i.meta.package_name.clone().unwrap_or_default(),
            };
            (name, version)
        }
        None => (file_name.clone(), doc.subtitle.get()),
    };
    let package = info
        .as_ref()
        .and_then(|i| i.meta.package_name.clone())
        .unwrap_or_default();

    // 图标：有就画位图，没有（解析中 / 失败）用包图标占位。
    let icon_sig = doc.icon;
    let image = Element::host_signal(doc.icon, |img: Rc<apkinfo_core::icon::RgbaImage>| {
        Element::image_rgba(img.width, img.height, &img.data)
            .fit(Fit::Contain)
            .size(40, 40)
            .corner(10.0)
    });
    let placeholder = Element::stack()
        .size(40, 40)
        .corner(10.0)
        .bg_role(Role::SurfaceAlt)
        .child(
            Element::image_content(icons::gray("package"))
                .size(20, 20)
                .align_xy(Align::Center, Align::Center),
        )
        .visible_when(move || icon_sig.with(|v| v.is_empty()));
    let icon = Element::stack()
        .size(40, 40)
        .child(placeholder)
        .child(image);

    let texts = Element::col()
        .weight(1.0)
        .spacing(2)
        .child(
            Element::label(title.clone())
                .font_size(13.5)
                .font_weight(600)
                .fg_role(Role::Text)
                .max_lines(1)
                .truncate(Truncate::End)
                .width_match(),
        )
        .child(
            Element::label(subtitle.clone())
                .font_size(12.0)
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
        .padding_edges(Insets::new(8, 7, 4, 7))
        .corner(10.0)
        .clickable()
        .tooltip(file_name.clone())
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
    let hay = format!("{}\n{}\n{}\n{}", title, subtitle, file_name, package).to_lowercase();
    row.visible_when(move || {
        filter.with(|f| {
            let f = f.trim().to_lowercase();
            f.is_empty() || hay.contains(&f)
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
    Element::col().fill().padding(super::PAGE_PAD).child(
        Element::col()
            .fill()
            .cross(Align::Center)
            .spacing(10)
            .corner(super::CARD_CORNER)
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
