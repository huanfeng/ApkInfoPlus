//! 标签页：概览 / 权限 / 分包 / 签名 / 哈希 / 文本信息。
//!
//! 每页都是纯函数 `(state, info) -> Element`，在 `ApkInfo` 变化时整体重建。

use std::rc::Rc;

use apkinfo_core::info::{format_file_size, format_file_size_full, ApkInfo, SplitType};
use windui::prelude::*;

use super::{
    copy_button, field_row, field_row_custom, field_row_with, join_lines, mono_block, page,
    section, section_with_action,
};
use crate::state::{AppState, Doc, HashState, SigState};

/// 把标签页组装好。
pub fn build_tabs(state: &AppState, doc: Rc<Doc>, info: Rc<ApkInfo>) -> Element {
    let perm_count = info.meta.permissions.len();
    let split_count = info.splits.len() + info.obbs.len();
    let perm_title = if perm_count > 0 {
        format!("{} ({})", tr!("tab.permissions"), perm_count)
    } else {
        tr!("tab.permissions")
    };
    let split_title = if split_count > 0 {
        format!("{} ({})", tr!("tab.splits"), split_count)
    } else {
        tr!("tab.splits")
    };
    let icon = crate::icons::gray;
    let mut pages = vec![
        (
            tr!("tab.overview"),
            icon("info"),
            overview(state, &doc, info.clone()),
        ),
        (perm_title, icon("key"), permissions(&doc, info.clone())),
    ];
    if info.is_archive() {
        pages.push((split_title, icon("layers"), splits(state, info.clone())));
    }
    pages.push((
        tr!("tab.signature"),
        icon("shield"),
        signature(state, &doc, info.clone()),
    ));
    pages.push((tr!("tab.hashes"), icon("hash"), hashes(state, &doc)));
    pages.push((
        tr!("tab.text_info"),
        icon("file_text"),
        text_info(state, info.clone()),
    ));

    // 标签数因归档包而变，选中索引越界时回到概览。
    let count = pages.len();
    if doc.tab.get() >= count {
        doc.tab.set(0);
    }
    Element::col()
        .fill()
        .padding_edges(Insets::new(super::PAGE_PAD, 10, super::PAGE_PAD, 0))
        .child(Element::tabs_icons(doc.tab, pages).fill())
}

fn overview(state: &AppState, doc: &Rc<Doc>, info: Rc<ApkInfo>) -> Element {
    let m = &info.meta;

    // ---- 左：基本信息 ----
    let mut basic = Element::col()
        .width_match()
        .child(field_row(
            state,
            t!("field.package_name"),
            m.package_name.clone().unwrap_or_default(),
        ))
        .child(field_row(
            state,
            t!("field.app_name"),
            m.label.clone().unwrap_or_default(),
        ))
        .child(field_row(
            state,
            t!("field.version_code"),
            m.version_code.map(|v| v.to_string()).unwrap_or_default(),
        ))
        .child(field_row(
            state,
            t!("field.version_name"),
            m.version_name.clone().unwrap_or_default(),
        ))
        .child(field_row(
            state,
            t!("field.application_class"),
            m.application_name.clone().unwrap_or_default(),
        ))
        .child(sdk_row(state, t!("field.min_sdk"), m.min_sdk_version, None))
        .child(sdk_row(
            state,
            t!("field.target_sdk"),
            m.target_sdk_version,
            None,
        ));
    if m.compile_sdk_version.is_some() {
        basic = basic.child(sdk_row(
            state,
            t!("field.compile_sdk"),
            m.compile_sdk_version,
            m.compile_sdk_version_codename.clone(),
        ));
    }
    basic = basic.child(field_row_with(
        state,
        t!("field.size"),
        format_file_size_full(info.size),
        false,
    ));
    basic = basic.child(field_row_custom(
        state,
        t!("field.app_icon"),
        icon_strip(state, doc),
        None,
    ));
    let activities: Vec<String> = m
        .launchable_activities
        .iter()
        .filter_map(|a| a.name.clone())
        .collect();
    basic = basic.child(field_row(
        state,
        t!("field.launchable_activities"),
        activities.join("\n"),
    ));
    let left = section(tr!("field.basic_info"), basic);

    // ---- 右：其他信息 ----
    let path_text = info.path.display().to_string();
    let open_dir = {
        let p = info.path.clone();
        crate::icons::button("folder", 24, tr!("action.open_dir")).on_click(move |ctx| {
            if let Err(e) = crate::actions::reveal_in_file_manager(&p) {
                ctx.toast_err(e);
            }
        })
    };
    let mut other = Element::col()
        .width_match()
        .child(field_row_custom(
            state,
            t!("field.file"),
            // 不设 width_match：在 weight 槽里它会按父宽度量、挤掉旁边的复制按钮。
            Element::row()
                .cross(Align::Center)
                .spacing(4)
                .child(
                    Element::label(path_text.clone())
                        .font_size(13.0)
                        .fg_role(Role::Text)
                        .weight(1.0),
                )
                .child(open_dir),
            Some(path_text),
        ))
        .child(field_row_with(
            state,
            t!("field.archive_type"),
            info.kind.display_name().to_string(),
            false,
        ));
    if info.is_archive() {
        if let Some(n) = &info.archive_app_name {
            other = other.child(field_row(state, t!("field.app_name"), n.clone()));
        }
        if let Some(b) = &info.base_apk_entry {
            other = other.child(field_row(state, t!("field.base_apk"), b.clone()));
        }
        if let Some(t) = info.total_size {
            other = other.child(field_row_with(
                state,
                t!("field.total_size"),
                format_file_size_full(t),
                false,
            ));
        }
        if !info.split_configs.is_empty() {
            other = other.child(field_row(
                state,
                t!("field.split_configs"),
                info.split_configs.join("  "),
            ));
        }
    }
    other = other
        .child(field_row(state, t!("field.abi"), m.native_codes.join("  ")))
        .child(field_row_with(
            state,
            t!("field.screen_size"),
            m.screen_sizes.join("  "),
            false,
        ))
        .child(field_row_with(
            state,
            t!("field.screen_density"),
            m.densities
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("  "),
            false,
        ));
    // 语言列表动辄几十上百项：只显示数量，复制按钮给完整列表。
    let locales = m.locales.join("  ");
    other = other.child(field_row_custom(
        state,
        t!("field.languages"),
        Element::label(if m.locales.is_empty() {
            tr!("field.none")
        } else {
            tr!("field.language_count", count = m.locales.len() as i64)
        })
        .font_size(13.0)
        .fg_role(if m.locales.is_empty() {
            Role::TextSubtle
        } else {
            Role::Text
        }),
        Some(locales),
    ));
    let mut right = Element::col()
        .width_match()
        .spacing(14)
        .child(section(tr!("field.other_info"), other));
    if !m.features.is_empty() || !m.features_not_required.is_empty() {
        let mut feat = Element::col().width_match();
        if !m.features.is_empty() {
            feat = feat.child(field_row(
                state,
                t!("field.features"),
                m.features.join("\n"),
            ));
        }
        if !m.features_not_required.is_empty() {
            feat = feat.child(field_row(
                state,
                t!("field.features_not_required"),
                m.features_not_required.join("\n"),
            ));
        }
        right = right.child(section(tr!("field.features"), feat));
    }

    page(
        Element::row()
            .width_match()
            .cross(Align::Start)
            .spacing(14)
            .child(Element::col().weight(5.0).child(left))
            .child(right.weight(4.0)),
    )
}

/// SDK 行：数字 + 「Android x」徽章。
fn sdk_row(
    state: &AppState,
    title: impl Into<TextContent>,
    sdk: Option<i64>,
    codename: Option<String>,
) -> Element {
    let Some(v) = sdk else {
        return field_row_with(state, title, String::new(), false);
    };
    let mut row = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(8)
        .child(
            Element::label(v.to_string())
                .font_size(13.0)
                .fg_role(Role::Text),
        );
    if let Some(name) = apkinfo_core::info::android_version_name(v) {
        row = row.child(Element::badge_intent(name, Intent::Primary));
    }
    if let Some(c) = codename {
        row = row.child(
            Element::label(format!("[{}]", c))
                .font_size(12.0)
                .fg_role(Role::TextSubtle),
        );
    }
    field_row_custom(state, title, row, None)
}

/// 全部候选图标的缩略图条：点一格就把概览头的大图标切过去。
fn icon_strip(state: &AppState, doc: &Rc<Doc>) -> Element {
    let st = state.clone();
    let d = doc.clone();
    let loading = Element::label(t!("icon.loading"))
        .font_size(12.0)
        .fg_role(Role::TextSubtle)
        .visible_when({
            let thumbs = doc.icon_thumbs;
            let candidates = doc.icon_candidates;
            move || thumbs.with(|v| v.is_empty()) && candidates.with(|c| !c.is_empty())
        });
    let none = Element::label(t!("field.none"))
        .font_size(13.0)
        .fg_role(Role::TextSubtle)
        .visible_when({
            let candidates = doc.icon_candidates;
            move || candidates.with(|c| c.is_empty())
        });
    let strip = Element::host_signal(
        doc.icon_thumbs,
        move |list: Rc<Vec<crate::state::IconThumb>>| {
            let mut cells = Vec::new();
            for t in list.iter() {
                let st = st.clone();
                let d = d.clone();
                let index = t.index;
                let selected = d.icon_index;
                let frame = Element::stack()
                    .size(56, 56)
                    .corner(10.0)
                    .child(
                        Element::image_rgba(t.image.width, t.image.height, &t.image.data)
                            .fit(Fit::Contain)
                            .size(48, 48)
                            .corner(8.0)
                            .align_xy(Align::Center, Align::Center),
                    )
                    .child(
                        Element::col()
                            .size(56, 56)
                            .corner(10.0)
                            .border_role(Role::Accent, 2)
                            .visible_when(move || selected.get() == index),
                    );
                cells.push(
                    Element::col()
                        .cross(Align::Center)
                        .spacing(2)
                        .padding(2)
                        .corner(8.0)
                        .clickable()
                        .tooltip(t.label.clone())
                        .on_click(move |_| crate::actions::select_icon(&st, &d, index))
                        .child(frame)
                        .child(
                            Element::label(t.label.clone())
                                .font_size(10.5)
                                .fg_role(Role::TextMuted),
                        ),
                );
            }
            // 一行放不下就折行：grid 按列数等分宽度，7 列在默认窗宽下正好一格 ~64px。
            Element::grid(7, 6, cells).width_match()
        },
    )
    .width_match();
    Element::col()
        .width_match()
        .child(none)
        .child(loading)
        .child(strip)
}

fn permissions(doc: &Doc, info: Rc<ApkInfo>) -> Element {
    let all = info.meta.permissions.clone();
    let filter = doc.permission_filter;
    let all_for_list = all.clone();
    let filtered: Signal<Vec<String>> = Signal::map(filter, move |f| {
        let f = f.trim().to_lowercase();
        all_for_list
            .iter()
            .filter(|p| f.is_empty() || p.to_lowercase().contains(&f))
            .cloned()
            .collect()
    });

    let header = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .child(Element::text_input(filter, t!("action.filter")).width(260))
        .child(
            Element::label(Signal::map(filtered, |v| {
                tr!("field.count", count = v.len() as i64)
            }))
            .font_size(12.5)
            .fg_role(Role::TextMuted)
            .weight(1.0),
        )
        .child({
            let joined = join_lines(&all);
            Element::button(t!("action.copy_all"))
                .small()
                .outline()
                .neutral()
                .enabled(!all.is_empty())
                .on_click(move |ctx| {
                    ctx.clipboard_set(&joined);
                    ctx.toast_ok(tr!("action.copied", content = tr!("field.permissions")));
                })
        });

    let list = Element::list_signal(
        filtered,
        |p: &String| p.clone(),
        |p: String| {
            Element::row()
                .width_match()
                .cross(Align::Center)
                .padding_xy(12, 6)
                .child(
                    Element::label(p.clone())
                        .font_size(13.0)
                        .font_family(super::mono_family())
                        .fg_role(Role::Text)
                        .weight(1.0),
                )
                .child(copy_button(p))
        },
    )
    .fill();

    let empty = Element::label(t!("field.none"))
        .fg_role(Role::TextSubtle)
        .padding(12)
        .visible(all.is_empty());

    Element::col()
        .fill()
        .padding_edges(Insets::new(
            super::PAGE_PAD,
            14,
            super::PAGE_PAD,
            super::PAGE_PAD,
        ))
        .spacing(10)
        .child(header)
        .child(
            Element::col()
                .fill()
                .weight(1.0)
                .bg_role(Role::Surface)
                .corner(10.0)
                .border_role(Role::Border, 1)
                .child(empty)
                .child(list.weight(1.0)),
        )
}

fn splits(state: &AppState, info: Rc<ApkInfo>) -> Element {
    let width = state.title_width.get();
    // 表头 + 行：用普通行而非 table_custom——表格要限高容器，而这页整体是滚动的。
    let head = |a: String, b: String, c: String| {
        Element::row()
            .width_match()
            .cross(Align::Center)
            .spacing(12)
            .padding_xy(8, 6)
            .bg_role(Role::SurfaceAlt)
            .corner(6.0)
            .child(
                Element::label(a)
                    .weight(1.0)
                    .font_size(12.5)
                    .font_weight(600)
                    .fg_role(Role::TextMuted),
            )
            .child(
                Element::label(b)
                    .width(width)
                    .font_size(12.5)
                    .font_weight(600)
                    .fg_role(Role::TextMuted),
            )
            .child(
                Element::label(c)
                    .width(90)
                    .font_size(12.5)
                    .font_weight(600)
                    .fg_role(Role::TextMuted),
            )
    };
    let mut split_rows = Element::col().width_match().spacing(2).child(head(
        tr!("field.split_apks"),
        tr!("install.split_options"),
        tr!("field.size"),
    ));
    for s in &info.splits {
        let kind = match s.kind {
            SplitType::Base => tr!("install.split_type.base"),
            SplitType::Language => tr!("install.split_type.language"),
            SplitType::Abi => tr!("install.split_type.abi"),
            SplitType::Density => tr!("install.split_type.density"),
            SplitType::Unknown => tr!("install.split_type.unknown"),
        };
        split_rows = split_rows.child(
            Element::row()
                .width_match()
                .cross(Align::Center)
                .spacing(12)
                .padding_xy(8, 5)
                .child(
                    Element::label(s.path.clone())
                        .weight(1.0)
                        .font_size(13.0)
                        .fg_role(Role::Text),
                )
                .child(
                    Element::label(kind)
                        .width(width)
                        .font_size(13.0)
                        .fg_role(Role::TextMuted),
                )
                .child(
                    Element::label(format_file_size(s.size))
                        .width(90)
                        .font_size(13.0)
                        .fg_role(Role::TextMuted),
                ),
        );
    }

    let joined = join_lines(info.splits.iter().map(|s| s.path.as_str()));
    let split_section = section_with_action(
        t!("field.split_apks"),
        Element::button(t!("action.copy_all"))
            .small()
            .outline()
            .neutral()
            .enabled(!info.splits.is_empty())
            .on_click(move |ctx| {
                ctx.clipboard_set(&joined);
                ctx.toast_ok(tr!("action.copied", content = tr!("field.split_apks")));
            }),
        if info.splits.is_empty() {
            Element::label(t!("field.none")).fg_role(Role::TextSubtle)
        } else {
            split_rows
        },
    );

    let mut obb_rows = Element::col().width_match().spacing(2).child(head(
        tr!("field.obb_files"),
        String::new(),
        tr!("field.size"),
    ));
    for o in &info.obbs {
        obb_rows = obb_rows.child(
            Element::row()
                .width_match()
                .cross(Align::Center)
                .spacing(12)
                .padding_xy(8, 5)
                .child(
                    Element::label(o.path.clone())
                        .weight(1.0)
                        .font_size(13.0)
                        .fg_role(Role::Text),
                )
                .child(Element::label("").width(width))
                .child(
                    Element::label(format_file_size(o.size))
                        .width(90)
                        .font_size(13.0)
                        .fg_role(Role::TextMuted),
                ),
        );
    }
    let obb_body = if info.obbs.is_empty() {
        Element::label(t!("field.none")).fg_role(Role::TextSubtle)
    } else {
        obb_rows
    };
    let obb_section = section(tr!("field.obb_files"), obb_body);

    page(
        Element::col()
            .width_match()
            .spacing(14)
            .child(split_section)
            .child(obb_section),
    )
}

fn signature(state: &AppState, doc: &Doc, info: Rc<ApkInfo>) -> Element {
    let sig_state = doc.sig_state;
    let sig_error = doc.sig_error;
    let st = state.clone();

    let status = Element::col()
        .width_match()
        .child(
            Element::label(t!("field.computing"))
                .fg_role(Role::TextMuted)
                .visible_when(move || sig_state.get() == SigState::Computing),
        )
        .child(
            Element::label(t!("signature.disabled"))
                .fg_role(Role::TextMuted)
                .visible_when(move || sig_state.get() == SigState::Disabled),
        )
        .child(
            Element::label(t!("signature.not_applicable"))
                .fg_role(Role::TextMuted)
                .visible_when(move || sig_state.get() == SigState::NotApplicable),
        )
        .child(
            Element::label(Signal::map(sig_error, |e| {
                tr!("signature.failed", error = e.clone())
            }))
            .fg_role(Role::Danger)
            .visible_when(move || sig_state.get() == SigState::Failed),
        );

    let cards = Element::host_signal(
        doc.signature,
        move |sig: Rc<apkinfo_core::info::SignatureInfo>| {
            let mut col = Element::col().width_match().spacing(14);
            let mut schemes = Element::row().cross(Align::Center).spacing(6);
            let list = sig.schemes();
            if list.is_empty() {
                schemes = schemes
                    .child(Element::label(t!("signature.no_signature")).fg_role(Role::TextMuted));
            }
            for s in list {
                schemes = schemes.child(Element::badge(s));
            }
            col = col.child(section(
                tr!("signature.schemes"),
                Element::col()
                    .width_match()
                    .spacing(8)
                    .child(schemes)
                    .child(
                        Element::label(t!("signature.note"))
                            .font_size(12.0)
                            .fg_role(Role::TextSubtle),
                    ),
            ));
            for (i, signer) in sig.signers.iter().enumerate() {
                let mut body = Element::col()
                    .width_match()
                    .child(field_row(
                        &st,
                        t!("signature.subject"),
                        signer.subject.clone(),
                    ))
                    .child(field_row(
                        &st,
                        t!("signature.issuer"),
                        signer.issuer.clone(),
                    ))
                    .child(field_row(
                        &st,
                        t!("signature.serial"),
                        signer.serial_hex.clone(),
                    ))
                    .child(field_row_with(
                        &st,
                        t!("signature.valid_from"),
                        signer.not_before.clone(),
                        false,
                    ))
                    .child(field_row_with(
                        &st,
                        t!("signature.valid_to"),
                        signer.not_after.clone(),
                        false,
                    ))
                    .child(field_row(
                        &st,
                        t!("signature.sha256"),
                        signer.sha256.clone(),
                    ))
                    .child(field_row(&st, t!("signature.sha1"), signer.sha1.clone()))
                    .child(field_row(&st, t!("signature.md5"), signer.md5.clone()))
                    .child(field_row_with(
                        &st,
                        t!("signature.key_algorithm"),
                        signer.key_algorithm.clone(),
                        false,
                    ));
                if let Some(bits) = signer.key_size_bits {
                    body = body.child(field_row_with(
                        &st,
                        t!("signature.key_size"),
                        tr!("signature.bits", bits = bits as i64),
                        false,
                    ));
                }
                body = body.child(field_row_with(
                    &st,
                    t!("signature.sig_algorithm"),
                    signer.signature_algorithm.clone(),
                    false,
                ));
                col = col.child(section(tr!("signature.signer", n = (i + 1) as i64), body));
            }
            for w in &sig.warnings {
                col = col.child(
                    Element::label(w.clone())
                        .font_size(12.0)
                        .fg_role(Role::Warning),
                );
            }
            col
        },
    );

    let _ = info;
    page(
        Element::col()
            .width_match()
            .spacing(14)
            .child(status)
            .child(cards),
    )
}

fn hashes(state: &AppState, doc: &Doc) -> Element {
    let hs = doc.hash_state;
    let err = doc.hash_error;
    let md5 = doc.md5;
    let sha1 = doc.sha1;
    let st = state.clone();

    let row = |title: &'static str, sig: Signal<String>| {
        let width = st.title_width.get();
        let shown = Signal::map(sig, move |v| {
            if v.is_empty() {
                match hs.get() {
                    HashState::Computing => tr!("field.computing"),
                    HashState::Disabled => tr!("hash.disabled"),
                    _ => String::new(),
                }
            } else {
                v.clone()
            }
        });
        Element::row()
            .width_match()
            .cross(Align::Center)
            .spacing(12)
            .padding_xy(0, 6)
            .child(
                Element::label(t!(title))
                    .width(width)
                    .font_size(13.0)
                    .fg_role(Role::TextMuted),
            )
            .child(
                Element::label(shown)
                    .weight(1.0)
                    .font_size(13.0)
                    .font_family(super::mono_family())
                    .fg_role(Role::Text),
            )
            .child(
                crate::icons::button("copy", 24, tr!("action.copy"))
                    .enabled_when(move || sig.with(|v| !v.is_empty()))
                    .on_click(move |ctx| {
                        let v = sig.get();
                        ctx.clipboard_set(&v);
                        ctx.toast_ok(tr!("action.copied", content = v));
                    }),
            )
    };

    page(
        Element::col().width_match().spacing(14).child(section(
            tr!("tab.hashes"),
            Element::col()
                .width_match()
                .child(row("field.md5", md5))
                .child(row("field.sha1", sha1))
                .child(
                    Element::label(Signal::map(err, |e| tr!("hash.failed", error = e.clone())))
                        .fg_role(Role::Danger)
                        .visible_when(move || hs.get() == HashState::Failed),
                ),
        )),
    )
}

fn text_info(state: &AppState, info: Rc<ApkInfo>) -> Element {
    let text = signal(info.text_info.clone());
    let copy_text = info.text_info.clone();
    let _ = state;
    Element::col()
        .fill()
        .padding_edges(Insets::new(
            super::PAGE_PAD,
            14,
            super::PAGE_PAD,
            super::PAGE_PAD,
        ))
        .spacing(10)
        .child(
            Element::row()
                .width_match()
                .cross(Align::Center)
                .child(
                    Element::label(t!("tab.text_info"))
                        .font_size(14.0)
                        .font_weight(600)
                        .fg_role(Role::Text)
                        .weight(1.0),
                )
                .child(
                    Element::button(t!("action.copy_all"))
                        .small()
                        .outline()
                        .neutral()
                        .on_click(move |ctx| {
                            ctx.clipboard_set(&copy_text);
                            ctx.toast_ok(tr!("action.copied", content = tr!("tab.text_info")));
                        }),
                ),
        )
        .child(mono_block(text, 200).weight(1.0))
}
