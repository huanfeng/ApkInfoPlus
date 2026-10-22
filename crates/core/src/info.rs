//! 面向界面的汇总模型 [`ApkInfo`] 与展示用格式化函数。
//!
//! 解析流水线（`apk` / `archive`）产出 `ApkInfo`，界面只读它；这里不碰 zip 与二进制格式。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model::ApkMeta;

/// 文件容器类型。`Apk` 是单个安装包，其余是「装着若干 APK 的归档」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveKind {
    Apk,
    Xapk,
    Apkm,
    Apks,
    /// 任意 zip：根目录没有 AndroidManifest.xml，但含 .apk 条目。
    Zip,
}

impl ArchiveKind {
    /// 按扩展名初判（`.zip` 的最终归属要看内容，见 `archive` 模块）。
    pub fn from_path(path: &Path) -> ArchiveKind {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name.ends_with(".xapk") {
            ArchiveKind::Xapk
        } else if name.ends_with(".apkm") {
            ArchiveKind::Apkm
        } else if name.ends_with(".apks") {
            ArchiveKind::Apks
        } else if name.ends_with(".zip") {
            ArchiveKind::Zip
        } else {
            ArchiveKind::Apk
        }
    }

    pub fn is_archive(self) -> bool {
        !matches!(self, ArchiveKind::Apk)
    }

    /// 界面与文本信息里显示的名字。
    pub fn display_name(self) -> &'static str {
        match self {
            ArchiveKind::Apk => "APK",
            ArchiveKind::Xapk => "XAPK",
            ArchiveKind::Apkm => "APKM",
            ArchiveKind::Apks => "APKS",
            ArchiveKind::Zip => "ZIP",
        }
    }
}

/// 分包类别，用于安装对话框分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SplitType {
    Base,
    Language,
    Abi,
    Density,
    Unknown,
}

impl SplitType {
    /// 由分包文件名（可带路径与扩展名）判类别。
    ///
    /// 相比 Dart 版，这里先剥掉 `config.` / `split_config.` 前缀再判断，
    /// 否则 `config.en` 这类语言包会落到 Unknown。
    pub fn from_split_name(name: &str) -> SplitType {
        let stem = split_stem(name);
        let lower = stem.to_lowercase();
        if is_base_split(&lower) {
            return SplitType::Base;
        }
        let suffix = lower
            .strip_prefix("split_config.")
            .or_else(|| lower.strip_prefix("config."))
            .unwrap_or(&lower);
        if suffix.ends_with("dpi") {
            return SplitType::Density;
        }
        if is_abi_token(suffix) {
            return SplitType::Abi;
        }
        if is_language_token(suffix) {
            return SplitType::Language;
        }
        SplitType::Unknown
    }

    /// 译文 key 的尾段（`install.split_type.<key>`）。
    pub fn key(self) -> &'static str {
        match self {
            SplitType::Base => "base",
            SplitType::Language => "language",
            SplitType::Abi => "abi",
            SplitType::Density => "density",
            SplitType::Unknown => "unknown",
        }
    }
}

/// 去掉目录与扩展名：`splits/config.arm64_v8a.apk` → `config.arm64_v8a`。
pub fn split_stem(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 => &base[..i],
        _ => base,
    }
}

/// `base` 或 `base-xxx` / `base_xxx` / `base.xxx`。
pub fn is_base_split(stem_lower: &str) -> bool {
    stem_lower == "base"
        || stem_lower.starts_with("base-")
        || stem_lower.starts_with("base_")
        || stem_lower.starts_with("base.")
}

pub fn is_abi_token(s: &str) -> bool {
    matches!(
        s,
        "armeabi"
            | "armeabi_v7a"
            | "armeabi-v7a"
            | "arm64_v8a"
            | "arm64-v8a"
            | "x86"
            | "x86_64"
            | "mips"
            | "mips64"
    ) || s.contains("v7a")
        || s.contains("v8a")
        || s.contains("x86")
}

/// `en`、`zh`、`pt-rbr`、`zh_cn` 这类语言标记。
pub fn is_language_token(s: &str) -> bool {
    let (lang, region) = match s.split_once(['-', '_']) {
        Some((l, r)) => (l, Some(r)),
        None => (s, None),
    };
    if !(2..=3).contains(&lang.len()) || !lang.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    match region {
        None => true,
        Some(r) => {
            let r = r.strip_prefix('r').unwrap_or(r);
            r.len() == 2 && r.chars().all(|c| c.is_ascii_alphabetic())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitEntry {
    /// zip 内路径（安装时按它解压）。
    pub path: String,
    pub size: u64,
    pub kind: SplitType,
}

impl SplitEntry {
    pub fn display_name(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObbEntry {
    pub path: String,
    pub size: u64,
}

/// 一个签名者的证书信息。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignerInfo {
    pub subject: String,
    pub issuer: String,
    pub serial_hex: String,
    pub not_before: String,
    pub not_after: String,
    pub sha256: String,
    pub sha1: String,
    pub md5: String,
    pub key_algorithm: String,
    pub key_size_bits: Option<u32>,
    pub signature_algorithm: String,
}

/// 签名概览：存在哪些方案 + 各方案的签名者。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignatureInfo {
    pub has_v1: bool,
    pub has_v2: bool,
    pub has_v3: bool,
    pub has_v31: bool,
    pub has_v4_stamp: bool,
    pub signers: Vec<SignerInfo>,
    /// 解析过程中的非致命问题（例如某个块无法解码）。
    pub warnings: Vec<String>,
}

impl SignatureInfo {
    pub fn schemes(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.has_v1 {
            v.push("v1");
        }
        if self.has_v2 {
            v.push("v2");
        }
        if self.has_v3 {
            v.push("v3");
        }
        if self.has_v31 {
            v.push("v3.1");
        }
        v
    }
}

/// APK 内的一个原生库（`lib/<abi>/<name>.so`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLib {
    pub abi: String,
    /// 文件名，不含目录。
    pub name: String,
    /// 解压后大小。
    pub size: u64,
    /// 来自归档包里的哪个分包；base APK 内的为 `None`。
    pub source: Option<String>,
}

/// 从 zip 条目清单挑出原生库，按 ABI、名字排序。
pub fn native_libs_from_entries(
    entries: impl IntoIterator<Item = (String, u64)>,
    source: Option<&str>,
) -> Vec<NativeLib> {
    let mut out: Vec<NativeLib> = entries
        .into_iter()
        .filter_map(|(path, size)| {
            let rest = path.strip_prefix("lib/")?;
            let (abi, name) = rest.split_once('/')?;
            if abi.is_empty() || name.contains('/') || !name.ends_with(".so") {
                return None;
            }
            Some(NativeLib {
                abi: abi.to_string(),
                name: name.to_string(),
                size,
                source: source.map(str::to_string),
            })
        })
        .collect();
    out.sort_by(|a, b| a.abi.cmp(&b.abi).then_with(|| a.name.cmp(&b.name)));
    out
}

/// 界面消费的完整信息。由解析线程产出一次，之后只读。
#[derive(Debug, Clone, PartialEq)]
pub struct ApkInfo {
    pub path: PathBuf,
    pub size: u64,
    pub kind: ArchiveKind,
    /// Manifest / 资源表信息（归档包时来自 base APK）。
    pub meta: ApkMeta,
    /// 主图标路径（资源表里 density 最高的那条，或归档包 manifest 指定的图标）。
    pub main_icon_path: Option<String>,

    // ---- 归档包（XAPK / APKM / APKS / ZIP）----
    /// manifest.json 里的应用名（可能与 base APK 的 label 不同）。
    pub archive_app_name: Option<String>,
    pub split_configs: Vec<String>,
    pub splits: Vec<SplitEntry>,
    pub obbs: Vec<ObbEntry>,
    /// manifest.json 的 total_size，缺省为文件大小。
    pub total_size: Option<u64>,
    /// 归档包中用于解析的 base APK 条目路径。
    pub base_apk_entry: Option<String>,

    /// 原生库清单（归档包时含 ABI 分包里的）。
    pub native_libs: Vec<NativeLib>,
    /// `classes*.dex` 汇总（归档包时是 base APK 的）；没有 dex 时为 `None`。
    pub dex: Option<crate::dex::DexInfo>,

    /// aapt2 `dump badging` 风格文本。
    pub text_info: String,
}

impl ApkInfo {
    pub fn new(path: PathBuf, size: u64, kind: ArchiveKind, meta: ApkMeta) -> Self {
        let text_info = generate_text_info(&meta);
        Self {
            path,
            size,
            kind,
            meta,
            main_icon_path: None,
            archive_app_name: None,
            split_configs: Vec::new(),
            splits: Vec::new(),
            obbs: Vec::new(),
            total_size: None,
            base_apk_entry: None,
            native_libs: Vec::new(),
            dex: None,
            text_info,
        }
    }

    pub fn is_archive(&self) -> bool {
        self.kind.is_archive()
    }

    /// 展示用应用名：优先指定 locale 的标签，其次默认标签，再次归档包名字。
    pub fn display_label(&self, locale: Option<&str>) -> Option<&str> {
        if let Some(l) = locale {
            if let Some(v) = self.meta.labels.get(l) {
                return Some(v.as_str());
            }
        }
        self.meta
            .label
            .as_deref()
            .or(self.archive_app_name.as_deref())
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// 按 locale 字典序给出「与默认标签不同」的本地化标签，供语言切换菜单使用。
    pub fn alternative_labels(&self) -> Vec<(&str, &str)> {
        let default = self.meta.label.as_deref();
        self.meta
            .labels
            .iter()
            .filter(|(_, v)| Some(v.as_str()) != default)
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    /// 重新生成文本信息（归档包流程在覆盖字段后调用）。
    pub fn refresh_text_info(&mut self) {
        self.text_info = generate_text_info(&self.meta);
        if self.is_archive() {
            let mut extra = String::new();
            extra.push_str(&format!("archive-type: '{}'\n", self.kind.display_name()));
            if let Some(n) = &self.archive_app_name {
                extra.push_str(&format!("archive-name: '{}'\n", n));
            }
            if let Some(t) = self.total_size {
                extra.push_str(&format!("archive-total-size: '{}'\n", t));
            }
            for s in &self.splits {
                extra.push_str(&format!("split-apk: '{}' size='{}'\n", s.path, s.size));
            }
            for o in &self.obbs {
                extra.push_str(&format!("obb-file: '{}' size='{}'\n", o.path, o.size));
            }
            self.text_info.push_str(&extra);
        }
    }
}

// ---------------------------------------------------------------------------
// 格式化
// ---------------------------------------------------------------------------

/// API level → Android 版本名。查不到返回 `None`（界面显示空括号以示未知）。
pub fn android_version_name(sdk: i64) -> Option<&'static str> {
    Some(match sdk {
        37 => "Android 17",
        36 => "Android 16",
        35 => "Android 15",
        34 => "Android 14",
        33 => "Android 13",
        32 => "Android 12L",
        31 => "Android 12",
        30 => "Android 11",
        29 => "Android 10",
        28 => "Android 9.0 (Pie)",
        27 => "Android 8.1 (Oreo)",
        26 => "Android 8.0 (Oreo)",
        25 => "Android 7.1 (Nougat)",
        24 => "Android 7.0 (Nougat)",
        23 => "Android 6.0 (Marshmallow)",
        22 => "Android 5.1 (Lollipop)",
        21 => "Android 5.0 (Lollipop)",
        20 => "Android 4.4W (KitKat Wear)",
        19 => "Android 4.4 (KitKat)",
        18 => "Android 4.3 (Jelly Bean)",
        17 => "Android 4.2 (Jelly Bean)",
        16 => "Android 4.1 (Jelly Bean)",
        15 => "Android 4.0.3 (ICS)",
        14 => "Android 4.0 (ICS)",
        13 => "Android 3.2 (Honeycomb)",
        12 => "Android 3.1 (Honeycomb)",
        11 => "Android 3.0 (Honeycomb)",
        10 => "Android 2.3.3 (Gingerbread)",
        9 => "Android 2.3 (Gingerbread)",
        8 => "Android 2.2 (Froyo)",
        7 => "Android 2.1 (Eclair)",
        6 => "Android 2.0.1 (Eclair)",
        5 => "Android 2.0 (Eclair)",
        4 => "Android 1.6 (Donut)",
        3 => "Android 1.5 (Cupcake)",
        2 => "Android 1.1",
        1 => "Android 1.0",
        _ => return None,
    })
}

/// `34 (Android 14)`；未知版本 `99 ()`；`None` → 空串。
pub fn sdk_version_text(sdk: Option<i64>) -> String {
    match sdk {
        None => String::new(),
        Some(v) => format!("{} ({})", v, android_version_name(v).unwrap_or("")),
    }
}

/// `1.5 KB` / `12.34 MB` / `512 B`：保留两位小数并去掉多余的 0。
pub fn format_file_size(size: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = size as f64;
    let mut idx = 0;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        return format!("{} B", size);
    }
    let s = format!("{:.2}", value);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{} {}", s, UNITS[idx])
}

/// 大小 + 精确字节数：`12.34 MB (12939264 Bytes)`。
pub fn format_file_size_full(size: u64) -> String {
    format!("{} ({} Bytes)", format_file_size(size), size)
}

/// 生成 aapt2 `dump badging` 风格的文本信息（与 Dart 版 `_generateTextInfo` 同格式）。
pub fn generate_text_info(meta: &ApkMeta) -> String {
    fn opt_i(v: Option<i64>) -> String {
        v.map(|x| x.to_string()).unwrap_or_default()
    }
    fn opt_s(v: &Option<String>) -> &str {
        v.as_deref().unwrap_or("")
    }

    let mut out = String::new();
    out.push_str(&format!(
        "package: name='{}' versionCode='{}' versionName='{}' compileSdkVersion='{}' compileSdkVersionCodename='{}'\n",
        opt_s(&meta.package_name),
        opt_i(meta.version_code),
        opt_s(&meta.version_name),
        opt_i(meta.compile_sdk_version),
        opt_s(&meta.compile_sdk_version_codename),
    ));
    if let Some(v) = meta.min_sdk_version {
        out.push_str(&format!("sdkVersion:'{}'\n", v));
    }
    if let Some(v) = meta.target_sdk_version {
        out.push_str(&format!("targetSdkVersion:'{}'\n", v));
    }
    if let Some(l) = &meta.label {
        out.push_str(&format!("application-label:'{}'\n", l));
    }
    for (loc, l) in &meta.labels {
        out.push_str(&format!("application-label-{}:'{}'\n", loc, l));
    }
    for (dpi, p) in &meta.icon_paths {
        out.push_str(&format!("application-icon-{}:'{}'\n", dpi, p));
    }
    out.push_str(&format!(
        "application: label='{}' icon='{}'\n",
        opt_s(&meta.label),
        opt_s(&meta.application_icon)
    ));
    for a in &meta.launchable_activities {
        out.push_str(&format!(
            "launchable-activity: name='{}'  label='{}' icon='{}'\n",
            opt_s(&a.name),
            opt_s(&a.label),
            opt_s(&a.icon)
        ));
    }
    for p in &meta.permissions {
        out.push_str(&format!("uses-permission: name='{}'\n", p));
    }
    for f in &meta.features {
        out.push_str(&format!("uses-feature: name='{}'\n", f));
    }
    for f in &meta.features_not_required {
        out.push_str(&format!("uses-feature-not-required: name='{}'\n", f));
    }
    if !meta.screen_sizes.is_empty() {
        out.push_str("supports-screens:");
        for s in &meta.screen_sizes {
            out.push_str(&format!(" '{}'", s));
        }
        out.push('\n');
    }
    out.push_str("locales: '--_--'");
    for l in &meta.locales {
        out.push_str(&format!(" '{}'", l));
    }
    out.push('\n');
    if !meta.densities.is_empty() {
        out.push_str("densities:");
        for d in &meta.densities {
            out.push_str(&format!(" '{}'", d));
        }
        out.push('\n');
    }
    if !meta.native_codes.is_empty() {
        out.push_str("native-code:");
        for n in &meta.native_codes {
            out.push_str(&format!(" '{}'", n));
        }
        out.push('\n');
    }
    out
}

/// 把 `BTreeMap<u32, String>` 形式的 icon_paths 转成 dpi 字符串键（文本信息与旧格式兼容）。
pub fn icon_paths_by_dpi_label(paths: &BTreeMap<u32, String>) -> Vec<(String, String)> {
    paths
        .iter()
        .map(|(d, p)| (d.to_string(), p.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_libs_are_filtered_and_sorted() {
        let entries = [
            ("lib/x86/libb.so", 2),
            ("lib/arm64-v8a/liba.so", 1),
            ("lib/arm64-v8a/sub/libc.so", 3),
            ("lib/arm64-v8a/readme.txt", 4),
            ("lib/liba.so", 5),
            ("assets/lib/x86/libd.so", 6),
        ]
        .map(|(n, s)| (n.to_string(), s));
        let libs = native_libs_from_entries(entries, Some("config.x86.apk"));
        let got: Vec<_> = libs
            .iter()
            .map(|l| (l.abi.as_str(), l.name.as_str(), l.size))
            .collect();
        assert_eq!(
            got,
            vec![("arm64-v8a", "liba.so", 1), ("x86", "libb.so", 2)]
        );
        assert_eq!(libs[0].source.as_deref(), Some("config.x86.apk"));
    }

    #[test]
    fn file_size_formatting() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(1023), "1023 B");
        assert_eq!(format_file_size(1024), "1 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
        assert_eq!(format_file_size(10 * 1024 * 1024), "10 MB");
        assert_eq!(format_file_size(12_939_264), "12.34 MB");
    }

    #[test]
    fn sdk_text() {
        assert_eq!(sdk_version_text(Some(34)), "34 (Android 14)");
        assert_eq!(sdk_version_text(Some(99)), "99 ()");
        assert_eq!(sdk_version_text(None), "");
    }

    #[test]
    fn split_classification() {
        assert_eq!(SplitType::from_split_name("base.apk"), SplitType::Base);
        assert_eq!(
            SplitType::from_split_name("x/base-master.apk"),
            SplitType::Base
        );
        assert_eq!(
            SplitType::from_split_name("config.en.apk"),
            SplitType::Language
        );
        assert_eq!(
            SplitType::from_split_name("split_config.zh_cn.apk"),
            SplitType::Language
        );
        assert_eq!(
            SplitType::from_split_name("config.arm64_v8a.apk"),
            SplitType::Abi
        );
        assert_eq!(
            SplitType::from_split_name("config.xxhdpi.apk"),
            SplitType::Density
        );
        assert_eq!(
            SplitType::from_split_name("config.pt-rbr.apk"),
            SplitType::Language
        );
        assert_eq!(
            SplitType::from_split_name("feature-maps.apk"),
            SplitType::Unknown
        );
    }

    #[test]
    fn archive_kind_from_extension() {
        assert_eq!(
            ArchiveKind::from_path(Path::new("a.XAPK")),
            ArchiveKind::Xapk
        );
        assert_eq!(
            ArchiveKind::from_path(Path::new("a.apk.1")),
            ArchiveKind::Apk
        );
        assert_eq!(ArchiveKind::from_path(Path::new("a.zip")), ArchiveKind::Zip);
    }

    #[test]
    fn text_info_shape() {
        let m = ApkMeta {
            package_name: Some("a.b".into()),
            min_sdk_version: Some(21),
            permissions: vec!["android.permission.INTERNET".into()],
            ..ApkMeta::default()
        };
        let t = generate_text_info(&m);
        assert!(t.starts_with("package: name='a.b' versionCode='' versionName=''"));
        assert!(t.contains("sdkVersion:'21'\n"));
        assert!(t.contains("uses-permission: name='android.permission.INTERNET'\n"));
        assert!(t.contains("locales: '--_--'\n"));
    }
}
