//! 标签页：概览 / 权限 / 库 / 分包 / 签名 / 哈希 / 文本信息。
//!
//! 每页都是纯函数 `(state, doc, info) -> Element`，在 `ApkInfo` 变化时整体重建。

use std::rc::Rc;

use apkinfo_core::android::{self, PermissionRisk, Protection, ProtectionBase};
use apkinfo_core::info::{
    format_file_size, format_file_size_full, sdk_version_text, ApkInfo, SplitType,
};
use windui::prelude::*;

use super::{
    copy_button, field_row, field_row_copy, field_row_custom, join_lines, mono_block, page,
    section, section_with_action, selectable_signal,
};
use crate::android_text;
use crate::state::{AppState, Doc, HashState, SigState};
use crate::theme::{CARD_BLUE, CARD_GREEN, CARD_ORANGE, CARD_PURPLE};

/// 标签页下标（归档包在「库」之后多一页「分包 / OBB」）。
struct TabIndex {
    splits: Option<usize>,
    signature: usize,
}

fn tab_index(info: &ApkInfo) -> TabIndex {
    if info.is_archive() {
        TabIndex {
            splits: Some(3),
            signature: 4,
        }
    } else {
        TabIndex {
            splits: None,
            signature: 3,
        }
    }
}

/// 把标签页组装好。
pub fn build_tabs(state: &AppState, doc: Rc<Doc>, info: Rc<ApkInfo>) -> Element {
    let perm_count = info.meta.permissions.len();
    let split_count = info.splits.len() + info.obbs.len();
    let perm_title = if perm_count > 0 {
        t!("tab.permissions_count", count = perm_count as i64)
    } else {
        t!("tab.permissions")
    };
    let split_title = if split_count > 0 {
        t!("tab.splits_count", count = split_count as i64)
    } else {
        t!("tab.splits")
    };
    let icon = crate::icons::gray;
    let mut pages = vec![
        (
            t!("tab.overview"),
            icon("info"),
            overview(state, &doc, info.clone()),
        ),
        (
            perm_title,
            icon("key"),
            permissions(state, &doc, info.clone()),
        ),
        (
            libs_title(&doc),
            icon("blocks"),
            super::libs::libs_page(state, &doc, info.clone()),
        ),
    ];
    if info.is_archive() {
        pages.push((split_title, icon("layers"), splits(state, info.clone())));
    }
    pages.push((
        t!("tab.signature"),
        icon("shield"),
        signature(state, &doc, info.clone()),
    ));
    pages.push((t!("tab.hashes"), icon("hash"), hashes(state, &doc)));
    pages.push((
        t!("tab.text_info"),
        icon("file_text"),
        text_info(state, info.clone()),
    ));

    // 标签数因归档包而变，选中索引越界时回到概览。
    let count = pages.len();
    if doc.tab.get() >= count {
        doc.tab.set(0);
    }
    // 标签条与各页内容同在一张白色卡片里（概览头一张、这里一张），页面本身不再铺底色，
    // 标签与内容之间不会露出一道色差。
    Element::col()
        .fill()
        .padding_edges(Insets::new(
            0,
            super::CARD_GAP,
            super::PAGE_PAD,
            super::PAGE_PAD,
        ))
        .child(
            // 标签条与内容之间默认空 16px，再加页面内边距就显得空；这里收掉，只留页面那一圈。
            Element::tabs_icons(doc.tab, pages)
                .spacing(2)
                .fill()
                .bg_role(Role::Surface)
                .corner(super::CARD_CORNER)
                .border_role(Role::Border, 1),
        )
}

fn libs_title(doc: &Doc) -> windui::i18n::Message {
    match doc.libs.with(|v| v.first().map(|r| r.sdk_count())) {
        Some(n) if n > 0 => t!("tab.libs_count", count = n as i64),
        _ => t!("tab.libs"),
    }
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
    // 语言列表动辄几十上百项：只显示数量，点开对话框看逐条名称，复制按钮给完整列表。
    let languages = if m.locales.is_empty() {
        Element::label(t!("field.none"))
            .font_size(13.0)
            .fg_role(Role::TextSubtle)
            .height(super::VALUE_H)
    } else {
        let dialog = state.locales_dialog.clone();
        let codes = m.locales.clone();
        Element::link(t!("field.language_count", count = m.locales.len() as i64))
            .font_size(13.0)
            .tooltip(t!("locale.hint"))
            .on_click(move |_| dialog.open(codes.clone()))
    };
    basic = basic.child(field_row_custom(
        state,
        t!("field.languages"),
        super::plain_value(languages),
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
    let basic_card = section("box", CARD_BLUE, t!("field.basic_info"), basic);

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
            t!("field.cert_info"),
            Some(link),
            certificate_summary(state, doc),
        )
    };

    // ---- 文件信息 ----
    let path_text = info.path.display().to_string();
    let open_dir = {
        let p = info.path.clone();
        crate::icons::button("folder", 24, t!("action.open_dir")).on_click(move |ctx| {
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
        file = file.child(field_row_custom(
            state,
            t!("field.split_count"),
            plain_label(t!("field.items", count = info.splits.len() as i64)),
            None,
        ));
        file = file.child(field_row_custom(
            state,
            t!("field.obb_count"),
            plain_label(t!("field.items", count = info.obbs.len() as i64)),
            None,
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
        t!("field.file_info"),
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
        col = col.child(features_card(m));
    }
    page(col)
}

/// 硬件 / 软件特性卡片：名称 | 说明 | 必需 / 可选。
fn features_card(m: &apkinfo_core::model::ApkMeta) -> Element {
    const NAME_W: i32 = 320;
    let head_label = |text: windui::i18n::Message| {
        Element::label(text)
            .font_size(12.5)
            .font_weight(600)
            .fg_role(Role::TextMuted)
    };
    let mut table = Element::col().width_match().child(
        Element::row()
            .width_match()
            .spacing(12)
            .padding_edges(Insets::new(super::VALUE_PAD_X, 6, 8, 6))
            .bg_role(Role::SurfaceAlt)
            .corner(6.0)
            .child(head_label(t!("feature_table.col_name")).width(NAME_W - super::VALUE_PAD_X))
            .child(head_label(t!("feature_table.col_desc")).weight(1.0))
            .child(head_label(t!("feature_table.col_required"))),
    );
    let rows = m
        .features
        .iter()
        .map(|f| (f, true))
        .chain(m.features_not_required.iter().map(|f| (f, false)));
    for (name, required) in rows {
        // 未收录的（厂商 / 第三方特性）说明栏留空，名字本身就是全部信息
        let desc = match android_text::feature_desc(name) {
            Some(d) => Element::label(d).font_size(13.0),
            None => Element::leaf(),
        };
        let badge = if required {
            Element::badge_intent(t!("feature_table.required"), windui::theme::Intent::Primary)
                .tooltip(t!("feature_table.required_tip"))
        } else {
            Element::badge_intent(t!("feature_table.optional"), windui::theme::Intent::Neutral)
                .tooltip(t!("feature_table.optional_tip"))
        };
        table = table.child(
            Element::row()
                .width_match()
                .cross(Align::Center)
                .spacing(12)
                .padding_edges(Insets::new(0, 3, 8, 3))
                .child(
                    super::selectable(name.clone())
                        .font_family(super::mono_family())
                        .font_size(12.5)
                        .width(NAME_W),
                )
                .child(desc.weight(1.0))
                .child(badge),
        );
    }
    let all: Vec<&String> = m.features.iter().chain(&m.features_not_required).collect();
    section_with_action(
        "cpu",
        CARD_GREEN,
        t!("field.features"),
        Some(copy_button(join_lines(all))),
        table,
    )
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
            Element::label(t!("signature.failed", error = sig_error))
                .font_size(13.0)
                .fg_role(Role::Danger)
                .padding_xy(0, 4)
                .visible_when(move || sig_state.get() == SigState::Failed),
        )
        .child(cards)
}

/// 字段行里不可选中的纯文本值（数量、位数这类跟随语言的短句）。
fn plain_label(text: windui::i18n::Message) -> Element {
    super::plain_value(
        Element::label(text)
            .font_size(13.0)
            .fg_role(Role::Text)
            .height(super::VALUE_H),
    )
}

/// 权限表的一行：清单里的原始条目 + 拆出来的名字、maxSdk 与框架保护级别。
#[derive(Clone, PartialEq)]
struct PermRow {
    entry: String,
    name: String,
    max_sdk: Option<u32>,
    level: Option<Protection>,
    risk: PermissionRisk,
}

impl PermRow {
    fn new(entry: &str) -> Self {
        let (name, max_sdk) = android::split_permission_entry(entry);
        Self {
            entry: entry.to_string(),
            name: name.to_string(),
            max_sdk,
            level: android::permission_protection(name),
            risk: android::permission_risk(name),
        }
    }

    /// 过滤：名字、当前界面语言下的标题与说明，任一包含关键字即命中。
    fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() || self.entry.to_lowercase().contains(needle) {
            return true;
        }
        let cat = windui::i18n::current();
        [
            android_text::permission_label(&self.name),
            android_text::permission_desc(&self.name),
            Some(risk_label(self.risk)),
        ]
        .into_iter()
        .flatten()
        .any(|m| cat.format(&m).to_lowercase().contains(needle))
    }
}

const PERM_NAME_W: i32 = 300;
const PERM_LEVEL_W: i32 = 76;
/// 行尾复制按钮占的宽度，表头要空出同样的位置才对得齐。
const PERM_COPY_W: i32 = 24;

fn risk_label(risk: PermissionRisk) -> windui::i18n::Message {
    match risk {
        PermissionRisk::High => t!("perm.risk_high"),
        PermissionRisk::Sensitive => t!("perm.risk_sensitive"),
        PermissionRisk::Normal => t!("perm.risk_normal"),
        PermissionRisk::System => t!("perm.risk_system"),
        PermissionRisk::Unknown => t!("perm.risk_custom"),
    }
}

fn risk_badge(ui_lang: Signal<String>, risk: PermissionRisk, level: Option<Protection>) -> Element {
    use windui::theme::Intent;
    let (tip, intent) = match risk {
        PermissionRisk::High => (t!("perm.risk_tip_high"), Intent::Danger),
        PermissionRisk::Sensitive => (t!("perm.risk_tip_sensitive"), Intent::Warning),
        PermissionRisk::Normal => (t!("perm.risk_tip_normal"), Intent::Success),
        PermissionRisk::System => (t!("perm.risk_tip_system"), Intent::Neutral),
        PermissionRisk::Unknown => (t!("perm.risk_tip_custom"), Intent::Neutral),
    };
    let badge = Element::badge_intent(risk_label(risk), intent);
    let Some(level) = level else {
        return badge.tooltip(tip);
    };
    // 提示里补上 Android 自己的保护级别与附加标志（原名 appop、privileged…，不翻译）。
    // 几条消息要拼起来，从 ui_lang 派生，换语言才跟随（行不会因换语言重建）
    let level_name = match level.base() {
        ProtectionBase::Normal => t!("perm.level_normal"),
        ProtectionBase::Dangerous => t!("perm.level_dangerous"),
        ProtectionBase::Signature => t!("perm.level_signature"),
        ProtectionBase::SignatureOrSystem => t!("perm.level_signature_or_system"),
        ProtectionBase::Internal => t!("perm.level_internal"),
    };
    let flags = level.flags();
    let flags = (!flags.is_empty()).then(|| t!("perm.flags", flags = flags.join(" | ")));
    badge.tooltip(ui_lang.map(move |_| {
        let cat = windui::i18n::current();
        let mut lines = vec![
            cat.format(&tip),
            cat.format(&t!("perm.level_line", level = cat.format(&level_name))),
        ];
        lines.extend(flags.iter().map(|f| cat.format(f)));
        lines.join("\n")
    }))
}

fn permission_row(ui_lang: Signal<String>, row: PermRow) -> Element {
    // android.permission.* 只显示常量名，其余（应用 / 厂商自定义）显示全名
    let shown = row
        .name
        .strip_prefix("android.permission.")
        .unwrap_or(&row.name)
        .to_string();
    let mut name_col = Element::col().width(PERM_NAME_W).spacing(2).child(
        super::selectable(shown)
            .font_family(super::mono_family())
            .font_size(12.5),
    );
    if let Some(max) = row.max_sdk {
        name_col = name_col.child(super::plain_value(
            Element::label(t!("perm.max_sdk", sdk = max as i64))
                .font_size(11.5)
                .fg_role(Role::TextSubtle)
                .tooltip(t!(
                    "perm.max_sdk_tip",
                    sdk = max as i64,
                    version = android_text::version_name(ui_lang, max as i64)
                )),
        ));
    }

    let mut desc_col = Element::col().weight(1.0).spacing(2);
    match android_text::permission_label(&row.name) {
        Some(label) => desc_col = desc_col.child(Element::label(label).font_size(13.0)),
        None if row.level.is_none() => {
            desc_col = desc_col.child(
                Element::label(t!("perm.custom_desc"))
                    .font_size(13.0)
                    .fg_role(Role::TextSubtle),
            )
        }
        None => {}
    }
    match android_text::permission_desc(&row.name) {
        Some(desc) => {
            desc_col = desc_col.child(
                Element::label(desc)
                    .font_size(12.0)
                    .fg_role(Role::TextMuted)
                    .width_match(),
            )
        }
        None if row.level.is_some() => {
            desc_col = desc_col.child(
                Element::label(t!("perm.no_desc"))
                    .font_size(12.0)
                    .fg_role(Role::TextSubtle),
            )
        }
        None => {}
    }

    Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(12)
        .padding_edges(Insets::new(2, 7, 12, 7))
        .child(name_col)
        .child(desc_col)
        .child(
            Element::row()
                .width(PERM_LEVEL_W)
                .child(risk_badge(ui_lang, row.risk, row.level)),
        )
        .child(copy_button(row.name))
}

fn permissions(state: &AppState, doc: &Doc, info: Rc<ApkInfo>) -> Element {
    let ui_lang = state.ui_lang;
    let all = info.meta.permissions.clone();
    // 同一权限常在 uses-permission 与 uses-permission-sdk-23 里各写一次，只留一行
    let mut seen = std::collections::HashSet::new();
    let mut rows: Vec<PermRow> = all
        .iter()
        .filter(|e| seen.insert(e.as_str()))
        .map(|e| PermRow::new(e))
        .collect();
    // 风险高的排前面（同级保持清单顺序）
    rows.sort_by_key(|r| r.risk);
    let count_of = |risk| rows.iter().filter(|r| r.risk == risk).count() as i64;
    let (high, sensitive) = (
        count_of(PermissionRisk::High),
        count_of(PermissionRisk::Sensitive),
    );
    let rows = Rc::new(rows);
    let filter = doc.permission_filter;
    let filtered: Signal<Vec<PermRow>> = Signal::map(filter, move |f| {
        let f = f.trim().to_lowercase();
        rows.iter().filter(|r| r.matches(&f)).cloned().collect()
    });

    let header = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .child(Element::text_input(filter, t!("action.filter")).width(260))
        .child(
            Element::label(t!(
                "field.count",
                count = Signal::map(filtered, |v| v.len() as i64)
            ))
            .font_size(12.5)
            .fg_role(Role::TextMuted),
        )
        .child(
            Element::badge_intent(
                t!("perm.high_count", count = high),
                windui::theme::Intent::Danger,
            )
            .tooltip(t!("perm.risk_tip_high"))
            .visible(high > 0),
        )
        .child(
            Element::badge_intent(
                t!("perm.sensitive_count", count = sensitive),
                windui::theme::Intent::Warning,
            )
            .tooltip(t!("perm.risk_tip_sensitive"))
            .visible(sensitive > 0),
        )
        .child(Element::flex_spacer())
        .child({
            let joined = join_lines(&all);
            Element::button(t!("action.copy_all"))
                .small()
                .outline_soft()
                .neutral()
                .enabled(!all.is_empty())
                .on_click(move |ctx| {
                    ctx.clipboard_set(&joined);
                    ctx.toast_ok(tr!("action.copied", content = tr!("field.permissions")));
                })
        });

    let head_label = |text: windui::i18n::Message| {
        Element::label(text)
            .font_size(12.5)
            .font_weight(600)
            .fg_role(Role::TextMuted)
    };
    // 左边距与行内无框输入框的文字对齐
    let table_head = Element::row()
        .width_match()
        .spacing(12)
        .padding_edges(Insets::new(2 + super::VALUE_PAD_X, 7, 12, 7))
        .bg_role(Role::SurfaceAlt)
        .corner(6.0)
        .child(head_label(t!("perm.col_name")).width(PERM_NAME_W - super::VALUE_PAD_X))
        .child(head_label(t!("perm.col_desc")).weight(1.0))
        .child(head_label(t!("perm.col_level")).width(PERM_LEVEL_W))
        .child(Element::leaf().width(PERM_COPY_W))
        .visible(!all.is_empty());

    let list = Element::list_signal(
        filtered,
        |r: &PermRow| r.entry.clone(),
        move |r| permission_row(ui_lang, r),
    )
    .fill();

    let empty = Element::label(t!("field.none"))
        .fg_role(Role::TextSubtle)
        .padding(12)
        .visible(all.is_empty());

    Element::col()
        .fill()
        .padding_edges(Insets::new(
            super::TAB_PAD,
            super::TAB_PAD,
            super::TAB_PAD,
            super::TAB_PAD,
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
                .padding(4)
                .child(table_head)
                .child(empty)
                .child(list.weight(1.0)),
        )
}

fn splits(state: &AppState, info: Rc<ApkInfo>) -> Element {
    let width = state.title_width.get();
    // 表头 + 行：用普通行而非 table_custom——表格要限高容器，而这页整体是滚动的。
    let head = |a: TextContent, b: TextContent, c: TextContent| {
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
        t!("field.split_apks").into(),
        t!("install.split_options").into(),
        t!("field.size").into(),
    ));
    for s in &info.splits {
        let kind = match s.kind {
            SplitType::Base => t!("install.split_type.base"),
            SplitType::Language => t!("install.split_type.language"),
            SplitType::Abi => t!("install.split_type.abi"),
            SplitType::Density => t!("install.split_type.density"),
            SplitType::Unknown => t!("install.split_type.unknown"),
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
                .outline_soft()
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
        t!("field.obb_files").into(),
        "".into(),
        t!("field.size").into(),
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
    let obb_section = section("box", CARD_ORANGE, t!("field.obb_files"), obb_body);

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
            Element::label(t!("signature.failed", error = sig_error))
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
                t!("signature.schemes"),
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
                    body = body.child(field_row_custom(
                        &st,
                        t!("signature.key_size"),
                        plain_label(t!("signature.bits", bits = bits as i64)),
                        None,
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
                    t!("signature.signer", n = (i + 1) as i64),
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
        // 值为空时按状态显示「计算中 / 已关闭」，占位文案走 t! 跟随语言。
        let placeholder = |key: &'static str, want: HashState| {
            plain_label(t!(key))
                .visible_when(move || sig.with(|v| v.is_empty()) && hs.get() == want)
        };
        field_row_custom(
            &st,
            t!(title),
            Element::row()
                .cross(Align::Center)
                .spacing(2)
                .child(
                    Element::col()
                        .weight(1.0)
                        .child(
                            selectable_signal(sig)
                                .font_family(super::mono_family())
                                .visible_when(move || sig.with(|v| !v.is_empty())),
                        )
                        .child(placeholder("field.computing", HashState::Computing))
                        .child(placeholder("hash.disabled", HashState::Disabled)),
                )
                .child(
                    crate::icons::button("copy", 24, t!("action.copy"))
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
            t!("tab.hashes"),
            Element::col()
                .width_match()
                .child(row("field.md5", md5))
                .child(row("field.sha1", sha1))
                .child(
                    Element::label(t!("hash.failed", error = err))
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
            super::TAB_PAD,
            super::TAB_PAD,
            super::TAB_PAD,
            super::TAB_PAD,
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
                        .outline_soft()
                        .neutral()
                        .on_click(move |ctx| {
                            ctx.clipboard_set(&copy_text);
                            ctx.toast_ok(tr!("action.copied", content = tr!("tab.text_info")));
                        }),
                ),
        )
        .child(mono_block(text, 200).weight(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::i18n::{Initial, Locales};

    #[test]
    fn permission_filter_matches_localized_label() {
        let b = Locales::builder().embed(include_str!("../../i18n/zh-CN.toml"));
        windui::i18n::install(
            android_text::embed_layers(b)
                .initial(Initial::Fixed("zh-CN".into()))
                .build(),
        );
        let row = PermRow::new("android.permission.ACCESS_FINE_LOCATION (maxSdk 32)");
        assert_eq!(row.name, "android.permission.ACCESS_FINE_LOCATION");
        assert_eq!(row.max_sdk, Some(32));
        assert!(row.matches("fine_location"));
        assert!(row.matches("位置"), "按中文标题 / 说明也能筛到");
        assert!(!row.matches("相机"));
        assert!(row.matches("敏感"), "按风险级别文字也能筛到");
    }
}
