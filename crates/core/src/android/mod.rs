//! Android 平台知识：框架权限的保护级别、资源语言代码的拆解与自称。
//!
//! 数据表 `generated.rs` 由 `scripts/gen-android-data.py` 从 SDK 的 android.jar 与 ICU 生成，
//! 各语言的权限文案 / 语言名称在 app 的 i18n 附加层里（同一脚本生成）。

mod generated;

/// 权限的基本保护级别（`protectionLevel` 低 4 位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionBase {
    Normal,
    Dangerous,
    Signature,
    SignatureOrSystem,
    Internal,
}

/// 框架权限的保护级别：基本级别 + 附加标志。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Protection(u32);

/// 附加标志位与名字（与 `R.attr#protectionLevel` 的取值同名）。
const FLAGS: &[(u32, &str)] = &[
    (0x10, "privileged"),
    (0x20, "development"),
    (0x40, "appop"),
    (0x80, "pre23"),
    (0x100, "installer"),
    (0x200, "verifier"),
    (0x400, "preinstalled"),
    (0x800, "setup"),
    (0x1000, "instant"),
    (0x2000, "runtime"),
    (0x4000, "oem"),
    (0x8000, "vendorPrivileged"),
    (0x10000, "textClassifier"),
    // 0x20000 wellbeing、0x40000 documenter 已废弃，但位仍占着
    (0x20000, "wellbeing"),
    (0x40000, "documenter"),
    (0x80000, "configurator"),
    (0x100000, "incidentReportApprover"),
    (0x200000, "appPredictor"),
    (0x400000, "module"),
    (0x800000, "companion"),
    (0x1000000, "retailDemo"),
    (0x2000000, "recents"),
    (0x4000000, "role"),
    (0x8000000, "knownSigner"),
];

impl Protection {
    pub fn raw(self) -> u32 {
        self.0
    }

    pub fn base(self) -> ProtectionBase {
        match self.0 & 0xf {
            1 => ProtectionBase::Dangerous,
            2 => ProtectionBase::Signature,
            3 => ProtectionBase::SignatureOrSystem,
            4 => ProtectionBase::Internal,
            _ => ProtectionBase::Normal,
        }
    }

    /// 附加标志名，按位序。
    pub fn flags(self) -> Vec<&'static str> {
        FLAGS
            .iter()
            .filter(|(bit, _)| self.0 & bit != 0)
            .map(|(_, name)| *name)
            .collect()
    }
}

/// 框架权限的保护级别；不是框架定义的权限（应用自定义、厂商权限）返回 `None`。
pub fn permission_protection(name: &str) -> Option<Protection> {
    generated::PERMISSIONS
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| Protection(generated::PERMISSIONS[i].1))
}

/// 面向用户的风险分级。Android 的 dangerous 只表示「运行时要用户授权」，存储、通知、
/// 相机都在里面，直接当「危险」显示会让大多数应用满屏红色；这里另分一层：
/// 只有能直接读走通讯与隐私数据、或能产生费用 / 接管设备的才算高风险。
///
/// 变体的声明顺序即显示优先级（`Ord`）：风险高的排前面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionRisk {
    /// 读取短信、通话记录、联系人，后台定位、身体数据、拨号、访问全部文件等。
    High,
    /// 其余运行时权限，以及用户可在设置里单独授予的特殊权限（悬浮窗、安装应用…，见
    /// `SENSITIVE_SPECIAL`）。
    Sensitive,
    /// 安装时自动授予。
    Normal,
    /// 签名 / 系统权限：普通第三方应用拿不到。
    System,
    /// 不是框架定义的权限。
    Unknown,
}

/// 高风险权限（`android.permission.` 之后的部分）。
const HIGH_RISK: &[&str] = &[
    "ACCESS_BACKGROUND_LOCATION",
    "ANSWER_PHONE_CALLS",
    "BODY_SENSORS",
    "BODY_SENSORS_BACKGROUND",
    "CALL_PHONE",
    "MANAGE_EXTERNAL_STORAGE",
    "PROCESS_OUTGOING_CALLS",
    "READ_CALL_LOG",
    "READ_CELL_BROADCASTS",
    "READ_CONTACTS",
    "READ_PHONE_NUMBERS",
    "READ_SMS",
    "RECEIVE_MMS",
    "RECEIVE_SMS",
    "RECEIVE_WAP_PUSH",
    "SEND_SMS",
];

/// 不是运行时权限、但同样敏感的：用户可在「特殊应用权限」里单独授予的签名 | appop 权限，
/// 以及能列出全部已装应用的 QUERY_ALL_PACKAGES。appop 标志本身不足以判断——同样带它的
/// 还有安装即授予的 normal 权限和普通应用拿不到的 privileged 权限，所以列名单。
const SENSITIVE_SPECIAL: &[&str] = &[
    "MANAGE_MEDIA",
    "PACKAGE_USAGE_STATS",
    "QUERY_ALL_PACKAGES",
    "REQUEST_INSTALL_PACKAGES",
    "SYSTEM_ALERT_WINDOW",
    "WRITE_SETTINGS",
];

pub fn permission_risk(name: &str) -> PermissionRisk {
    let Some(level) = permission_protection(name) else {
        return PermissionRisk::Unknown;
    };
    let short = name.strip_prefix("android.permission.").unwrap_or("");
    if HIGH_RISK.contains(&short) {
        return PermissionRisk::High;
    }
    if SENSITIVE_SPECIAL.contains(&short) {
        return PermissionRisk::Sensitive;
    }
    match level.base() {
        ProtectionBase::Dangerous => PermissionRisk::Sensitive,
        ProtectionBase::Normal => PermissionRisk::Normal,
        _ => PermissionRisk::System,
    }
}

/// 拆开 `ApkMeta::permissions` 的条目：`"X (maxSdk 32)"` → `("X", Some(32))`。
pub fn split_permission_entry(entry: &str) -> (&str, Option<u32>) {
    if let Some(head) = entry.strip_suffix(')') {
        if let Some((name, max)) = head.rsplit_once(" (maxSdk ") {
            if let Ok(v) = max.parse() {
                return (name, Some(v));
            }
        }
    }
    (entry, None)
}

/// 资源语言代码拆成的三段（`ResTable::locales` 的格式：`zh-HK`、`sr-Latn`、`es-419`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleParts {
    /// 小写语言代码，旧代码已换成现行代码（`iw` → `he`）。
    pub language: String,
    pub script: Option<String>,
    pub region: Option<String>,
}

/// 资源表里表示「无语言限定」的占位（与 aapt2 `locales: '--_--'` 一致）。
pub const DEFAULT_LOCALE: &str = "--_--";

impl LocaleParts {
    /// 拆解语言代码；默认占位与空串返回 `None`。
    pub fn parse(code: &str) -> Option<Self> {
        if code.is_empty() || code == DEFAULT_LOCALE {
            return None;
        }
        let mut parts = code.split(['-', '_']);
        let language = canonical_language(&parts.next()?.to_ascii_lowercase()).to_string();
        let (mut script, mut region) = (None, None);
        for p in parts {
            if p.len() == 4 && p.chars().all(|c| c.is_ascii_alphabetic()) {
                let mut s = p.to_ascii_lowercase();
                s[..1].make_ascii_uppercase();
                script = Some(s);
            } else if p.starts_with('r') && p.len() == 3 {
                // Android 资源目录写法 `zh-rHK`
                region = Some(p[1..].to_ascii_uppercase());
            } else if p.len() == 2 || (p.len() == 3 && p.chars().all(|c| c.is_ascii_digit())) {
                region = Some(p.to_ascii_uppercase());
            }
            // 其余（`valencia` 这类变体子标签）不影响名称，忽略
        }
        Some(Self {
            language,
            script,
            region,
        })
    }
}

/// Java `Locale` 沿用至今的旧代码 → 现行 ISO 639 代码。
fn canonical_language(lang: &str) -> &str {
    match lang {
        "iw" => "he",
        "in" => "id",
        "ji" => "yi",
        other => other,
    }
}

/// 语言对自己的称呼（`ja` → `日本語`）；ICU 不认识的代码返回 `None`。
pub fn autonym(language: &str) -> Option<&'static str> {
    generated::AUTONYMS
        .binary_search_by(|(c, _)| (*c).cmp(language))
        .ok()
        .map(|i| generated::AUTONYMS[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_for_binary_search() {
        assert!(generated::PERMISSIONS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(generated::AUTONYMS.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn framework_permissions_have_levels() {
        let camera = permission_protection("android.permission.CAMERA").unwrap();
        assert_eq!(camera.base(), ProtectionBase::Dangerous);
        let internet = permission_protection("android.permission.INTERNET").unwrap();
        assert_eq!(internet.base(), ProtectionBase::Normal);
        let alert = permission_protection("android.permission.SYSTEM_ALERT_WINDOW").unwrap();
        assert_eq!(alert.base(), ProtectionBase::Signature);
        assert!(alert.flags().contains(&"appop"));
        assert!(permission_protection("com.example.permission.C2D_MESSAGE").is_none());
    }

    /// 高位标志按 R.attr#protectionLevel 的真实取值对齐（曾漏掉 wellbeing / documenter 两位而整体错位）。
    #[test]
    fn high_protection_flags_are_aligned() {
        let flags = |n: &str| permission_protection(n).unwrap().flags();
        assert!(flags("android.permission.START_TASKS_FROM_RECENTS").contains(&"recents"));
        assert!(flags("android.permission.PACKAGE_USAGE_STATS").contains(&"retailDemo"));
        assert!(flags("android.permission.MANAGE_ROLE_HOLDERS").contains(&"module"));
        let shortcuts = flags("android.permission.ACCESS_SHORTCUTS");
        assert!(shortcuts.contains(&"role") && !shortcuts.contains(&"knownSigner"));
    }

    #[test]
    fn risk_reserves_high_for_private_data() {
        let r = |n: &str| permission_risk(&format!("android.permission.{n}"));
        assert_eq!(r("READ_SMS"), PermissionRisk::High);
        assert_eq!(r("ACCESS_BACKGROUND_LOCATION"), PermissionRisk::High);
        assert_eq!(r("MANAGE_EXTERNAL_STORAGE"), PermissionRisk::High);
        for n in [
            "WRITE_EXTERNAL_STORAGE",
            "POST_NOTIFICATIONS",
            "RECORD_AUDIO",
            "CAMERA",
        ] {
            assert_eq!(r(n), PermissionRisk::Sensitive, "{n}");
        }
        assert_eq!(r("SYSTEM_ALERT_WINDOW"), PermissionRisk::Sensitive);
        assert_eq!(r("QUERY_ALL_PACKAGES"), PermissionRisk::Sensitive);
        assert_eq!(r("WRITE_CONTACTS"), PermissionRisk::Sensitive);
        assert_eq!(r("INTERNET"), PermissionRisk::Normal);
        // normal | appop：安装即授予
        assert_eq!(r("USE_FULL_SCREEN_INTENT"), PermissionRisk::Normal);
        // signature | privileged | appop：普通应用拿不到
        assert_eq!(r("LOADER_USAGE_STATS"), PermissionRisk::System);
        assert_eq!(r("INSTALL_PACKAGES"), PermissionRisk::System);
        assert_eq!(
            permission_risk("com.example.permission.C2D_MESSAGE"),
            PermissionRisk::Unknown
        );
    }

    #[test]
    fn risk_order_puts_high_first() {
        use PermissionRisk::*;
        let mut v = vec![Unknown, Normal, High, System, Sensitive];
        v.sort();
        assert_eq!(v, vec![High, Sensitive, Normal, System, Unknown]);
    }

    #[test]
    fn high_risk_names_are_framework_permissions() {
        for n in HIGH_RISK.iter().chain(SENSITIVE_SPECIAL) {
            let full = format!("android.permission.{n}");
            assert!(
                permission_protection(&full).is_some(),
                "{full} 不在框架表里"
            );
        }
    }

    #[test]
    fn permission_entry_splits_max_sdk() {
        assert_eq!(
            split_permission_entry("android.permission.ACCESS_FINE_LOCATION (maxSdk 32)"),
            ("android.permission.ACCESS_FINE_LOCATION", Some(32))
        );
        assert_eq!(
            split_permission_entry("android.permission.INTERNET"),
            ("android.permission.INTERNET", None)
        );
    }

    #[test]
    fn locale_codes_split_into_parts() {
        let p = |c: &str| LocaleParts::parse(c).unwrap();
        assert_eq!(p("zh-HK").region.as_deref(), Some("HK"));
        assert_eq!(p("zh-rTW").region.as_deref(), Some("TW"));
        let sr = p("sr-Latn");
        assert_eq!(
            (sr.language.as_str(), sr.script.as_deref()),
            ("sr", Some("Latn"))
        );
        assert_eq!(p("es-419").region.as_deref(), Some("419"));
        assert_eq!(p("iw").language, "he");
        assert_eq!(p("in").language, "id");
        assert_eq!(p("ca-ES-valencia").region.as_deref(), Some("ES"));
        assert!(LocaleParts::parse(DEFAULT_LOCALE).is_none());
    }

    #[test]
    fn autonyms_come_from_icu() {
        assert_eq!(autonym("ja"), Some("日本語"));
        assert_eq!(autonym("en"), Some("English"));
        assert_eq!(autonym("qq"), None);
    }
}
