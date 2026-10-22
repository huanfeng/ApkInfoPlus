//! 「库」标签页：LibChecker 规则识别出的原生库与 SDK 组件，点开某个 SDK 看详情。

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use apkinfo_core::info::{format_file_size, ApkInfo};
use apkinfo_core::libchecker::{LibReport, MatchSource, RuleDetail, RuleSet, SdkGroup};
use apkinfo_core::model::ComponentKind;
use windui::prelude::*;

use super::{page, section, selectable};
use crate::state::{AppState, Doc};
use crate::theme::{CARD_BLUE, CARD_GREEN, CARD_ORANGE, CARD_PURPLE};
use windui::core::EventCtx;

pub const RULES_HOME: &str = "https://github.com/LibChecker/LibChecker-Rules";
/// 组件分组里预览几个名字，其余收进「等 N 个」。
const PREVIEW_NAMES: usize = 3;

/// SDK 详情对话框的状态（应用级，一次只开一个）。
#[derive(Clone)]
pub struct DetailState {
    pub show: Signal<bool>,
    pub view: Signal<Vec<Rc<DetailView>>>,
}

impl DetailState {
    pub fn new() -> Self {
        Self {
            show: signal(false),
            view: signal(Vec::new()),
        }
    }
}

/// 设置页「LibChecker 规则」卡片的状态（应用级：设置窗关了再开，进行中的检查还在）。
#[derive(Clone)]
pub struct RulesUpdateState {
    pub running: Signal<bool>,
    /// 当前规则版本与来源的说明。
    pub current: Signal<String>,
    /// 上次检查的结果（成功 / 已是最新 / 失败原因）。
    pub message: Signal<String>,
}

impl RulesUpdateState {
    pub fn new() -> Self {
        Self {
            running: signal(false),
            current: signal(String::new()),
            message: signal(String::new()),
        }
    }
}

fn current_rules_text() -> String {
    match crate::libs::current() {
        Some((r, origin)) => {
            let origin = match origin {
                crate::libs::Origin::Bundled => tr!("settings.rules_bundled"),
                crate::libs::Origin::Downloaded => tr!("settings.rules_downloaded"),
            };
            tr!(
                "settings.rules_current",
                version = r.meta().data_version as i64,
                count = r.rules().len() as i64,
                origin = origin
            )
        }
        None => tr!("settings.rules_none"),
    }
}

/// 设置 →「工具」里的规则卡片：当前版本 + 检查更新。
pub fn rules_card(state: &AppState) -> Element {
    let ru = state.rules_update.clone();
    ru.current.set(current_rules_text());
    let st = state.clone();
    Element::card(
        tr!("settings.rules_title"),
        Element::col()
            .width_match()
            .spacing(8)
            .child(
                Element::label(ru.current)
                    .font_size(12.5)
                    .fg_role(Role::TextMuted)
                    .width_match(),
            )
            .child(
                Element::row()
                    .width_match()
                    .spacing(10)
                    .cross(Align::Center)
                    .child(
                        Element::button(t!("settings.rules_check"))
                            .small()
                            .enabled_when({
                                let running = ru.running;
                                move || !running.get()
                            })
                            .on_click(move |_| start_rules_update(&st)),
                    )
                    .child(
                        Element::label(t!("settings.rules_checking"))
                            .font_size(12.0)
                            .fg_role(Role::TextMuted)
                            .visible_when({
                                let running = ru.running;
                                move || running.get()
                            }),
                    )
                    .child(
                        Element::link("LibChecker-Rules")
                            .url(RULES_HOME)
                            .font_size(12.0),
                    ),
            )
            .child(
                Element::label(ru.message)
                    .font_size(12.0)
                    .fg_role(Role::TextMuted)
                    .width_match()
                    .visible_when({
                        let (m, running) = (ru.message, ru.running);
                        move || !running.get() && m.with(|s| !s.is_empty())
                    }),
            ),
    )
}

fn start_rules_update(state: &AppState) {
    let ru = &state.rules_update;
    if ru.running.get() {
        return;
    }
    ru.running.set(true);
    ru.message.set(String::new());
    let tx = state.tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(crate::state::Msg::RulesUpdate(crate::libs::check_update()));
    });
}

/// 后台检查结束：更新卡片，换了新规则时把已打开的文件重新识别一遍。
pub fn on_rules_update(state: &AppState, ctx: &mut EventCtx, result: Result<(u32, bool), String>) {
    let ru = &state.rules_update;
    ru.running.set(false);
    ru.current.set(current_rules_text());
    match result {
        Ok((v, true)) => {
            ru.message
                .set(tr!("settings.rules_updated", version = v as i64));
            ctx.toast_ok(tr!("settings.rules_updated", version = v as i64));
            for doc in state.docs.get() {
                if let Some(info) = doc.current_info() {
                    doc.libs.set(
                        crate::libs::analyze(&info)
                            .map(Rc::new)
                            .into_iter()
                            .collect(),
                    );
                    // 标签页随 info 重建：原样再写一次，让「库」页和标签标题拿到新结果
                    doc.info.set(vec![info]);
                }
            }
        }
        Ok((v, false)) => ru
            .message
            .set(tr!("settings.rules_up_to_date", version = v as i64)),
        Err(e) => {
            log::warn!("LibChecker 规则更新失败: {e}");
            ru.message
                .set(tr!("settings.rules_update_failed", error = e.clone()));
            ctx.toast_err(tr!("settings.rules_update_failed", error = e));
        }
    }
}

/// 打开详情时备好的全部内容，对话框只管展示。
pub struct DetailView {
    label: String,
    icon_svg: Option<Vec<u8>>,
    simple_color_icon: bool,
    detail: Option<RuleDetail>,
    rule_name: String,
    regex: bool,
    /// 命中项（原生库：`名字  (ABI…)`；组件：`类别  全名`）。
    items: Vec<String>,
}

pub fn libs_page(state: &AppState, doc: &Doc, info: Rc<ApkInfo>) -> Element {
    let report = doc.libs.with(|v| v.first().cloned());
    let (Some(rules), Some(report)) = (crate::libs::rules(), report) else {
        return page(section(
            "blocks",
            CARD_PURPLE,
            t!("libs.title"),
            Element::label(t!("libs.no_rules"))
                .width_match()
                .fg_role(Role::TextMuted),
        ));
    };
    page(
        Element::col()
            .width_match()
            .spacing(super::CARD_GAP)
            .child(summary(&report, &info))
            .child(native_section(state, &rules, &report, &info))
            .child(component_section(state, &rules, &report, &info))
            .children(unmatched_section(state, &report, &info)),
    )
}

fn summary(report: &LibReport, info: &ApkInfo) -> Element {
    let body = Element::col()
        .width_match()
        .spacing(6)
        .child(
            Element::label(tr!(
                "libs.summary",
                sdks = report.sdk_count() as i64,
                native = info.native_libs.len() as i64,
                components = info.meta.components.len() as i64
            ))
            .width_match(),
        )
        .child(
            Element::row()
                .cross(Align::Center)
                .spacing(4)
                .child(
                    Element::label(tr!(
                        "libs.rules_source",
                        version = report.data_version as i64
                    ))
                    .font_size(12.0)
                    .fg_role(Role::TextSubtle),
                )
                .child(
                    Element::link("LibChecker-Rules")
                        .url(RULES_HOME)
                        .font_size(12.0),
                )
                .child(
                    Element::label("(Apache-2.0)")
                        .font_size(12.0)
                        .fg_role(Role::TextSubtle),
                ),
        );
    section("blocks", CARD_PURPLE, t!("libs.title"), body)
}

fn native_section(
    state: &AppState,
    rules: &Arc<RuleSet>,
    report: &LibReport,
    info: &ApkInfo,
) -> Element {
    let libs = &info.native_libs;
    let mut body = Element::col().width_match().spacing(2);
    if libs.is_empty() {
        body = body.child(Element::label(t!("field.none")).fg_role(Role::TextSubtle));
    } else if report.native.is_empty() {
        body = body.child(Element::label(t!("libs.no_native_match")).fg_role(Role::TextSubtle));
    }
    for g in &report.native {
        let mut names: Vec<&str> = Vec::new();
        let mut abis = BTreeSet::new();
        for &i in &g.items {
            if !names.contains(&libs[i].name.as_str()) {
                names.push(libs[i].name.as_str());
            }
            abis.insert(libs[i].abi.as_str());
        }
        let items = names
            .iter()
            .map(|n| {
                let abis: Vec<&str> = g
                    .items
                    .iter()
                    .filter(|&&i| libs[i].name == *n)
                    .map(|&i| libs[i].abi.as_str())
                    .collect();
                format!("{n}  ({})", abis.join(", "))
            })
            .collect();
        let trailing = abis.into_iter().collect::<Vec<_>>().join(" · ");
        body = body.child(group_row(
            state,
            rules,
            g,
            names.join(", "),
            trailing,
            items,
        ));
    }

    section(
        "cpu",
        CARD_BLUE,
        tr!("libs.native_title", native = libs.len() as i64),
        body,
    )
}

/// 未被任何规则命中的原生库（按库名合并各 ABI），放在页面最后，免得把组件挤到下面去。
fn unmatched_section(state: &AppState, report: &LibReport, info: &ApkInfo) -> Option<Element> {
    let libs = &info.native_libs;
    let mut unmatched: BTreeMap<&str, (Vec<&str>, u64)> = BTreeMap::new();
    for &i in &report.unmatched_native {
        let e = unmatched.entry(libs[i].name.as_str()).or_default();
        e.0.push(libs[i].abi.as_str());
        e.1 = e.1.max(libs[i].size);
    }
    if unmatched.is_empty() {
        return None;
    }
    let title = tr!("libs.unmatched", count = unmatched.len() as i64);
    let width = state.title_width.get();
    let mut body = Element::col().width_match().spacing(2);
    for (name, (abis, size)) in unmatched {
        body = body.child(
            Element::row()
                .width_match()
                .cross(Align::Center)
                .spacing(12)
                .padding_xy(8, 2)
                .child(
                    selectable(name.to_string())
                        .font_family(super::mono_family())
                        .weight(1.0),
                )
                .child(
                    Element::label(abis.join(" · "))
                        .width(width)
                        .font_size(12.5)
                        .fg_role(Role::TextMuted),
                )
                .child(
                    Element::label(format_file_size(size))
                        .width(90)
                        .font_size(12.5)
                        .fg_role(Role::TextMuted),
                ),
        );
    }
    Some(section("cpu", CARD_ORANGE, title, body))
}

fn component_section(
    state: &AppState,
    rules: &Arc<RuleSet>,
    report: &LibReport,
    info: &ApkInfo,
) -> Element {
    let components = &info.meta.components;
    let pkg = info.meta.package_name.as_deref();
    let matched: usize = report.components.iter().map(|g| g.items.len()).sum();
    let mut body = Element::col().width_match().spacing(2);
    if report.components.is_empty() {
        body = body.child(Element::label(t!("libs.no_components")).fg_role(Role::TextSubtle));
    }
    for g in &report.components {
        let mut kinds: BTreeMap<ComponentKind, usize> = BTreeMap::new();
        for &i in &g.items {
            *kinds.entry(components[i].kind).or_default() += 1;
        }
        let trailing = kinds
            .iter()
            .map(|(k, n)| format!("{} {n}", kind_name(*k)))
            .collect::<Vec<_>>()
            .join(" · ");
        let preview: Vec<&str> = g
            .items
            .iter()
            .take(PREVIEW_NAMES)
            .map(|&i| components[i].short_name(pkg))
            .collect();
        let mut preview_text = preview.join(", ");
        if g.items.len() > PREVIEW_NAMES {
            preview_text = tr!(
                "libs.and_more",
                names = preview_text,
                count = g.items.len() as i64
            );
        }
        let items = g
            .items
            .iter()
            .zip(&g.sources)
            .map(|(&i, s)| {
                let c = &components[i];
                let via = if *s == MatchSource::Action {
                    format!("  [{}]", tr!("libs.via_action"))
                } else {
                    String::new()
                };
                format!("{}  {}{via}", kind_name(c.kind), c.name)
            })
            .collect();
        body = body.child(group_row(state, rules, g, preview_text, trailing, items));
    }
    body = body.child(
        Element::label(tr!(
            "libs.components_footer",
            total = components.len() as i64,
            matched = matched as i64
        ))
        .font_size(12.0)
        .fg_role(Role::TextSubtle)
        .padding_edges(Insets::new(8, 8, 8, 0)),
    );
    section("box", CARD_GREEN, t!("libs.components_title"), body)
}

fn kind_name(kind: ComponentKind) -> String {
    match kind {
        ComponentKind::Activity => tr!("libs.kind.activity"),
        ComponentKind::Service => tr!("libs.kind.service"),
        ComponentKind::Receiver => tr!("libs.kind.receiver"),
        ComponentKind::Provider => tr!("libs.kind.provider"),
    }
}

/// 一个 SDK 分组：图标 + 名称 + 命中预览 + 右侧摘要，整行可点开详情。
fn group_row(
    state: &AppState,
    rules: &Arc<RuleSet>,
    g: &SdkGroup,
    preview: String,
    trailing: String,
    items: Vec<String>,
) -> Element {
    let icon_svg = g.icon_id.as_deref().and_then(|id| rules.icon_svg(id));
    let row = Element::row()
        .width_match()
        .cross(Align::Center)
        .spacing(10)
        .padding_xy(8, 6)
        .corner(6.0)
        .child(sdk_icon(icon_svg.as_deref(), g.simple_color_icon, 24))
        .child(
            Element::col()
                .weight(1.0)
                .spacing(2)
                .child(Element::label(g.label.clone()).font_weight(600))
                .child(
                    Element::label(preview)
                        .font_size(12.0)
                        .fg_role(Role::TextMuted)
                        .width_match(),
                ),
        )
        .child(
            Element::label(trailing)
                .font_size(12.0)
                .fg_role(Role::TextMuted),
        );
    let detail_state = state.lib_detail.clone();
    let lang = crate::libs::detail_locale(&state.locale.language());
    let rules = rules.clone();
    let group = g.clone();
    row.clickable().on_click(move |_| {
        let view = DetailView {
            label: group.label.clone(),
            icon_svg: icon_svg.clone(),
            simple_color_icon: group.simple_color_icon,
            detail: rules.detail(&group.rule, lang),
            // 多个库名分别命中各自的精确规则时，只列第一条反而误导，干脆不列
            rule_name: if group.rule.is_regex_rule || group.items.len() == 1 {
                group.rule.name.clone()
            } else {
                String::new()
            },
            regex: group.rule.is_regex_rule,
            items: items.clone(),
        };
        detail_state.view.set(vec![Rc::new(view)]);
        detail_state.show.set(true);
    })
}

fn sdk_icon(svg: Option<&[u8]>, simple_color: bool, px: i32) -> Element {
    match svg {
        Some(bytes) => {
            let img = Element::image_svg(bytes, None).size(px, px);
            // 单色图标按主题文字色着色，暗色主题下才看得见
            if simple_color {
                img.tint_role(Role::Text)
            } else {
                img
            }
        }
        None => Element::image_content(crate::icons::gray("package")).size(px, px),
    }
}

/// SDK 详情对话框，挂在主窗口根部。
pub fn build_dialog(state: &AppState) -> Element {
    let ds = state.lib_detail.clone();
    let show = ds.show;
    let body = Element::host_signal(ds.view, |v: Rc<DetailView>| detail_body(&v));
    let footer = Element::row()
        .width_match()
        .child(Element::flex_spacer())
        .child(
            Element::button(t!("action.close"))
                .small()
                .neutral()
                .outline_soft()
                .on_click(move |_| show.set(false)),
        );
    Element::dialog_panel(
        show,
        tr!("libs.detail_title"),
        600,
        move |_| show.set(false),
        body,
        footer,
    )
}

fn detail_body(v: &DetailView) -> Element {
    let d = v.detail.clone().unwrap_or_default();
    let mut head = Element::col().weight(1.0).spacing(2).child(
        Element::label(v.label.clone())
            .font_size(16.0)
            .font_weight(600),
    );
    if !d.dev_team.is_empty() {
        head = head.child(
            Element::label(d.dev_team.clone())
                .font_size(12.5)
                .fg_role(Role::TextMuted),
        );
    }
    let mut col = Element::col().width_match().spacing(10).child(
        Element::row()
            .width_match()
            .cross(Align::Center)
            .spacing(12)
            .child(sdk_icon(v.icon_svg.as_deref(), v.simple_color_icon, 36))
            .child(head),
    );
    if !d.description.is_empty() {
        col = col.child(Element::label(d.description.clone()).width_match());
    }
    if !d.source_link.is_empty() {
        col = col.child(
            Element::link(d.source_link.clone())
                .url(d.source_link.clone())
                .font_size(12.5),
        );
    }
    if !v.rule_name.is_empty() {
        let rule = if v.regex {
            tr!("libs.rule_regex", rule = v.rule_name.clone())
        } else {
            tr!("libs.rule_exact", rule = v.rule_name.clone())
        };
        col = col.child(
            Element::label(rule)
                .font_size(12.0)
                .font_family(super::mono_family())
                .fg_role(Role::TextMuted)
                .width_match(),
        );
    }
    col = col
        .child(
            Element::label(tr!("libs.matched_items", count = v.items.len() as i64))
                .font_size(12.5)
                .font_weight(600)
                .fg_role(Role::TextMuted),
        )
        .child(
            super::selectable_lines(super::join_lines(&v.items), 8)
                .font_family(super::mono_family())
                .font_size(12.0),
        );
    if !d.rule_contributors.is_empty() {
        col = col.child(
            Element::label(tr!(
                "libs.contributors",
                names = d.rule_contributors.join(", ")
            ))
            .font_size(12.0)
            .fg_role(Role::TextSubtle)
            .width_match(),
        );
    }
    col
}
