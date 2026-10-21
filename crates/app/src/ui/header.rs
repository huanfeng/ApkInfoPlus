//! 概览头：卡片里放图标 + 应用名 / 包名 / 版本 / 类型徽章 + 摘要，右侧是「安装到设备」。

use std::rc::Rc;

use apkinfo_core::info::{format_file_size, sdk_version_text, ApkInfo, SignatureInfo};
use windui::prelude::*;

use crate::actions;
use crate::icons;
use crate::state::{AppState, Doc};

pub const ICON_SIZE: i32 = 96;

pub fn build(state: &AppState, doc: Rc<Doc>, info: Rc<ApkInfo>) -> Element {
    Element::col()
        .width_match()
        .padding_edges(Insets::new(super::PAGE_PAD, 14, super::PAGE_PAD, 0))
        .child(
            Element::row()
                .width_match()
                .cross(Align::Start)
                .spacing(18)
                .padding(16)
                .bg_role(Role::Surface)
                .corner(12.0)
                .border_role(Role::Border, 1)
                .child(icon_block(state, doc.clone()))
                .child(text_block(doc.clone(), info.clone()).weight(1.0))
                .child(action_block(state)),
        )
}

/// 图标区：图片（随 `doc.icon` 重建）+ 候选切换条。
fn icon_block(state: &AppState, doc: Rc<Doc>) -> Element {
    let st = state.clone();
    let image = Element::host_signal(doc.icon, move |img: Rc<apkinfo_core::icon::RgbaImage>| {
        Element::image_rgba(img.width, img.height, &img.data)
            .fit(Fit::Contain)
            .size(ICON_SIZE, ICON_SIZE)
            .corner(ICON_SIZE as f32 * 0.22)
    });
    let placeholder_visible = {
        let icon = doc.icon;
        move || icon.with(|v| v.is_empty())
    };
    let loading = doc.icon_loading;
    let placeholder = Element::stack()
        .size(ICON_SIZE, ICON_SIZE)
        .corner(ICON_SIZE as f32 * 0.22)
        .bg_role(Role::SurfaceAlt)
        .border_role(Role::Border, 1)
        .child(
            Element::label(t!("icon.no_icon"))
                .font_size(12.0)
                .fg_role(Role::TextSubtle)
                .align_xy(Align::Center, Align::Center)
                .visible_when(move || !loading.get()),
        )
        .child(
            Element::label(t!("icon.loading"))
                .font_size(12.0)
                .fg_role(Role::TextSubtle)
                .align_xy(Align::Center, Align::Center)
                .visible_when(move || loading.get()),
        )
        .visible_when(placeholder_visible);

    let icon_stack = Element::stack()
        .size(ICON_SIZE, ICON_SIZE)
        .child(placeholder)
        .child(image)
        .on_context_menu({
            let st = st.clone();
            let doc = doc.clone();
            move || actions::icon_menu_items(&st, &doc)
        });

    let candidates = doc.icon_candidates;
    let index = doc.icon_index;
    let counter = Signal::map(candidates, move |c| {
        if c.len() <= 1 {
            String::new()
        } else {
            format!("{} / {}", index.get() + 1, c.len())
        }
    });
    let nav = Element::row()
        .width(ICON_SIZE)
        .cross(Align::Center)
        .child(
            icons::button("chevron_left", 22, tr!("action.prev_icon")).on_click({
                let st = st.clone();
                let doc = doc.clone();
                move |_| actions::switch_icon(&st, &doc, -1)
            }),
        )
        .child(
            Element::label(counter)
                .font_size(11.0)
                .fg_role(Role::TextMuted)
                .text_align(Align::Center)
                .weight(1.0),
        )
        .child(
            icons::button("chevron_right", 22, tr!("action.next_icon")).on_click({
                let st = st.clone();
                let doc = doc.clone();
                move |_| actions::switch_icon(&st, &doc, 1)
            }),
        )
        .visible_when(move || candidates.with(|c| c.len() > 1));

    Element::col()
        .spacing(4)
        .cross(Align::Center)
        .child(icon_stack)
        .child(nav)
}

fn text_block(doc: Rc<Doc>, info: Rc<ApkInfo>) -> Element {
    let meta = &info.meta;

    // 应用名随语言选择变化：派生信号，读 label_locale 现算。
    let label_locale = doc.label_locale;
    let info_for_label = info.clone();
    let label_text = Signal::map(label_locale, move |loc| {
        info_for_label
            .display_label(loc.as_deref())
            .unwrap_or("")
            .to_string()
    });
    let has_alternatives = !info.alternative_labels().is_empty();

    let mut name_row = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(8)
        .child(
            Element::label(label_text)
                .font_size(22.0)
                .font_weight(700)
                .fg_role(Role::Text)
                .max_lines(1)
                .truncate(Truncate::End),
        );
    if has_alternatives {
        let d = doc.clone();
        name_row = name_row.child(
            icons::button("globe", 26, tr!("action.switch_label_language"))
                .on_click(move |ctx| actions::show_label_menu(&d, ctx)),
        );
    }
    name_row = name_row.child(Element::badge_intent(
        info.kind.display_name(),
        Intent::Primary,
    ));
    // 签名方案徽章：签名解析完成后出现（v1 / v2 / v3 / v3.1）。
    // 不用 host_signal——它是撑满的 col，放进这一行会把整块文字区撑开。
    let signature = doc.signature;
    for (scheme, pick) in [
        (
            "v1",
            (|s: &SignatureInfo| s.has_v1) as fn(&SignatureInfo) -> bool,
        ),
        ("v2", |s| s.has_v2),
        ("v3", |s| s.has_v3),
        ("v3.1", |s| s.has_v31),
    ] {
        name_row = name_row.child(
            Element::badge_intent(
                format!("{} {}", tr!("tab.signature"), scheme),
                Intent::Success,
            )
            .visible_when(move || signature.with(|v| v.first().is_some_and(|s| pick(s)))),
        );
    }

    let package = meta.package_name.clone().unwrap_or_default();
    let version = match (&meta.version_name, meta.version_code) {
        (Some(n), Some(c)) => format!("{} ({})", n, c),
        (Some(n), None) => n.clone(),
        (None, Some(c)) => format!("({})", c),
        (None, None) => String::new(),
    };

    let mut summary = vec![format_file_size(info.size)];
    if let Some(min) = meta.min_sdk_version {
        summary.push(format!("minSdk {}", sdk_version_text(Some(min))));
    }
    if let Some(t) = meta.target_sdk_version {
        summary.push(format!("targetSdk {}", sdk_version_text(Some(t))));
    }
    if !meta.native_codes.is_empty() {
        summary.push(meta.native_codes.join(", "));
    }

    Element::col()
        .width_match()
        .spacing(4)
        .child(name_row)
        .child(
            Element::row()
                .width_match()
                .cross(Align::Center)
                .spacing(6)
                .child(
                    Element::label(package.clone())
                        .font_size(14.0)
                        .fg_role(Role::TextMuted)
                        .font_family(super::mono_family()),
                )
                .child(
                    super::copy_button(package)
                        .visible(!meta.package_name.as_deref().unwrap_or("").is_empty()),
                ),
        )
        .child(Element::label(version).font_size(14.0).fg_role(Role::Text))
        .child(
            Element::label(summary.join("  ·  "))
                .font_size(12.5)
                .fg_role(Role::TextSubtle),
        )
}

/// 右侧动作：安装到设备（与工具栏同一入口，放在最显眼的位置）+ 目标设备选择。
fn action_block(state: &AppState) -> Element {
    let st = state.clone();
    let selected = state.selected_device;
    let device_label = Signal::map(selected, |d| match d {
        Some((_, name)) => name.clone(),
        None => tr!("install.pick_device"),
    });
    Element::col()
        .spacing(8)
        .cross(Align::Stretch)
        .child(
            Element::button(t!("install.install_to_device"))
                .icon_content(icons::content("play", icons::ICON_WHITE))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_install(&st, ctx)
                }),
        )
        .child(
            Element::button(device_label)
                .icon_content(icons::gray("smartphone"))
                .neutral()
                .outline()
                .tooltip(tr!("install.select_device"))
                .on_click(move |ctx| actions::show_device_menu(&st, ctx)),
        )
}
