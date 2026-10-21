//! 标签页：概览 / 权限 / 分包 / 签名 / 哈希 / 文本信息。
//!
//! 每页都是纯函数 `(state, doc, info) -> Element`，在 `ApkInfo` 变化时整体重建。

use std::rc::Rc;

use apkinfo_core::info::{
    format_file_size, format_file_size_full, sdk_version_text, ApkInfo, SplitType,
};
use windui::prelude::*;

use super::{
    copy_button, field_row, field_row_copy, field_row_custom, join_lines, mono_block, page,
    section, section_with_action, selectable_signal,
};
use crate::state::{AppState, Doc, HashState, SigState};
use crate::theme::{CARD_BLUE, CARD_GREEN, CARD_ORANGE, CARD_PURPLE};

/// 标签页下标（归档包多一页「分包 / OBB」）。
struct TabIndex {
    splits: Option<usize>,
    signature: usize,
}

fn tab_index(info: &ApkInfo) -> TabIndex {
    if info.is_archive() {
        TabIndex {
            splits: Some(2),
            signature: 3,
        }
    } else {
        TabIndex {
            splits: None,
            signature: 2,
        }
    }
}

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
    // 标签条放在白色卡片里，各页内容自带 Bg 底色，于是只有标签条是白的。
    Element::col()
        .fill()
        .padding_edges(Insets::new(
            super::PAGE_PAD,
            super::CARD_GAP,
            super::PAGE_PAD,
            0,
        ))
        .child(
            Element::tabs_icons(doc.tab, pages)
                .fill()
                .bg_role(Role::Surface)
                .corner(super::CARD_CORNER)
                .border_role(Role::Border, 1),
        )
}

fn overview(state: &AppState, doc: &Rc<Doc>, info: Rc<ApkInfo>) -> Element {
    let m = &info.meta;
    let tabs = tab_index(&info);

    // ---- 基本信息 ----
    let mut basic = Element::col()
        .width_match()
        .child(field_row_copy(
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
        .child(field_row(
            state,
            t!("field.min_sdk"),
            sdk_version_text(m.min_sdk_version),
        ))
        .child(field_row(
            state,
            t!("field.target_sdk"),
            sdk_version_text(m.target_sdk_version),
        ));
    if m.compile_sdk_version.is_some() {
        let mut v = sdk_version_text(m.compile_sdk_version);
        if let Some(c) = &m.compile_sdk_version_codename {
            v.push_str(&format!(" [{}]", c));
        }
        basic = basic.child(field_row(state, t!("field.compile_sdk"), v));
    }
    basic = basic.child(field_row(
        state,
        t!("field.size"),
        format_file_size_full(info.size),
    ));
    // 架构：一串徽章。
    let abi_value = if m.native_codes.is_empty() {
        Element::label(t!("field.none"))
            .font_size(13.0)
            .fg_role(Role::TextSubtle)
    } else {
        let mut row = Element::row().cross(Align::Center).spacing(6);
        for abi in &m.native_codes {
            row = row.child(Element::badge_intent(abi.clone(), Intent::Primary));
        }
        row
    };
    basic = basic.child(field_row_custom(
        state,
        t!("field.abi"),
        super::plain_value(abi_value),
        Some(m.native_codes.join(" ")),
    ));
    basic = basic
        .child(field_row(
            state,
            t!("field.screen_size"),
            m.screen_sizes.join("  "),
        ))
        .child(field_row(
            state,
            t!("field.screen_density"),
            m.densities
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("  "),
        ));
    // 语言列表动辄几十上百项：只显示数量，复制按钮给完整列表。
    basic = basic.child(field_row_custom(
        state,
        t!("field.languages"),
        super::plain_value(
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
            })
            .height(super::VALUE_H),
        ),
        Some(m.locales.join("  ")),
    ));
    let activities: Vec<String> = m
        .launchable_activities
        .iter()
        .filter_map(|a| a.name.clone())
        .collect();
    basic = basic.child(field_row_copy(
        state,
        t!("field.launchable_activities"),
        activities.join("\n"),
    ));
    let basic_card = section("box", CARD_BLUE, tr!("field.basic_info"), basic);

    // ---- 证书信息（摘要，详情在签名页）----
    let cert_card = {
        let tab = doc.tab;
        let idx = tabs.signature;
        let link = Element::link(t!("field.view_details"))
            .font_size(12.5)
            .on_click(move |_| tab.set(idx));
        section_with_action(
            "shield",
            CARD_PURPLE,
            tr!("field.cert_info"),
            Some(link),
            certificate_summary(state, doc),
        )
    };

    // ---- 文件信息 ----
    let path_text = info.path.display().to_string();
    let open_dir = {
        let p = info.path.clone();
        crate::icons::button("folder", 24, tr!("action.open_dir")).on_click(move |ctx| {
            if let Err(e) = crate::actions::reveal_in_file_manager(&p) {
                ctx.toast_err(e);
            }
        })
    };
    let mut file = Element::col()
        .width_match()
        .child(field_row_custom(
            state,
            t!("field.file"),
            // 不设 width_match：在 weight 槽里它会按父宽度量、挤掉旁边的按钮。
            Element::row()
                .cross(Align::Center)
                .spacing(2)
                .child(super::selectable(path_text.clone()).weight(1.0))
                .child(open_dir),
            Some(path_text),
        ))
        .child(field_row(
            state,
            t!("field.size"),
            format_file_size_full(info.size),
        ))
        .child(field_row(
            state,
            t!("field.archive_type"),
            info.kind.display_name().to_string(),
        ));
    if info.is_archive() {
        if let Some(n) = &info.archive_app_name {
            file = file.child(field_row(state, t!("field.app_name"), n.clone()));
        }
        if let Some(b) = &info.base_apk_entry {
            file = file.child(field_row(state, t!("field.base_apk"), b.clone()));
        }
        if let Some(t) = info.total_size {
            file = file.child(field_row(
                state,
                t!("field.total_size"),
                format_file_size_full(t),
            ));
        }
        file = file.child(field_row(
            state,
            t!("field.split_count"),
            tr!("field.items", count = info.splits.len() as i64),
        ));
        file = file.child(field_row(
            state,
            t!("field.obb_count"),
            tr!("field.items", count = info.obbs.len() as i64),
        ));
        if !info.split_configs.is_empty() {
            file = file.child(field_row(
                state,
                t!("field.split_configs"),
                info.split_configs.join("  "),
            ));
        }
    }
    let file_action = tabs.splits.map(|idx| {
        let tab = doc.tab;
        Element::link(t!("field.view_details"))
            .font_size(12.5)
            .on_click(move |_| tab.set(idx))
    });
    let file_card = section_with_action(
        "file",
        CARD_ORANGE,
        tr!("field.file_info"),
        file_action,
        file,
    );

    let mut col = Element::col()
        .width_match()
        .spacing(super::CARD_GAP)
        .child(basic_card)
        .child(cert_card)
        .child(file_card);
    if !m.features.is_empty() || !m.features_not_required.is_empty() {
        let mut feat = Element::col().width_match();
        if !m.features.is_empty() {
            feat = feat.child(field_row_copy(
                state,
                t!("field.features"),
                m.features.join("\n"),
            ));
        }
        if !m.features_not_required.is_empty() {
            feat = feat.child(field_row_copy(
                state,
                t!("field.features_not_required"),
                m.features_not_required.join("\n"),
            ));
        }
        col = col.child(section("cpu", CARD_GREEN, tr!("field.features"), feat));
    }
    page(col)
}

/// 概览页的证书摘要：状态文案 + 第一个签名者的几项关键信息。
fn certificate_summary(state: &AppState, doc: &Rc<Doc>) -> Element {
    let sig_state = doc.sig_state;
    let sig_error = doc.sig_error;
    let st = state.clone();
    let status = |key: &'static str, which: SigState| {
        Element::label(t!(key))
            .font_size(13.0)
            .fg_role(Role::TextMuted)
            .padding_xy(0, 4)
            .visible_when(move || sig_state.get() == which)
    };
    let cards = Element::host_signal(
        doc.signature,
        move |sig: Rc<apkinfo_core::info::SignatureInfo>| {
            let mut col = Element::col().width_match();
            let mut schemes = Element::row().cross(Align::Center).spacing(6);
            let list = sig.schemes();
            if list.is_empty() {
                schemes = schemes.child(
                    Element::label(t!("signature.no_signature"))
                        .font_size(13.0)
                        .fg_role(Role::TextMuted),
                );
            }
            for s in list {
                schemes = schemes.child(Element::badge_intent(s, Intent::Success));
            }
            col = col.child(field_row_custom(
                &st,
                t!("signature.schemes"),
                super::plain_value(schemes),
                None,
            ));
            if let Some(signer) = sig.signers.first() {
                col = col
                    .child(field_row(
                        &st,
                        t!("signature.sig_algorithm"),
                        signer.signature_algorithm.clone(),
                    ))
                    .child(field_row_copy(
                        &st,
                        t!("signature.subject"),
                        signer.subject.clone(),
                    ))
                    .child(field_row_copy(
                        &st,
                        t!("signature.issuer"),
                        signer.issuer.clone(),
                    ))
                    .child(field_row(
                        &st,
                        t!("field.valid_period"),
                        format!("{}  ~  {}", signer.not_before, signer.not_after),
                    ))
                    .child(field_row_copy(
                        &st,
                        t!("signature.sha256"),
                        signer.sha256.clone(),
                    ));
            }
            col
        },
    )
    .width_match();
    Element::col()
        .width_match()
        .child(status("field.computing", SigState::Computing))
        .child(status("signature.disabled", SigState::Disabled))
        .child(status("signature.not_applicable", SigState::NotApplicable))
        .child(
            Element::label(Signal::map(sig_error, |e| {
                tr!("signature.failed", error = e.clone())
            }))
            .font_size(13.0)
            .fg_role(Role::Danger)
            .padding_xy(0, 4)
            .visible_when(move || sig_state.get() == SigState::Failed),
        )
        .child(cards)
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
                .padding_xy(12, 4)
                .child(
                    super::selectable(p.clone())
                        .font_family(super::mono_family())
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
        .bg_role(Role::Bg)
        .padding_edges(Insets::new(
            super::PAGE_PAD,
            super::CARD_GAP,
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
                .corner(super::CARD_CORNER)
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
                .padding_xy(8, 2)
                .child(super::selectable(s.path.clone()).weight(1.0))
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
        "layers",
        CARD_BLUE,
        t!("field.split_apks"),
        Some(
            Element::button(t!("action.copy_all"))
                .small()
                .outline()
                .neutral()
                .enabled(!info.splits.is_empty())
                .on_click(move |ctx| {
                    ctx.clipboard_set(&joined);
                    ctx.toast_ok(tr!("action.copied", content = tr!("field.split_apks")));
                }),
        ),
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
                .padding_xy(8, 2)
                .child(super::selectable(o.path.clone()).weight(1.0))
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
    let obb_section = section("box", CARD_ORANGE, tr!("field.obb_files"), obb_body);

    page(
        Element::col()
            .width_match()
            .spacing(super::CARD_GAP)
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
            let mut col = Element::col().width_match().spacing(super::CARD_GAP);
            let mut schemes = Element::row().cross(Align::Center).spacing(6);
            let list = sig.schemes();
            if list.is_empty() {
                schemes = schemes
                    .child(Element::label(t!("signature.no_signature")).fg_role(Role::TextMuted));
            }
            for s in list {
                schemes = schemes.child(Element::badge_intent(s, Intent::Success));
            }
            col = col.child(section(
                "shield",
                CARD_PURPLE,
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
                    .child(field_row_copy(
                        &st,
                        t!("signature.subject"),
                        signer.subject.clone(),
                    ))
                    .child(field_row_copy(
                        &st,
                        t!("signature.issuer"),
                        signer.issuer.clone(),
                    ))
                    .child(field_row_copy(
                        &st,
                        t!("signature.serial"),
                        signer.serial_hex.clone(),
                    ))
                    .child(field_row(
                        &st,
                        t!("signature.valid_from"),
                        signer.not_before.clone(),
                    ))
                    .child(field_row(
                        &st,
                        t!("signature.valid_to"),
                        signer.not_after.clone(),
                    ))
                    .child(field_row_copy(
                        &st,
                        t!("signature.sha256"),
                        signer.sha256.clone(),
                    ))
                    .child(field_row_copy(
                        &st,
                        t!("signature.sha1"),
                        signer.sha1.clone(),
                    ))
                    .child(field_row_copy(&st, t!("signature.md5"), signer.md5.clone()))
                    .child(field_row(
                        &st,
                        t!("signature.key_algorithm"),
                        signer.key_algorithm.clone(),
                    ));
                if let Some(bits) = signer.key_size_bits {
                    body = body.child(field_row(
                        &st,
                        t!("signature.key_size"),
                        tr!("signature.bits", bits = bits as i64),
                    ));
                }
                body = body.child(field_row(
                    &st,
                    t!("signature.sig_algorithm"),
                    signer.signature_algorithm.clone(),
                ));
                col = col.child(section(
                    "badge_check",
                    CARD_GREEN,
                    tr!("signature.signer", n = (i + 1) as i64),
                    body,
                ));
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
            .spacing(super::CARD_GAP)
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
        field_row_custom(
            &st,
            t!(title),
            Element::row()
                .cross(Align::Center)
                .spacing(2)
                .child(
                    selectable_signal(shown)
                        .font_family(super::mono_family())
                        .weight(1.0),
                )
                .child(
                    crate::icons::button("copy", 24, tr!("action.copy"))
                        .enabled_when(move || sig.with(|v| !v.is_empty()))
                        .on_click(move |ctx| {
                            let v = sig.get();
                            ctx.clipboard_set(&v);
                            ctx.toast_ok(tr!("action.copied", content = v));
                        }),
                ),
            None,
        )
    };

    page(
        Element::col().width_match().child(section(
            "hash",
            CARD_BLUE,
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
        .bg_role(Role::Bg)
        .padding_edges(Insets::new(
            super::PAGE_PAD,
            super::CARD_GAP,
            super::PAGE_PAD,
            super::PAGE_PAD,
        ))
        .spacing(10)
        .child(
            Element::row()
                .width_match()
                .cross(Align::Center)
                .child(super::card_icon("file_text", CARD_BLUE).margin_xy(0, 0))
                .child(
                    Element::label(t!("tab.text_info"))
                        .font_size(15.0)
                        .font_weight(600)
                        .fg_role(Role::Text)
                        .margin_xy(10, 0)
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
