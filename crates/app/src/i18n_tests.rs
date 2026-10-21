//! 译文一致性：各语言 key 集合一致、占位符一致；代码里用到的 key 都有译文。

#[cfg(test)]
mod tests {
    const FILES: [&str; 6] = [
        include_str!("../i18n/zh-CN.toml"),
        include_str!("../i18n/en.toml"),
        include_str!("../i18n/zh-TW.toml"),
        include_str!("../i18n/zh-HK.toml"),
        include_str!("../i18n/ja.toml"),
        include_str!("../i18n/ko.toml"),
    ];

    #[test]
    fn translations_stay_in_sync() {
        let problems = windui::i18n::lint::check(&FILES);
        assert!(problems.is_empty(), "{problems:#?}");
    }

    #[test]
    fn every_key_used_is_translated() {
        let locales = windui::i18n::Locales::builder()
            .embed(FILES[0])
            .embed(FILES[1])
            .fallback("en")
            .build();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let problems = windui::i18n::lint::check_usage(dir, &locales);
        assert!(problems.is_empty(), "{problems:#?}");
    }
}
