//! LibChecker 规则：进程内共享一份，首次用到时在调用线程（解析线程）加载。
//!
//! 来源有两个：构建时内嵌的规则包，和设置里「更新规则」下载到数据目录的上游原包。
//! 两份都在时用 `dataVersion` 高的那份；下载的那份坏了或版本不兼容就退回内嵌的。

use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use apkinfo_core::info::ApkInfo;
use apkinfo_core::libchecker::{self, LibReport, RuleSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Bundled,
    Downloaded,
}

type Current = Option<(Arc<RuleSet>, Origin)>;

static RULES: LazyLock<RwLock<Current>> = LazyLock::new(|| {
    let current = choose(libchecker::bundled(), load_downloaded());
    match &current {
        Some((r, origin)) => log::info!(
            "LibChecker 规则 v{}（{} 条，{:?}）",
            r.meta().data_version,
            r.rules().len(),
            origin
        ),
        None => log::warn!("没有可用的 LibChecker 规则，库识别不可用"),
    }
    RwLock::new(current)
});

/// 下载的规则包存放位置。
pub fn downloaded_path() -> PathBuf {
    crate::config::data_dir()
        .join("libchecker")
        .join("portable-v5.zip")
}

fn load_downloaded() -> Option<RuleSet> {
    let path = downloaded_path();
    let bytes = std::fs::read(&path).ok()?;
    RuleSet::from_zip(bytes)
        .map_err(|e| log::warn!("下载的 LibChecker 规则不可用 {}: {e}", path.display()))
        .ok()
}

/// 两份规则里挑 `dataVersion` 高的；相同时用内嵌的（随程序发布、肯定兼容）。
fn choose(bundled: Option<RuleSet>, downloaded: Option<RuleSet>) -> Current {
    let version = |r: &Option<RuleSet>| r.as_ref().map(|r| r.meta().data_version);
    if version(&downloaded) > version(&bundled) {
        downloaded.map(|r| (Arc::new(r), Origin::Downloaded))
    } else {
        bundled.map(|r| (Arc::new(r), Origin::Bundled))
    }
}

/// 当前规则；一份都没有时为 `None`。
pub fn rules() -> Option<Arc<RuleSet>> {
    current().map(|(r, _)| r)
}

pub fn current() -> Current {
    RULES.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 换上新下载的规则（更新线程里调用）。之后打开的文件、以及重新识别的已开文件都用它。
pub fn install_downloaded(rules: RuleSet) {
    *RULES.write().unwrap_or_else(|e| e.into_inner()) = Some((Arc::new(rules), Origin::Downloaded));
}

/// 检查并下载更新（阻塞，放后台线程）。返回上游版本与是否换上了新规则。
pub fn check_update() -> Result<(u32, bool), String> {
    let current = current().map_or(0, |(r, _)| r.meta().data_version);
    let path = downloaded_path();
    match libchecker::update_rules(&path, current)? {
        libchecker::UpdateOutcome::UpToDate(v) => Ok((v, false)),
        libchecker::UpdateOutcome::Updated(v) => {
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            install_downloaded(RuleSet::from_zip(bytes).map_err(|e| e.to_string())?);
            log::info!("LibChecker 规则已更新到 v{v}");
            Ok((v, true))
        }
    }
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
    use apkinfo_core::libchecker::Rule;

    #[test]
    fn detail_locale_follows_ui_language() {
        assert_eq!(detail_locale("zh-CN"), "zh-Hans");
        assert_eq!(detail_locale("zh-TW"), "zh-Hans");
        assert_eq!(detail_locale("ja"), "en");
        assert_eq!(detail_locale("en"), "en");
    }

    #[test]
    fn choose_prefers_newer_and_falls_back() {
        // from_rules 建出的规则集 dataVersion 为 0，够区分「有 / 没有」
        let empty = || RuleSet::from_rules(Vec::<Rule>::new());
        assert_eq!(choose(None, None).map(|c| c.1), None);
        assert_eq!(
            choose(Some(empty()), None).map(|c| c.1),
            Some(Origin::Bundled)
        );
        assert_eq!(
            choose(None, Some(empty())).map(|c| c.1),
            Some(Origin::Downloaded)
        );
        assert_eq!(
            choose(Some(empty()), Some(empty())).map(|c| c.1),
            Some(Origin::Bundled),
            "同版本用内嵌的"
        );
        if let Some(b) = libchecker::bundled() {
            assert_eq!(
                choose(Some(empty()), Some(b)).map(|c| c.1),
                Some(Origin::Downloaded),
                "下载的版本更高就用下载的"
            );
        }
    }
}
