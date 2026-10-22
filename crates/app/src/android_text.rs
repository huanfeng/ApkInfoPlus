//! Android 平台数据的界面文案：权限标题 / 说明、语言代码的名称。
//!
//! 文案在 `i18n/android/*.toml`（`scripts/gen-android-data.py` 生成），作为 i18n 附加层
//! 跟主译文一起装载，切换界面语言即跟随。

use apkinfo_core::android::{self, LocaleParts, DEFAULT_LOCALE};
use windui::i18n::{LocalesBuilder, Message};
use windui::prelude::Signal;

/// 附加层：与主译文同语言 id，按 key 合并。
pub const LAYERS: [&str; 6] = [
    include_str!("../i18n/android/zh-CN.toml"),
    include_str!("../i18n/android/en.toml"),
    include_str!("../i18n/android/zh-TW.toml"),
    include_str!("../i18n/android/zh-HK.toml"),
    include_str!("../i18n/android/ja.toml"),
    include_str!("../i18n/android/ko.toml"),
];

/// 硬件 / 软件特性说明（手工维护的对照表）。
pub const FEATURE_LAYERS: [&str; 6] = [
    include_str!("../i18n/features/zh-CN.toml"),
    include_str!("../i18n/features/en.toml"),
    include_str!("../i18n/features/zh-TW.toml"),
    include_str!("../i18n/features/zh-HK.toml"),
    include_str!("../i18n/features/ja.toml"),
    include_str!("../i18n/features/ko.toml"),
];

pub fn embed_layers(mut b: LocalesBuilder) -> LocalesBuilder {
    for src in LAYERS.into_iter().chain(FEATURE_LAYERS) {
        b = b.embed(src);
    }
    b
}

const PERM_PREFIX: &str = "android.permission.";

fn has_key(key: &str) -> bool {
    windui::i18n::current().entry(key).is_some()
}

/// 框架权限的标题（`读取联系人`）；没有收录的权限返回 `None`。
pub fn permission_label(name: &str) -> Option<Message> {
    permission_text("perm_label", name)
}

/// 框架权限的说明（一两句话）；没有收录的权限返回 `None`。
pub fn permission_desc(name: &str) -> Option<Message> {
    permission_text("perm_desc", name)
}

fn permission_text(section: &str, name: &str) -> Option<Message> {
    let short = name.strip_prefix(PERM_PREFIX)?;
    let key = format!("{section}.{short}");
    has_key(&key).then(|| Message::new(key))
}

/// `<uses-feature>` 特性的简短说明（`android.hardware.camera` → `后置相机`）；未收录返回 `None`。
pub fn feature_desc(name: &str) -> Option<Message> {
    let key = format!("feature.{name}");
    has_key(&key).then(|| Message::new(key))
}

/// 语言代码在当前界面语言下的名称：`zh-HK` → `中文（香港）`，`sr-Latn` → `塞尔维亚语（拉丁文）`。
/// 认不出的部分原样保留代码。
pub fn locale_name(code: &str) -> String {
    let cat = windui::i18n::current();
    let Some(parts) = LocaleParts::parse(code) else {
        return cat.text(if code == DEFAULT_LOCALE {
            "locale.default"
        } else {
            "field.none"
        });
    };
    let lookup = |section: &str, c: &str| {
        let key = format!("{section}.{c}");
        if cat.entry(&key).is_some() {
            cat.text(&key)
        } else {
            c.to_string()
        }
    };
    let lang = lookup("lang_name", &parts.language);
    let variants: Vec<String> = parts
        .script
        .iter()
        .map(|s| lookup("script_name", s))
        .chain(parts.region.iter().map(|r| lookup("region_name", r)))
        .collect();
    if variants.is_empty() {
        return lang;
    }
    let variant = variants.join(&cat.text("locale.variant_sep"));
    cat.format(
        &Message::new("locale.with_variant")
            .set("lang", lang)
            .set("variant", variant),
    )
}

/// 系统版本名（`Android 7.0 (Nougat)`），作消息参数用：表里查不到时给「未知版本」，
/// 随界面语言变——构建期就 `tr!` 好的串换语言后不会跟着变。
pub fn version_name(ui_lang: Signal<String>, api: i64) -> Signal<String> {
    ui_lang.map(
        move |_| match apkinfo_core::info::android_version_name(api) {
            Some(v) => v.to_string(),
            None => windui::i18n::current().text("sdk.unknown_version"),
        },
    )
}

/// 语言自称（`ja` → `日本語`），与界面语言无关。
pub fn locale_autonym(code: &str) -> Option<&'static str> {
    LocaleParts::parse(code).and_then(|p| android::autonym(&p.language))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::i18n::{Initial, Locales};

    fn install(lang: &str) {
        let b = Locales::builder()
            .embed(include_str!("../i18n/zh-CN.toml"))
            .embed(include_str!("../i18n/en.toml"));
        windui::i18n::install(
            embed_layers(b)
                .fallback("en")
                .initial(Initial::Fixed(lang.into()))
                .build(),
        );
    }

    #[test]
    fn locale_names_follow_ui_language() {
        install("zh-CN");
        assert_eq!(locale_name("en"), "英语");
        assert_eq!(locale_name("zh-HK"), "中文（香港）");
        assert_eq!(locale_name("sr-Latn"), "塞尔维亚语（拉丁文）");
        assert_eq!(locale_name("iw"), "希伯来语");
        assert_eq!(locale_name("xx-YY"), "xx（YY）");
        install("en");
        assert_eq!(locale_name("zh-HK"), "Chinese (Hong Kong)");
        assert_eq!(locale_name("es-419"), "Spanish (Latin America)");
    }

    #[test]
    fn permission_texts_cover_framework_permissions() {
        install("zh-CN");
        let label = permission_label("android.permission.CAMERA").unwrap();
        assert_eq!(windui::i18n::current().format(&label), "拍摄照片和视频");
        assert!(permission_desc("android.permission.INTERNET").is_some());
        assert!(permission_label("com.example.permission.C2D_MESSAGE").is_none());
    }

    #[test]
    fn layers_share_key_sets() {
        for layers in [LAYERS, FEATURE_LAYERS] {
            let problems = windui::i18n::lint::check(&layers);
            assert!(problems.is_empty(), "{problems:#?}");
        }
    }

    #[test]
    fn feature_descriptions_resolve_by_full_name() {
        install("zh-CN");
        let cat = windui::i18n::current();
        let camera = feature_desc("android.hardware.camera").unwrap();
        assert_eq!(cat.format(&camera), "后置相机");
        let autofocus = feature_desc("android.hardware.camera.autofocus").unwrap();
        assert_eq!(cat.format(&autofocus), "相机自动对焦");
        assert!(feature_desc("com.example.feature.x").is_none());
    }
}
