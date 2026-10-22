//! LibChecker 库识别：用 LibChecker-Rules 识别 APK 里的原生库与第三方 SDK 组件。
//! 设计与匹配语义见 `docs/libchecker.md`。

mod report;
mod rules;

pub use report::{analyze, LibReport, MatchSource, SdkGroup};
pub use rules::{
    normalize_digit_class, Rule, RuleDetail, RuleSet, RulesError, RulesMeta, READER_VERSION,
};

/// 构建时内嵌的规则包（`scripts/sync-libchecker-rules.py` 生成）；构建时没有则为空。
static BUNDLED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libchecker-rules.zip"));

/// 加载内嵌规则包。没有内嵌或内容无效时返回 `None`。
pub fn bundled() -> Option<RuleSet> {
    if BUNDLED.is_empty() {
        return None;
    }
    RuleSet::from_zip(BUNDLED.to_vec())
        .map_err(|e| log::warn!("bundled libchecker rules invalid: {e}"))
        .ok()
}
