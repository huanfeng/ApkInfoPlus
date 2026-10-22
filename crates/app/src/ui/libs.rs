//! 「库」标签页：LibChecker 规则识别出的原生库、SDK 组件与 DEX 包，点开某个 SDK 看详情。
//!
//! 页面里的固定文案一律用 `t!`：切换界面语言时不重建页面也能跟着变。

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
    /// 当前规则的版本与条数；没有规则时版本为 0。
    pub version: Signal<i64>,
    pub count: Signal<i64>,
    pub origin: Signal<Option<crate::libs::Origin>>,
    /// 上次检查的结果；`None` 表示还没查过或正在查。
    pub outcome: Signal<Option<RulesOutcome>>,
    /// 结果里的上游版本号（已更新 / 已是最新）。
    pub outcome_version: Signal<i64>,
    /// 失败原因（原始错误文本，句子由 `t!` 套）。
    pub error: Signal<String>,
}

/// 规则检查的结果类别，界面按它切换对应的 `t!` 文案。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RulesOutcome {
    Updated,
    UpToDate,
    Failed,
}

impl RulesUpdateState {
    pub fn new() -> Self {
        Self {
            running: signal(false),
            version: signal(0),
            count: signal(0),
            origin: signal(None),
            outcome: signal(None),
            outcome_version: signal(0),
            error: signal(String::new()),
        }
    }

    fn refresh(&self) {
        let cur = crate::libs::current();
        self.version.set(
            cur.as_ref()
                .map_or(0, |(r, _)| r.meta().data_version as i64),
        );
        self.count
            .set(cur.as_ref().map_or(0, |(r, _)| r.rules().len() as i64));
        self.origin.set(cur.map(|(_, o)| o));
    }
}

/// 设置 →「工具」里的规则卡片：当前版本 + 检查更新。
pub fn rules_card(state: &AppState) -> Element {
    let ru = state.rules_update.clone();
    ru.refresh();
    let st = state.clone();
    let origin_label = |key: &'static str, want: crate::libs::Origin| {
        let origin = ru.origin;
        Element::label(t!(key))
            .font_size(12.5)
            .fg_role(Role::TextMuted)
            .visible_when(move || origin.get() == Some(want))
    };
    let has_rules = {
        let origin = ru.origin;
        move || origin.get().is_some()
    };
    Element::card(
        t!("settings.rules_title"),
        Element::col()
            .width_match()
            .spacing(8)
            .child(
                Element::row()
                    .width_match()
                    .spacing(4)
                    .child(
                        Element::label(t!(
                            "settings.rules_current",
                            version = ru.version,
                            count = ru.count
                        ))
                        .font_size(12.5)
                        .fg_role(Role::TextMuted)
                        .visible_when(has_rules),
                    )
                    .child(origin_label(
                        "settings.rules_bundled",
                        crate::libs::Origin::Bundled,
                    ))
                    .child(origin_label(
                        "settings.rules_downloaded",
                        crate::libs::Origin::Downloaded,
                    ))
                    .child(
                        Element::label(t!("settings.rules_none"))
                            .font_size(12.5)
                            .fg_role(Role::TextMuted)
                            .width_match()
                            .visible_when(move || !has_rules()),
                    ),
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
            .child(outcome_label(
                &ru,
                RulesOutcome::Updated,
                t!("settings.rules_updated", version = ru.outcome_version),
            ))
            .child(outcome_label(
                &ru,
                RulesOutcome::UpToDate,
                t!("settings.rules_up_to_date", version = ru.outcome_version),
            ))
            .child(outcome_label(
                &ru,
                RulesOutcome::Failed,
                t!("settings.rules_update_failed", error = ru.error),
            )),
    )
}

/// 检查结果的一行提示：只在结果是 `want` 且不在检查中时显示。
fn outcome_label(
    ru: &RulesUpdateState,
    want: RulesOutcome,
    text: windui::i18n::Message,
) -> Element {
    let (outcome, running) = (ru.outcome, ru.running);
    Element::label(text)
        .font_size(12.0)
        .fg_role(Role::TextMuted)
        .width_match()
        .visible_when(move || !running.get() && outcome.get() == Some(want))
}

fn start_rules_update(state: &AppState) {
    let ru = &state.rules_update;
    if ru.running.get() {
        return;
    }
    ru.running.set(true);
    ru.outcome.set(None);
    let tx = state.tx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(crate::state::Msg::RulesUpdate(crate::libs::check_update()));
    });
}

/// 后台检查结束：更新卡片，换了新规则时把已打开的文件重新识别一遍。
pub fn on_rules_update(state: &AppState, ctx: &mut EventCtx, result: Result<(u32, bool), String>) {
    let ru = &state.rules_update;
    ru.running.set(false);
    ru.refresh();
    match result {
        Ok((v, true)) => {
            ru.outcome_version.set(v as i64);
            ru.outcome.set(Some(RulesOutcome::Updated));
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
        Ok((v, false)) => {
            ru.outcome_version.set(v as i64);
            ru.outcome.set(Some(RulesOutcome::UpToDate));
        }
        Err(e) => {
            log::warn!("LibChecker 规则更新失败: {e}");
            ru.error.set(e.clone());
            ru.outcome.set(Some(RulesOutcome::Failed));
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
            .child(dex_section(state, &rules, &report, &info))
            .children(unmatched_section(state, &report, &info)),
    )
}

fn summary(report: &LibReport, info: &ApkInfo) -> Element {
    let body = Element::col()
        .width_match()
        .spacing(6)
        .child(
            Element::label(t!(
                "libs.summary",
                sdks = report.sdk_count() as i64,
                native = info.native_libs.len() as i64,
                components = info.meta.components.len() as i64
            ))
            .width_match(),
        )
        .children(markers_row(report))
        .child(
            Element::row()
                .cross(Align::Center)
                .spacing(4)
                .child(
                    Element::label(t!(
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

/// 技术栈 / 加固标记徽章；一个都没有时不占行。
fn markers_row(report: &LibReport) -> Option<Element> {
    let m = report.markers;
    let badges: Vec<(&'static str, Intent)> = [
        (m.kotlin, "libs.marker.kotlin", Intent::Primary),
        (m.compose, "libs.marker.compose", Intent::Primary),
        (m.qihoo, "libs.marker.qihoo", Intent::Warning),
        (m.secneo, "libs.marker.secneo", Intent::Warning),
    ]
    .into_iter()
    .filter(|(on, _, _)| *on)
    .map(|(_, k, i)| (k, i))
    .collect();
    if badges.is_empty() {
        return None;
    }
    let mut row = Element::flow().spacing(6).run_spacing(4).child(
        Element::label(t!("libs.tech"))
            .font_size(12.5)
            .fg_role(Role::TextMuted),
    );
    for (key, intent) in badges {
        row = row.child(Element::badge_intent(t!(key), intent));
    }
    Some(row)
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
        let items: Vec<String> = names
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
            names.join(", ").into(),
            vec![trailing.into()],
            move || items.clone(),
        ));
    }

    section(
        "cpu",
        CARD_BLUE,
        t!("libs.native_title", native = libs.len() as i64),
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
    let title = t!("libs.unmatched", count = unmatched.len() as i64);
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
        // 「活动 3 · 服务 2」：类别名逐个走 t!，换语言跟随
        let mut trailing: Vec<TextContent> = Vec::new();
        for (k, n) in &kinds {
            if !trailing.is_empty() {
                trailing.push("·".into());
            }
            trailing.push(t!(kind_key(*k)).into());
            trailing.push(n.to_string().into());
        }
        let preview: Vec<&str> = g
            .items
            .iter()
            .take(PREVIEW_NAMES)
            .map(|&i| components[i].short_name(pkg))
            .collect();
        let preview_text = preview_with_more(preview.join(", "), g.items.len());
        let hits: Vec<(ComponentKind, String, bool)> = g
            .items
            .iter()
            .zip(&g.sources)
            .map(|(&i, s)| {
                let c = &components[i];
                (c.kind, c.name.clone(), *s == MatchSource::Action)
            })
            .collect();
        // 详情对话框打开时才拼文本（tr! 定格在打开那一刻的语言）
        let items = move || {
            hits.iter()
                .map(|(kind, name, via_action)| {
                    let via = if *via_action {
                        format!("  [{}]", tr!("libs.via_action"))
                    } else {
                        String::new()
                    };
                    format!("{}  {name}{via}", tr!(kind_key(*kind)))
                })
                .collect()
        };
        body = body.child(group_row(state, rules, g, preview_text, trailing, items));
    }
    body = body.child(
        Element::label(t!(
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

/// 「a, b, c 等 N 个」：超过预览数时整句走 `t!`，换语言跟随。
fn preview_with_more(names: String, total: usize) -> TextContent {
    if total > PREVIEW_NAMES {
        t!("libs.and_more", names = names, count = total as i64).into()
    } else {
        names.into()
    }
}

/// DEX 里的包按 type 5 规则（包名前缀）识别出的 SDK。
fn dex_section(
    state: &AppState,
    rules: &Arc<RuleSet>,
    report: &LibReport,
    info: &ApkInfo,
) -> Element {
    let mut body = Element::col().width_match().spacing(2);
    let Some(dex) = &info.dex else {
        body = body.child(Element::label(t!("libs.no_dex")).fg_role(Role::TextSubtle));
        return section("hash", CARD_PURPLE, t!("libs.dex_title"), body);
    };
    if report.dex.is_empty() {
        body = body.child(Element::label(t!("libs.no_dex_match")).fg_role(Role::TextSubtle));
    }
    let pkgs = &report.dex_packages;
    for g in &report.dex {
        let preview: Vec<&str> = g
            .items
            .iter()
            .take(PREVIEW_NAMES)
            .map(|&i| pkgs[i].as_str())
            .collect();
        let items: Vec<String> = g
            .items
            .iter()
            .map(|&i| {
                let classes = dex.packages.get(&pkgs[i]).copied().unwrap_or(0);
                format!("{}  ({classes})", pkgs[i])
            })
            .collect();
        body = body.child(group_row(
            state,
            rules,
            g,
            preview_with_more(preview.join(", "), g.items.len()),
            vec![t!("libs.packages", count = g.items.len() as i64).into()],
            move || items.clone(),
        ));
    }
    body = body.child(
        Element::label(t!(
            "libs.dex_footer",
            dex = dex.dex_count as i64,
            classes = dex.class_count as i64,
            packages = dex.packages.len() as i64
        ))
        .font_size(12.0)
        .fg_role(Role::TextSubtle)
        .padding_edges(Insets::new(8, 8, 8, 0)),
    );
    section("hash", CARD_PURPLE, t!("libs.dex_title"), body)
}

fn kind_key(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Activity => "libs.kind.activity",
        ComponentKind::Service => "libs.kind.service",
        ComponentKind::Receiver => "libs.kind.receiver",
        ComponentKind::Provider => "libs.kind.provider",
    }
}

/// 一个 SDK 分组：图标 + 名称 + 命中预览 + 右侧摘要，整行可点开详情。
fn group_row(
    state: &AppState,
    rules: &Arc<RuleSet>,
    g: &SdkGroup,
    preview: TextContent,
    trailing: Vec<TextContent>,
    items: impl Fn() -> Vec<String> + 'static,
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
            Element::row().spacing(4).children(
                trailing
                    .into_iter()
                    .map(|t| Element::label(t).font_size(12.0).fg_role(Role::TextMuted)),
            ),
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
            items: items(),
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
        t!("libs.detail_title"),
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
            t!("libs.rule_regex", rule = v.rule_name.clone())
        } else {
            t!("libs.rule_exact", rule = v.rule_name.clone())
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
            Element::label(t!("libs.matched_items", count = v.items.len() as i64))
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
            Element::label(t!(
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
