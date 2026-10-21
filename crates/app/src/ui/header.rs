//! 概览头：图标 + 应用名 / 包名 / 版本 / 类型徽章，右侧图标候选切换与导出。

use std::rc::Rc;

use apkinfo_core::info::{format_file_size, sdk_version_text, ApkInfo};
use windui::prelude::*;

use crate::actions;
use crate::state::AppState;

pub const ICON_SIZE: i32 = 96;

pub fn build(state: &AppState, info: Rc<ApkInfo>) -> Element {
    Element::row()
        .width_match()
        .cross(Align::Start)
        .spacing(18)
        .padding_edges(Insets::new(super::PAGE_PAD, 16, super::PAGE_PAD, 12))
        .child(icon_block(state))
        .child(text_block(state, info.clone()).weight(1.0))
}

/// 图标区：图片（随 `state.icon` 重建）+ 候选切换条。
fn icon_block(state: &AppState) -> Element {
    let st = state.clone();
    let image = Element::host_signal(state.icon, move |img: Rc<apkinfo_core::icon::RgbaImage>| {
        Element::image_rgba(img.width, img.height, &img.data)
            .fit(Fit::Contain)
            .size(ICON_SIZE, ICON_SIZE)
            .corner(ICON_SIZE as f32 * 0.22)
    });
    let placeholder_visible = {
        let icon = state.icon;
        move || icon.with(|v| v.is_empty())
    };
    let loading = state.icon_loading;
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
            move || actions::icon_menu_items(&st)
        });

    let candidates = state.icon_candidates;
    let index = state.icon_index;
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
            crate::icons::button("chevron_left", 22, tr!("action.prev_icon")).on_click({
                let st = st.clone();
                move |ctx| actions::switch_icon(&st, ctx, -1)
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
            crate::icons::button("chevron_right", 22, tr!("action.next_icon")).on_click({
                let st = st.clone();
                move |ctx| actions::switch_icon(&st, ctx, 1)
            }),
        )
        .visible_when(move || candidates.with(|c| c.len() > 1));

    Element::col()
        .spacing(4)
        .cross(Align::Center)
        .child(icon_stack)
        .child(nav)
}

fn text_block(state: &AppState, info: Rc<ApkInfo>) -> Element {
    let st = state.clone();
    let meta = &info.meta;

    // 应用名随语言选择变化：派生信号，读 label_locale 现算。
    let label_locale = state.label_locale;
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
        let st2 = st.clone();
        name_row = name_row.child(
            crate::icons::button("globe", 26, tr!("action.switch_label_language"))
                .on_click(move |ctx| actions::show_label_menu(&st2, ctx)),
        );
    }
    name_row = name_row.child(Element::badge_intent(
        info.kind.display_name(),
        Intent::Neutral,
    ));

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
