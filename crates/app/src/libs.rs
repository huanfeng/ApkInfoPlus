//! LibChecker 规则：进程内共享一份，首次用到时在调用线程（解析线程）加载内嵌规则包。

use std::sync::{Arc, LazyLock, RwLock};

use apkinfo_core::info::ApkInfo;
use apkinfo_core::libchecker::{self, LibReport, RuleSet};

static RULES: LazyLock<RwLock<Option<Arc<RuleSet>>>> = LazyLock::new(|| {
    let rules = libchecker::bundled();
    match &rules {
        Some(r) => log::info!(
            "LibChecker 规则 v{}（{} 条）",
            r.meta().data_version,
            r.rules().len()
        ),
        None => log::warn!("没有可用的 LibChecker 规则，库识别不可用"),
    }
    RwLock::new(rules.map(Arc::new))
});

/// 当前规则；没有内嵌规则包时为 `None`。
pub fn rules() -> Option<Arc<RuleSet>> {
    RULES.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 对解析结果跑一遍规则；没有规则时为 `None`。
pub fn analyze(info: &ApkInfo) -> Option<LibReport> {
    rules().map(|r| libchecker::analyze(&r, info))
}

/// 规则详情用的 locale：中文界面用 `zh-Hans`（上游只有简体），其余用 `en`。
pub fn detail_locale(ui_language: &str) -> &'static str {
    if ui_language.starts_with("zh") {
        "zh-Hans"
    } else {
        "en"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_locale_follows_ui_language() {
        assert_eq!(detail_locale("zh-CN"), "zh-Hans");
        assert_eq!(detail_locale("zh-TW"), "zh-Hans");
        assert_eq!(detail_locale("ja"), "en");
        assert_eq!(detail_locale("en"), "en");
    }
}
