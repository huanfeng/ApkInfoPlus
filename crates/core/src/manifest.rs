//! AndroidManifest.xml（已解码的元素树）→ [`ApkMeta`]。含混淆 APK 启发式。
//! 移植自 Dart 版 `manifest_parser.dart`。

use std::sync::LazyLock;

use regex::Regex;

use crate::axml::XmlElement;
use crate::model::{ActivityInfo, ApkMeta};

/// Java 包名 / 全限定类名格式。用 `[0-9]` 而非 `\d`，与 Dart（JS 风格正则）一样只认 ASCII 数字。
static PACKAGE_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z][a-zA-Z0-9_]*(\.[a-zA-Z][a-zA-Z0-9_]*)+$").expect("valid regex")
});

static VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]+\.[0-9]+(\.[0-9]+)*([.\-_a-zA-Z0-9]*)$").expect("valid regex")
});

/// supports-screens 属性名 → aapt2 输出名。顺序即输出顺序（按框架资源 ID 升序，
/// 等同于 Dart 版按二进制 XML 中属性出现顺序遍历的结果）。
const SCREEN_NAME_MAP: &[(&str, &str)] = &[
    ("anyDensity", "anyDensity"),
    ("smallScreens", "small"),
    ("normalScreens", "normal"),
    ("largeScreens", "large"),
    ("resizeable", "resizeable"),
    ("xlargeScreens", "xlarge"),
];

/// 解析文档根（[`crate::axml::parse_document`] 的返回值）。
pub fn parse_manifest(doc: &XmlElement) -> ApkMeta {
    let mut meta = ApkMeta::default();
    let Some(manifest) = doc.child("manifest") else {
        return meta;
    };
    let attr = |el: &XmlElement, k: &str| el.attr(k).map(str::to_string);

    meta.package_name = attr(manifest, "package");
    meta.version_code = parse_int(manifest.attr("versionCode"));
    meta.version_name = attr(manifest, "versionName");
    // 混淆 APK 可能无法通过属性名找到 package / versionName，启发式兜底
    if meta.package_name.is_none() {
        meta.package_name = guess_package_name(manifest);
    }
    if meta.version_name.is_none() {
        meta.version_name = guess_version_name(manifest);
    }
    meta.compile_sdk_version = parse_int(manifest.attr("compileSdkVersion"));
    meta.compile_sdk_version_codename = attr(manifest, "compileSdkVersionCodename");
    meta.platform_build_version_name = attr(manifest, "platformBuildVersionName");
    meta.platform_build_version_code = parse_int(manifest.attr("platformBuildVersionCode"));

    if let Some(sdk) = manifest.child("uses-sdk") {
        meta.min_sdk_version = parse_int(sdk.attr("minSdkVersion"));
        meta.target_sdk_version = parse_int(sdk.attr("targetSdkVersion"));
    }

    // `uses-permission-sdk-23` 与带 maxSdkVersion 的声明也算权限；后者在名字后标注上限，
    // 与 aapt2 `maxSdkVersion='28'` 的信息等价，便于一眼看出「仅旧系统申请」。
    for el in manifest
        .children_named("uses-permission")
        .chain(manifest.children_named("uses-permission-sdk-23"))
    {
        if let Some(name) = el.attr("name").filter(|n| !n.is_empty()) {
            let mut entry = name.to_string();
            if let Some(max) = el.attr("maxSdkVersion").filter(|m| !m.is_empty()) {
                entry.push_str(&format!(" (maxSdk {})", max));
            }
            meta.permissions.push(entry);
        }
    }

    for el in manifest.children_named("uses-feature") {
        if let Some(name) = el.attr("name").filter(|n| !n.is_empty()) {
            if el.attr("required") == Some("false") {
                meta.features_not_required.push(name.to_string());
            } else {
                meta.features.push(name.to_string());
            }
        }
    }

    if let Some(screens) = manifest.child("supports-screens") {
        meta.screen_sizes = supported_screens(screens);
    }
    // 没解析到任何 screen 属性（元素缺失或被混淆）时，targetSdk >= 4 默认支持全部尺寸
    if meta.screen_sizes.is_empty() && meta.target_sdk_version.unwrap_or(0) >= 4 {
        meta.screen_sizes = ["small", "normal", "large", "xlarge"]
            .map(String::from)
            .to_vec();
    }

    if let Some(app) = manifest.child("application") {
        meta.application_name = attr(app, "name");
        meta.label = attr(app, "label");
        meta.application_icon = attr(app, "icon");
        if meta.application_name.is_none() {
            meta.application_name = guess_class_name(app);
        }
        for activity in &app.children {
            if activity.name != "activity" && activity.name != "activity-alias" {
                continue;
            }
            if !is_launchable_activity(activity) {
                continue;
            }
            meta.launchable_activities.push(ActivityInfo {
                name: attr(activity, "name").or_else(|| guess_class_name(activity)),
                label: attr(activity, "label"),
                icon: attr(activity, "icon"),
            });
        }
    }
    meta
}

fn supported_screens(screens: &XmlElement) -> Vec<String> {
    let mut out: Vec<String> = SCREEN_NAME_MAP
        .iter()
        .filter(|(k, _)| screens.attr(k) == Some("true"))
        .map(|(_, v)| v.to_string())
        .collect();
    // 未知属性名原样输出（Dart 版的 `mapped ?? attr.key`）
    for (k, v) in &screens.attributes {
        if v == "true" && !SCREEN_NAME_MAP.iter().any(|(m, _)| m == k) {
            out.push(k.clone());
        }
    }
    out
}

fn is_launchable_activity(activity: &XmlElement) -> bool {
    activity.children_named("intent-filter").any(|filter| {
        let has_main = filter
            .children
            .iter()
            .any(|c| c.name == "action" && c.attr("name") == Some("android.intent.action.MAIN"));
        let has_launcher = filter.children.iter().any(|c| {
            c.name == "category" && c.attr("name") == Some("android.intent.category.LAUNCHER")
        });
        has_main && has_launcher
    })
}

/// 与 Dart `_parseInt` 一致：`0x` 前缀按十六进制，否则十进制（允许正负号与首尾空白）。
fn parse_int(value: Option<&str>) -> Option<i64> {
    let v = value?.trim();
    if v.is_empty() {
        return None;
    }
    if let Some(hex) = v.strip_prefix("0x") {
        return i64::from_str_radix(hex, 16).ok();
    }
    v.parse().ok()
}

fn is_class_like(value: &str) -> bool {
    value.contains('.')
        && !value.starts_with('@')
        && !value.starts_with("http")
        && !value.contains(' ')
        && PACKAGE_NAME_RE.is_match(value)
}

/// 包名启发式：属性值中符合包名格式的最短者（类名通常更长）。
fn guess_package_name(manifest: &XmlElement) -> Option<String> {
    let mut shortest: Option<&String> = None;
    for v in manifest.attributes.values() {
        if is_class_like(v) && shortest.is_none_or(|s| v.len() < s.len()) {
            shortest = Some(v);
        }
    }
    shortest.cloned()
}

/// 版本名启发式：先查属性值，再查从混淆属性 ns 字段恢复的额外字符串。
fn guess_version_name(manifest: &XmlElement) -> Option<String> {
    manifest
        .attributes
        .values()
        .chain(manifest.extra_strings.iter())
        .find(|v| VERSION_RE.is_match(v))
        .cloned()
}

/// 值虽形如类名、但语义上不是组件自身类名的已知属性。
/// 例如未声明 Application 子类的 APK 会被误取 `appComponentFactory`
/// （androidx.core.app.CoreComponentFactory）作为 application name。
const NON_CLASS_NAME_ATTRS: &[&str] = &[
    "appComponentFactory",
    "backupAgent",
    "theme",
    "label",
    "icon",
    "roundIcon",
    "banner",
    "logo",
    "permission",
    "process",
    "taskAffinity",
    "parentActivityName",
    "targetActivity",
    "manageSpaceActivity",
    "zygotePreloadName",
    "classLoader",
    "networkSecurityConfig",
];

/// 类名启发式：第一个形如全限定类名、且不属于已知非类名属性的属性值。
/// 混淆 APK 的属性名多为 `attr_N`，不受排除列表影响。
fn guess_class_name(el: &XmlElement) -> Option<String> {
    el.attributes
        .iter()
        .filter(|(k, _)| !NON_CLASS_NAME_ATTRS.contains(&k.as_str()))
        .map(|(_, v)| v)
        .find(|v| is_class_like(v))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(name: &str, attrs: &[(&str, &str)], children: Vec<XmlElement>) -> XmlElement {
        XmlElement {
            name: name.into(),
            attributes: attrs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            children,
            extra_strings: Vec::new(),
        }
    }

    fn doc(manifest: XmlElement) -> XmlElement {
        el("", &[], vec![manifest])
    }

    fn launcher_filter() -> XmlElement {
        el(
            "intent-filter",
            &[],
            vec![
                el("action", &[("name", "android.intent.action.MAIN")], vec![]),
                el(
                    "category",
                    &[("name", "android.intent.category.LAUNCHER")],
                    vec![],
                ),
            ],
        )
    }

    #[test]
    fn parses_standard_manifest() {
        let m = el(
            "manifest",
            &[
                ("package", "com.example.app"),
                ("versionCode", "0x10"),
                ("versionName", "1.2.3"),
                ("compileSdkVersion", "34"),
                ("compileSdkVersionCodename", "14"),
                ("platformBuildVersionCode", "34"),
            ],
            vec![
                el(
                    "uses-sdk",
                    &[("minSdkVersion", "21"), ("targetSdkVersion", "34")],
                    vec![],
                ),
                el(
                    "uses-permission",
                    &[("name", "android.permission.INTERNET")],
                    vec![],
                ),
                el("uses-permission", &[("name", "")], vec![]),
                el(
                    "uses-feature",
                    &[("name", "android.hardware.camera")],
                    vec![],
                ),
                el(
                    "uses-feature",
                    &[("name", "android.hardware.nfc"), ("required", "false")],
                    vec![],
                ),
                el(
                    "application",
                    &[
                        ("name", "com.example.App"),
                        ("label", "@res/0x7f010000"),
                        ("icon", "@res/0x7f020000"),
                    ],
                    vec![
                        el(
                            "activity",
                            &[("name", ".Main"), ("label", "Main")],
                            vec![launcher_filter()],
                        ),
                        el("activity", &[("name", ".Other")], vec![]),
                        el(
                            "activity-alias",
                            &[("name", ".Alias")],
                            vec![launcher_filter()],
                        ),
                    ],
                ),
            ],
        );
        let meta = parse_manifest(&doc(m));
        assert_eq!(meta.package_name.as_deref(), Some("com.example.app"));
        assert_eq!(meta.version_code, Some(16));
        assert_eq!(meta.version_name.as_deref(), Some("1.2.3"));
        assert_eq!(meta.compile_sdk_version, Some(34));
        assert_eq!(meta.min_sdk_version, Some(21));
        assert_eq!(meta.target_sdk_version, Some(34));
        assert_eq!(meta.permissions, vec!["android.permission.INTERNET"]);
        assert_eq!(meta.features, vec!["android.hardware.camera"]);
        assert_eq!(meta.features_not_required, vec!["android.hardware.nfc"]);
        assert_eq!(
            meta.screen_sizes,
            vec!["small", "normal", "large", "xlarge"]
        );
        assert_eq!(meta.label.as_deref(), Some("@res/0x7f010000"));
        assert_eq!(meta.application_icon.as_deref(), Some("@res/0x7f020000"));
        let names: Vec<_> = meta
            .launchable_activities
            .iter()
            .map(|a| a.name.clone().unwrap())
            .collect();
        assert_eq!(names, vec![".Main", ".Alias"]);
        assert_eq!(meta.launchable_activities[0].label.as_deref(), Some("Main"));
    }

    #[test]
    fn supports_screens_mapping_and_low_target_sdk() {
        let m = el(
            "manifest",
            &[],
            vec![
                el("uses-sdk", &[("targetSdkVersion", "3")], vec![]),
                el(
                    "supports-screens",
                    &[
                        ("smallScreens", "false"),
                        ("largeScreens", "true"),
                        ("anyDensity", "true"),
                        ("foo", "true"),
                    ],
                    vec![],
                ),
            ],
        );
        assert_eq!(
            parse_manifest(&doc(m)).screen_sizes,
            vec!["anyDensity", "large", "foo"]
        );
        let none = el(
            "manifest",
            &[],
            vec![el("uses-sdk", &[("targetSdkVersion", "3")], vec![])],
        );
        assert!(parse_manifest(&doc(none)).screen_sizes.is_empty());
    }

    #[test]
    fn heuristics_for_obfuscated_manifest() {
        let mut m = el(
            "manifest",
            &[
                ("attr_9", "com.example.app.MainActivity"),
                ("attr_7", "com.example.app"),
                ("attr_3", "@res/0x7f01"),
            ],
            vec![el(
                "application",
                &[("attr_1", "not a class"), ("attr_2", "com.example.app.App")],
                vec![el(
                    "activity",
                    &[("attr_5", "com.example.app.Main")],
                    vec![launcher_filter()],
                )],
            )],
        );
        m.extra_strings = vec!["junk".into(), "2.10.0-beta1".into()];
        let meta = parse_manifest(&doc(m));
        assert_eq!(meta.package_name.as_deref(), Some("com.example.app"));
        assert_eq!(meta.version_name.as_deref(), Some("2.10.0-beta1"));
        assert_eq!(
            meta.application_name.as_deref(),
            Some("com.example.app.App")
        );
        assert_eq!(
            meta.launchable_activities[0].name.as_deref(),
            Some("com.example.app.Main")
        );
    }

    #[test]
    fn class_name_heuristic_skips_known_non_class_attrs() {
        let app = el(
            "application",
            &[
                (
                    "appComponentFactory",
                    "androidx.core.app.CoreComponentFactory",
                ),
                ("backupAgent", "com.example.Backup"),
                ("theme", "com.example.Theme"),
            ],
            vec![],
        );
        let meta = parse_manifest(&doc(el("manifest", &[], vec![app])));
        assert_eq!(meta.application_name, None);
    }

    #[test]
    fn version_heuristic_prefers_attribute_values() {
        let mut m = el("manifest", &[("attr_1", "3.4")], vec![]);
        m.extra_strings = vec!["9.9.9".into()];
        assert_eq!(parse_manifest(&doc(m)).version_name.as_deref(), Some("3.4"));
    }

    #[test]
    fn parse_int_variants() {
        assert_eq!(parse_int(Some("42")), Some(42));
        assert_eq!(parse_int(Some("-1")), Some(-1));
        assert_eq!(parse_int(Some("0xff")), Some(255));
        assert_eq!(parse_int(Some("1.0")), None);
        assert_eq!(parse_int(Some("")), None);
        assert_eq!(parse_int(None), None);
    }

    #[test]
    fn missing_manifest_element_gives_default() {
        assert_eq!(parse_manifest(&el("", &[], vec![])), ApkMeta::default());
    }
}
