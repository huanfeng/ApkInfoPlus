//! 概览头：卡片里放图标 + 应用名 / 包名 / 徽章 / 版本 / 摘要，右侧是这份文件的操作按钮。

use std::rc::Rc;

use apkinfo_core::info::{format_file_size, sdk_version_text, ApkInfo};
use windui::prelude::*;

use crate::actions;
use crate::icons;
use crate::state::{AppState, Doc};

pub const ICON_SIZE: i32 = 96;

pub fn build(state: &AppState, doc: Rc<Doc>, info: Rc<ApkInfo>) -> Element {
    Element::col()
        .width_match()
        .padding_edges(Insets::new(0, super::PAGE_PAD, super::PAGE_PAD, 0))
        .child(
            Element::row()
                .width_match()
                .cross(Align::Start)
                .spacing(18)
                .padding(16)
                .bg_role(Role::Surface)
                .corner(super::CARD_CORNER)
                .border_role(Role::Border, 1)
                .child(icon_block(state, doc.clone()))
                .child(text_block(doc.clone(), info.clone()).weight(1.0))
                .child(action_block(state, doc)),
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

    let name_label = super::with_copy_menu(
        Element::label(label_text)
            .font_size(24.0)
            .font_weight(700)
            .fg_role(Role::Text)
            .max_lines(2)
            .truncate(Truncate::End),
        doc.display_label(),
    );
    // 标题行：应用名 + 格式徽章（+ 切换语言），放不下就折行；名字本身过长时最多两行。
    let mut name_row = Element::flow()
        .width_match()
        .spacing(10)
        .run_spacing(4)
        .child(name_label)
        .child(super::kind_badge(info.kind));
    if has_alternatives {
        let d = doc.clone();
        name_row = name_row.child(
            icons::button("globe", 26, tr!("action.switch_label_language"))
                .on_click(move |ctx| actions::show_label_menu(&d, ctx)),
        );
    }
    // 签名方案合成一枚徽章（「签名 v1 · v2 · v3」），解析完成后出现。
    // 不用 host_signal——它是撑满的 col，放进这一行会把整块文字区撑开。
    let signature = doc.signature;
    let schemes = Signal::map(signature, |v| {
        let Some(s) = v.first() else {
            return String::new();
        };
        let names: Vec<&str> = [
            (s.has_v1, "v1"),
            (s.has_v2, "v2"),
            (s.has_v3, "v3"),
            (s.has_v31, "v3.1"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, n)| *n)
        .collect();
        if names.is_empty() {
            String::new()
        } else {
            format!("{} {}", tr!("tab.signature"), names.join(" · "))
        }
    });
    let version = match (&meta.version_name, meta.version_code) {
        (Some(n), Some(c)) => format!("{} ({})", n, c),
        (Some(n), None) => n.clone(),
        (None, Some(c)) => format!("({})", c),
        (None, None) => String::new(),
    };

    // 版本行：版本号 + 签名 / 分包徽章，同样放不下就折行，版本号过长自身换行。
    let mut version_row = Element::flow()
        .width_match()
        .spacing(8)
        .run_spacing(4)
        .child(super::with_copy_menu(
            Element::label(version.clone())
                .font_size(16.0)
                .font_weight(500)
                .fg_role(Role::Text),
            version.clone(),
        ))
        .child(
            Element::badge_intent(schemes, Intent::Success)
                .visible_when(move || schemes.with(|t| !t.is_empty())),
        );
    if info.is_archive() {
        version_row = version_row.child(Element::badge_intent(
            tr!("field.split_badge", count = info.splits.len() as i64),
            Intent::Primary,
        ));
        if !info.obbs.is_empty() {
            version_row = version_row.child(Element::badge_intent("OBB", Intent::Warning));
        }
    }

    let package = meta.package_name.clone().unwrap_or_default();
    let package_row = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(4)
        .child(super::with_copy_menu(
            Element::label(package.clone())
                .font_size(14.0)
                .fg_role(Role::TextMuted)
                .font_family(super::mono_family()),
            package.clone(),
        ))
        .child(super::copy_button(package.clone()).visible(!package.is_empty()));

    // 摘要一行只放数字，完整的 Android 版本名放在悬停提示里，免得信息少时也折成两三行。
    let mut summary = vec![format_file_size(info.size)];
    let mut tip = vec![format_file_size(info.size)];
    if let Some(min) = meta.min_sdk_version {
        summary.push(format!("minSdk {}", min));
        tip.push(format!("minSdk {}", sdk_version_text(Some(min))));
    }
    if let Some(t) = meta.target_sdk_version {
        summary.push(format!("targetSdk {}", t));
        tip.push(format!("targetSdk {}", sdk_version_text(Some(t))));
    }
    if let Some(c) = meta.compile_sdk_version {
        tip.push(format!("compileSdk {}", sdk_version_text(Some(c))));
    }
    if !meta.native_codes.is_empty() {
        summary.push(meta.native_codes.join(", "));
        tip.push(meta.native_codes.join(", "));
    }

    Element::col()
        .width_match()
        .spacing(6)
        .child(name_row)
        .child(package_row)
        .child(version_row)
        .child(
            Element::label(summary.join("  ·  "))
                .font_size(12.5)
                .fg_role(Role::TextSubtle)
                .max_lines(1)
                .truncate(Truncate::End)
                .tooltip(tip.join("\n")),
        )
}

/// 右侧动作：纵向一列——安装（主操作）· 重命名 · 更多（重新解析 / 导出 / 路径 / 关闭都在菜单里）。
fn action_block(state: &AppState, doc: Rc<Doc>) -> Element {
    let st = state.clone();
    Element::col()
        .cross(Align::Stretch)
        .spacing(6)
        .width(112)
        .child(
            Element::button(t!("action.install"))
                .icon_content(icons::content("play", icons::ICON_WHITE))
                .tooltip(format!("{} (Ctrl+I)", tr!("install.install_to_device")))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_install(&st, ctx)
                }),
        )
        .child(
            Element::button(t!("action.rename"))
                .icon_content(icons::gray("pencil"))
                .neutral()
                .outline_soft()
                .tooltip(format!("{} (F2)", tr!("rename.title")))
                .on_click({
                    let st = st.clone();
                    move |ctx| actions::open_rename(&st, ctx)
                }),
        )
        .child(
            Element::button(t!("action.more"))
                .icon_content(icons::gray("more"))
                .neutral()
                .outline_soft()
                .on_click({
                    let st = st.clone();
                    let d = doc.clone();
                    move |ctx| {
                        let items = actions::more_menu_items(&st, &d);
                        let mut req = windui::event::MenuRequest::below(ctx.bounds(), items);
                        req.click_through = false;
                        ctx.show_menu_request(req);
                    }
                }),
        )
}
