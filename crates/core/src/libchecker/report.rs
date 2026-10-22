//! 对一个 [`ApkInfo`] 跑规则，产出按 SDK 分组的报告。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::rules::{
    Rule, RuleSet, TYPE_ACTION, TYPE_ACTIVITY, TYPE_NATIVE, TYPE_PROVIDER, TYPE_RECEIVER,
    TYPE_SERVICE,
};
use crate::info::ApkInfo;
use crate::model::{ComponentInfo, ComponentKind};

/// 命中方式：名字精确相等、正则、或经由 intent-filter action。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchSource {
    Exact,
    Regex,
    Action,
}

/// 同一个 SDK（label + 图标相同）的全部命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkGroup {
    pub label: String,
    pub icon_id: Option<String>,
    pub simple_color_icon: bool,
    /// 组内第一条命中的规则，用来取详情。
    pub rule: Rule,
    /// 命中项在 `ApkInfo::native_libs` 或 `ApkMeta::components` 里的下标，按出现顺序。
    pub items: Vec<usize>,
    /// 每个命中项的命中方式，与 `items` 一一对应。
    pub sources: Vec<MatchSource>,
}

impl SdkGroup {
    fn new(rule: &Rule) -> Self {
        Self {
            label: rule.label.clone(),
            icon_id: rule.icon_id.clone(),
            simple_color_icon: rule.is_simple_color_icon,
            rule: rule.clone(),
            items: Vec::new(),
            sources: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibReport {
    pub data_version: u32,
    /// 原生库命中，组按去重库名数降序、label 升序。
    pub native: Vec<SdkGroup>,
    /// 未命中任何规则的原生库下标。
    pub unmatched_native: Vec<usize>,
    /// 组件命中，组按组件数降序、label 升序。
    pub components: Vec<SdkGroup>,
}

impl LibReport {
    /// 识别出的不同 SDK 个数（原生库与组件按 label + 图标合并计）。
    pub fn sdk_count(&self) -> usize {
        self.native
            .iter()
            .chain(&self.components)
            .map(|g| (g.label.as_str(), g.icon_id.as_deref()))
            .collect::<BTreeSet<_>>()
            .len()
    }
}

pub fn analyze(rules: &RuleSet, info: &ApkInfo) -> LibReport {
    let libs = &info.native_libs;
    let lib_names: BTreeSet<&str> = libs.iter().map(|l| l.name.as_str()).collect();

    let mut native = Grouper::default();
    let mut unmatched_native = Vec::new();
    for (i, lib) in libs.iter().enumerate() {
        let hit = rules
            .find(TYPE_NATIVE, &lib.name, true)
            .filter(|_| passes_native_validation(&lib.name, &lib_names));
        match hit {
            Some(rule) => native.add(rule, i, source_of(rule, &lib.name)),
            None => unmatched_native.push(i),
        }
    }

    let mut components = Grouper::default();
    for (i, c) in info.meta.components.iter().enumerate() {
        if let Some((rule, source)) = match_component(rules, c) {
            components.add(rule, i, source);
        }
    }

    LibReport {
        data_version: rules.meta().data_version,
        native: native.finish(|g| {
            g.items
                .iter()
                .map(|&i| libs[i].name.as_str())
                .collect::<BTreeSet<_>>()
                .len()
        }),
        unmatched_native,
        components: components.finish(|g| g.items.len()),
    }
}

fn match_component<'r>(rules: &'r RuleSet, c: &ComponentInfo) -> Option<(&'r Rule, MatchSource)> {
    let kind = match c.kind {
        ComponentKind::Activity => TYPE_ACTIVITY,
        ComponentKind::Service => TYPE_SERVICE,
        ComponentKind::Receiver => TYPE_RECEIVER,
        ComponentKind::Provider => TYPE_PROVIDER,
    };
    if let Some(rule) = rules.find(kind, &c.name, true) {
        return Some((rule, source_of(rule, &c.name)));
    }
    c.actions
        .iter()
        .find_map(|a| rules.find(TYPE_ACTION, a, false))
        .map(|rule| (rule, MatchSource::Action))
}

fn source_of(rule: &Rule, name: &str) -> MatchSource {
    if rule.name == name {
        MatchSource::Exact
    } else {
        MatchSource::Regex
    }
}

/// 几个过于常见的库名需要旁证才算命中（见设计文档 §2.4）。
/// 加固库的旁证要扫 DEX，DEX 扫描完成前一律不命中。
fn passes_native_validation(name: &str, all: &BTreeSet<&str>) -> bool {
    match name {
        "libjiagu.so" | "libjiagu_a64.so" | "libjiagu_x86.so" | "libjiagu_x64.so" => false,
        "libDexHelper.so" | "libDexHelper-x86.so" | "libdexjni.so" => false,
        "libapp.so" => all.contains("libflutter.so"),
        "libmain.so" => all.contains("libunity.so"),
        _ => true,
    }
}

#[derive(Default)]
struct Grouper {
    groups: Vec<SdkGroup>,
    by_key: HashMap<(String, Option<String>), usize>,
}

impl Grouper {
    fn add(&mut self, rule: &Rule, item: usize, source: MatchSource) {
        let key = (rule.label.clone(), rule.icon_id.clone());
        let idx = *self.by_key.entry(key).or_insert_with(|| {
            self.groups.push(SdkGroup::new(rule));
            self.groups.len() - 1
        });
        let g = &mut self.groups[idx];
        g.items.push(item);
        g.sources.push(source);
    }

    fn finish(self, count: impl Fn(&SdkGroup) -> usize) -> Vec<SdkGroup> {
        let mut keyed: BTreeMap<(std::cmp::Reverse<usize>, String, usize), SdkGroup> =
            BTreeMap::new();
        for (i, g) in self.groups.into_iter().enumerate() {
            keyed.insert((std::cmp::Reverse(count(&g)), g.label.clone(), i), g);
        }
        keyed.into_values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::info::{ArchiveKind, NativeLib};
    use crate::model::ApkMeta;
    use std::path::PathBuf;

    fn rule(id: u32, kind: u8, name: &str, label: &str, regex: bool) -> Rule {
        Rule {
            id,
            uuid: String::new(),
            name: name.into(),
            label: label.into(),
            kind,
            icon_id: Some(format!("ic_{label}")),
            is_simple_color_icon: false,
            is_regex_rule: regex,
            regex_name: None,
            priority: id,
            detail_path: None,
        }
    }

    fn lib(abi: &str, name: &str) -> NativeLib {
        NativeLib {
            abi: abi.into(),
            name: name.into(),
            size: 1,
            source: None,
        }
    }

    fn comp(kind: ComponentKind, name: &str, actions: &[&str]) -> ComponentInfo {
        ComponentInfo {
            kind,
            name: name.into(),
            exported: None,
            actions: actions.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn info(libs: Vec<NativeLib>, components: Vec<ComponentInfo>) -> ApkInfo {
        let meta = ApkMeta {
            components,
            ..Default::default()
        };
        let mut info = ApkInfo::new(PathBuf::from("a.apk"), 0, ArchiveKind::Apk, meta);
        info.native_libs = libs;
        info
    }

    fn rules() -> RuleSet {
        RuleSet::from_rules(vec![
            rule(1, TYPE_NATIVE, "libflutter.so", "Flutter", false),
            rule(2, TYPE_NATIVE, "libapp.so", "Flutter", false),
            rule(3, TYPE_NATIVE, r"libAMapSDK_MAP_v(.*)\.so", "AMap", true),
            rule(4, TYPE_NATIVE, "libjiagu.so", "Jiagu", false),
            rule(5, TYPE_NATIVE, "libmain.so", "Unity", false),
            rule(6, TYPE_SERVICE, "com.push.PushService", "Push", false),
            rule(7, TYPE_ACTION, "com.push.action.RECEIVE", "Push", false),
            rule(8, TYPE_ACTIVITY, r"com\.ads\..*", "Ads", true),
        ])
    }

    #[test]
    fn groups_native_libs_with_validation() {
        let i = info(
            vec![
                lib("arm64-v8a", "libapp.so"),
                lib("arm64-v8a", "libflutter.so"),
                lib("armeabi-v7a", "libflutter.so"),
                lib("arm64-v8a", "libAMapSDK_MAP_v9.so"),
                lib("arm64-v8a", "libjiagu.so"),
                lib("arm64-v8a", "libmain.so"),
                lib("arm64-v8a", "libfoo.so"),
            ],
            vec![],
        );
        let r = analyze(&rules(), &i);
        let got: Vec<_> = r
            .native
            .iter()
            .map(|g| (g.label.as_str(), g.items.clone()))
            .collect();
        assert_eq!(got, vec![("Flutter", vec![0, 1, 2]), ("AMap", vec![3])]);
        assert_eq!(r.native[1].sources, vec![MatchSource::Regex]);
        // libjiagu 缺 DEX 旁证、libmain 缺 libunity.so，与 libfoo 一起算未识别
        assert_eq!(r.unmatched_native, vec![4, 5, 6]);
    }

    #[test]
    fn libapp_needs_libflutter() {
        let r = analyze(&rules(), &info(vec![lib("x86", "libapp.so")], vec![]));
        assert!(r.native.is_empty());
        assert_eq!(r.unmatched_native, vec![0]);
    }

    #[test]
    fn components_match_by_name_regex_and_action() {
        let i = info(
            vec![],
            vec![
                comp(ComponentKind::Service, "com.push.PushService", &[]),
                comp(
                    ComponentKind::Receiver,
                    "x.Recv",
                    &["com.push.action.RECEIVE"],
                ),
                comp(ComponentKind::Activity, "com.ads.Banner", &[]),
                comp(ComponentKind::Activity, "com.ads.Full", &[]),
                comp(ComponentKind::Activity, "com.app.Main", &[]),
                // action 规则不走正则；服务名也不会去匹配 activity 规则
                comp(
                    ComponentKind::Service,
                    "com.ads.Svc",
                    &["com.push.action.RECEIVE.x"],
                ),
            ],
        );
        let r = analyze(&rules(), &i);
        let got: Vec<_> = r
            .components
            .iter()
            .map(|g| (g.label.as_str(), g.items.clone(), g.sources.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "Ads",
                    vec![2, 3],
                    vec![MatchSource::Regex, MatchSource::Regex]
                ),
                (
                    "Push",
                    vec![0, 1],
                    vec![MatchSource::Exact, MatchSource::Action]
                ),
            ]
        );
        assert_eq!(r.sdk_count(), 2);
    }
}
