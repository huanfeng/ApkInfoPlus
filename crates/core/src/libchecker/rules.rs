//! LibChecker-Rules v5 portable 规则包的加载与匹配。语义见 `docs/libchecker.md` §2。

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Mutex, OnceLock};

use regex::Regex;
use serde::Deserialize;

use crate::zip::{Archive, ZipError};

/// 我们实现的 portable 读取器版本；规则包要求更高时拒绝加载。
pub const READER_VERSION: u32 = 5;

pub const TYPE_NATIVE: u8 = 0;
pub const TYPE_SERVICE: u8 = 1;
pub const TYPE_ACTIVITY: u8 = 2;
pub const TYPE_RECEIVER: u8 = 3;
pub const TYPE_PROVIDER: u8 = 4;
pub const TYPE_ACTION: u8 = 9;

/// 一期支持的规则类型（与上游 tgbot 能力集一致），其余类型加载时过滤掉。
const SUPPORTED_TYPES: &[u8] = &[
    TYPE_NATIVE,
    TYPE_SERVICE,
    TYPE_ACTIVITY,
    TYPE_RECEIVER,
    TYPE_PROVIDER,
    TYPE_ACTION,
];

/// 正则阶段拒收的行终止符：契约要求点号不匹配它们，这里与 JS 参考实现一样直接跳过正则。
const LINE_TERMINATORS: &[char] = &['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}'];

#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error(transparent)]
    Zip(#[from] ZipError),
    #[error("invalid {0}: {1}")]
    Json(&'static str, serde_json::Error),
    #[error("unsupported rules schema {schema} (reader {READER_VERSION}, needs {minimum})")]
    Unsupported { schema: u32, minimum: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: u32,
    #[serde(default)]
    pub uuid: String,
    pub name: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub icon_id: Option<String>,
    #[serde(default)]
    pub is_simple_color_icon: bool,
    #[serde(default)]
    pub is_regex_rule: bool,
    #[serde(default)]
    pub regex_name: Option<String>,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub detail_path: Option<String>,
}

/// `metadata.json` 里我们关心的字段。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RulesMeta {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub data_version: u32,
    #[serde(default)]
    pub rule_count: u32,
    #[serde(default)]
    pub source_revision: String,
    #[serde(default)]
    pub minimum_reader: MinimumReader,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MinimumReader {
    #[serde(default)]
    pub portable: u32,
}

impl RulesMeta {
    pub fn check_supported(&self) -> Result<(), RulesError> {
        if self.schema_version != READER_VERSION || self.minimum_reader.portable > READER_VERSION {
            return Err(RulesError::Unsupported {
                schema: self.schema_version,
                minimum: self.minimum_reader.portable,
            });
        }
        Ok(())
    }
}

/// 规则详情（某个 locale 的版本）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct RuleDetail {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub dev_team: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source_link: String,
    #[serde(default)]
    pub rule_contributors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LocaleEntry {
    locale: String,
    data: RuleDetail,
}

#[derive(Deserialize)]
struct CoreFile {
    #[serde(rename = "schemaVersion", default)]
    schema_version: u32,
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct DetailFile {
    data: Vec<LocaleEntry>,
}

/// 重新打包后的详情合集（`scripts/sync-libchecker-rules.py` 生成）：
/// `paths` 把 detailPath 映射到 `data` 的下标，内容相同的详情只存一份。
#[derive(Deserialize)]
struct SlimDetails {
    paths: HashMap<String, usize>,
    data: Vec<Vec<LocaleEntry>>,
}

struct TypeIndex {
    exact: HashMap<String, usize>,
    /// 按 (priority, id) 排好序。
    regex: Vec<(Regex, usize)>,
}

pub struct RuleSet {
    meta: RulesMeta,
    rules: Vec<Rule>,
    index: HashMap<u8, TypeIndex>,
    store: Mutex<Archive<Cursor<Vec<u8>>>>,
    slim_details: OnceLock<Option<SlimDetails>>,
}

impl std::fmt::Debug for RuleSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuleSet")
            .field("meta", &self.meta)
            .field("rules", &self.rules.len())
            .finish()
    }
}

impl RuleSet {
    /// 从规则包字节加载：上游 portable-v5.zip 或重新打包后的 rules.zip 都可以。
    pub fn from_zip(bytes: Vec<u8>) -> Result<Self, RulesError> {
        let mut store = Archive::from_bytes(bytes)?;
        let meta: RulesMeta = serde_json::from_slice(&store.read("metadata.json")?)
            .map_err(|e| RulesError::Json("metadata.json", e))?;
        meta.check_supported()?;
        let core: CoreFile = serde_json::from_slice(&store.read("core.json")?)
            .map_err(|e| RulesError::Json("core.json", e))?;
        if core.schema_version != READER_VERSION {
            return Err(RulesError::Unsupported {
                schema: core.schema_version,
                minimum: meta.minimum_reader.portable,
            });
        }
        let mut set = Self::from_rules(core.rules);
        set.meta = meta;
        set.store = Mutex::new(store);
        Ok(set)
    }

    /// 由规则列表直接建索引（测试与 fixtures 用；详情与图标为空）。
    pub fn from_rules(mut rules: Vec<Rule>) -> Self {
        rules.retain(|r| SUPPORTED_TYPES.contains(&r.kind));
        rules.sort_by_key(|r| (r.priority, r.id));
        let mut index: HashMap<u8, TypeIndex> = HashMap::new();
        for (i, rule) in rules.iter().enumerate() {
            let group = index.entry(rule.kind).or_insert_with(|| TypeIndex {
                exact: HashMap::new(),
                regex: Vec::new(),
            });
            // 正则规则的原文也参与精确匹配（旧版行为，契约保留）；重名取优先级高的那条。
            group.exact.entry(rule.name.clone()).or_insert(i);
            if rule.is_regex_rule {
                match Regex::new(&format!("^(?:{})$", normalize_digit_class(&rule.name))) {
                    Ok(re) => group.regex.push((re, i)),
                    Err(e) => log::warn!("skip libchecker rule {}: {e}", rule.id),
                }
            }
        }
        Self {
            meta: RulesMeta::default(),
            rules,
            index,
            store: Mutex::new(
                Archive::from_bytes(empty_zip()).expect("empty zip is a valid archive"),
            ),
            slim_details: OnceLock::new(),
        }
    }

    pub fn meta(&self) -> &RulesMeta {
        &self.meta
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// 在 `kind` 类型下查找命中 `name` 的规则：先精确，再按 (priority, id) 试正则。
    pub fn find(&self, kind: u8, name: &str, use_regex: bool) -> Option<&Rule> {
        let group = self.index.get(&kind)?;
        if let Some(&i) = group.exact.get(name) {
            return Some(&self.rules[i]);
        }
        if !use_regex || name.contains(LINE_TERMINATORS) {
            return None;
        }
        group
            .regex
            .iter()
            .find(|(re, _)| re.is_match(name))
            .map(|(_, i)| &self.rules[*i])
    }

    /// 规则详情：`lang` 是首选 locale（如 `zh-Hans`），缺失时依次退到 `en` 与第一项。
    pub fn detail(&self, rule: &Rule, lang: &str) -> Option<RuleDetail> {
        let path = rule.detail_path.as_deref()?;
        let entries = match self.slim_details() {
            Some(slim) => slim
                .paths
                .get(path)
                .and_then(|&i| slim.data.get(i))?
                .clone(),
            None => {
                let bytes = self.read(path)?;
                serde_json::from_slice::<DetailFile>(&bytes).ok()?.data
            }
        };
        pick_locale(entries, lang)
    }

    /// `icons/<iconId>.svg` 原文。
    pub fn icon_svg(&self, icon_id: &str) -> Option<Vec<u8>> {
        if icon_id.is_empty() || icon_id.contains(['/', '\\']) {
            return None;
        }
        self.read(&format!("icons/{icon_id}.svg"))
    }

    /// 规则包里的任意条目（LICENSE、matching-fixtures.json 等）。
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let mut store = self.store.lock().unwrap_or_else(|e| e.into_inner());
        store.read(name).ok()
    }

    fn slim_details(&self) -> Option<&SlimDetails> {
        self.slim_details
            .get_or_init(|| {
                let bytes = self.read("details.json")?;
                serde_json::from_slice(&bytes)
                    .map_err(|e| log::warn!("libchecker details.json: {e}"))
                    .ok()
            })
            .as_ref()
    }
}

fn pick_locale(entries: Vec<LocaleEntry>, lang: &str) -> Option<RuleDetail> {
    let pos = entries
        .iter()
        .position(|e| e.locale == lang)
        .or_else(|| entries.iter().position(|e| e.locale == "en"))
        .or(if entries.is_empty() { None } else { Some(0) })?;
    entries.into_iter().nth(pos).map(|e| e.data)
}

fn empty_zip() -> Vec<u8> {
    // 只有 EOCD 的合法空 zip。
    let mut v = vec![0x50, 0x4b, 0x05, 0x06];
    v.extend_from_slice(&[0; 18]);
    v
}

/// 契约要求 `\d` 只认 ASCII 数字：按词法把未转义的 `\d` 换成 `[0-9]`（字符类内换成 `0-9`）。
///
/// 同时处理 JS 与 Rust 字符类语法的差异：JS 字符类里的 `[` 是字面量，`&&` / `--` / `~~`
/// 也没有集合运算含义，Rust 里则有，所以这些在字符类内转义掉。
pub fn normalize_digit_class(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('d') if in_class => out.push_str("0-9"),
                Some('d') => out.push_str("[0-9]"),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            },
            '[' if in_class => out.push_str("\\["),
            '[' => {
                in_class = true;
                out.push('[');
                // `[^` 与紧跟的 `]`（JS 里 `[]` 是空类，这里不特殊处理）原样保留
                if chars.peek() == Some(&'^') {
                    out.push(chars.next().unwrap_or('^'));
                }
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            '&' | '-' | '~' if in_class && chars.peek() == Some(&c) => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: u32, kind: u8, name: &str, regex: bool, priority: u32) -> Rule {
        Rule {
            id,
            uuid: String::new(),
            name: name.into(),
            label: format!("L{id}"),
            kind,
            icon_id: None,
            is_simple_color_icon: false,
            is_regex_rule: regex,
            regex_name: None,
            priority,
            detail_path: None,
        }
    }

    #[test]
    fn digit_class_normalization() {
        assert_eq!(normalize_digit_class(r"lib\d+"), "lib[0-9]+");
        assert_eq!(normalize_digit_class(r"[\d]+"), "[0-9]+");
        assert_eq!(normalize_digit_class(r"\\d+"), r"\\d+");
        assert_eq!(normalize_digit_class(r"\\\d+"), r"\\[0-9]+");
        assert_eq!(normalize_digit_class(r"[^\d.]"), "[^0-9.]");
        assert_eq!(normalize_digit_class(r"[a[b]"), r"[a\[b]");
        assert_eq!(normalize_digit_class(r"[a&&b]"), r"[a\&&b]");
        assert_eq!(normalize_digit_class(r"libx\.so"), r"libx\.so");
    }

    #[test]
    fn exact_beats_regex_and_regex_follows_priority() {
        let set = RuleSet::from_rules(vec![
            rule(1, TYPE_NATIVE, "foo|bar", true, 2),
            rule(2, TYPE_NATIVE, "foo", false, 20),
            rule(3, TYPE_NATIVE, ".*", true, 10),
            rule(4, 7, "foo", false, 0),
        ]);
        let id = |n: &str, re: bool| set.find(TYPE_NATIVE, n, re).map(|r| r.id);
        assert_eq!(id("foo", true), Some(2));
        assert_eq!(id("bar", true), Some(1));
        assert_eq!(id("foobar", true), Some(3));
        assert_eq!(id("foo|bar", true), Some(1));
        assert_eq!(id("bar\n", true), None);
        assert_eq!(id("bar", false), None);
        assert_eq!(set.find(TYPE_SERVICE, "foo", true), None);
        // 不支持的类型（7 permission）在加载时被过滤
        assert_eq!(set.rules().len(), 3);
    }

    #[test]
    fn invalid_regex_is_skipped() {
        let set = RuleSet::from_rules(vec![rule(1, TYPE_NATIVE, "(", true, 1)]);
        assert_eq!(set.find(TYPE_NATIVE, "x", true), None);
        assert_eq!(set.find(TYPE_NATIVE, "(", true).map(|r| r.id), Some(1));
    }

    /// 上游 `matching-fixtures.json` 的全部用例（规则包内置时运行）。
    #[test]
    fn upstream_matching_fixtures() {
        let Some(set) = crate::libchecker::bundled() else {
            eprintln!("bundled rules absent, skipping fixtures");
            return;
        };
        let bytes = set
            .read("matching-fixtures.json")
            .expect("fixtures in bundle");
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let rules: Vec<Rule> = serde_json::from_value(serde_json::Value::Array(
                case["rules"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| {
                        let mut r = r.clone();
                        r["label"] = "x".into();
                        r
                    })
                    .collect(),
            ))
            .unwrap();
            let set = RuleSet::from_rules(rules);
            let kind = case["type"].as_u64().unwrap() as u8;
            let use_regex = case["useRegex"].as_bool().unwrap_or(true);
            let got = set
                .find(kind, case["input"].as_str().unwrap(), use_regex)
                .map(|r| r.id as u64);
            assert_eq!(got, case["expected"].as_u64(), "fixture {}", case["name"]);
        }
    }

    #[test]
    fn bundled_rules_load_and_compile() {
        let Some(set) = crate::libchecker::bundled() else {
            return;
        };
        assert!(set.meta().data_version >= 48);
        let regex_rules = set.rules().iter().filter(|r| r.is_regex_rule).count();
        let compiled: usize = set.index.values().map(|g| g.regex.len()).sum();
        assert_eq!(compiled, regex_rules, "every upstream regex compiles");
        let flutter = set.find(TYPE_NATIVE, "libflutter.so", true).unwrap();
        assert_eq!(flutter.label, "Flutter");
        let detail = set.detail(flutter, "en").unwrap();
        assert!(!detail.source_link.is_empty());
        let icon = flutter.icon_id.as_deref().unwrap();
        assert!(set.icon_svg(icon).is_some());
        assert!(set.read("LICENSE").is_some());
    }
}
