//! 解析结果数据模型（与 Dart 版 `apk_parser/models.dart` 对应）。

use std::collections::BTreeMap;

/// Manifest + 资源表解析出的 APK 元信息。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApkMeta {
    pub package_name: Option<String>,
    pub version_code: Option<i64>,
    pub version_name: Option<String>,
    pub min_sdk_version: Option<i64>,
    pub target_sdk_version: Option<i64>,
    pub compile_sdk_version: Option<i64>,
    pub compile_sdk_version_codename: Option<String>,
    pub platform_build_version_name: Option<String>,
    pub platform_build_version_code: Option<i64>,

    /// 默认语言的应用名（已经过资源表解析；解析不到时保留 `@res/0x...` 原文）。
    pub label: Option<String>,
    /// locale → 应用名。
    pub labels: BTreeMap<String, String>,

    pub application_name: Option<String>,
    /// `<application android:icon>` 的原始值（通常是 `@res/0x7f...`）。
    pub application_icon: Option<String>,
    /// density → 图标文件路径（来自资源表的全部 density 变体）。
    pub icon_paths: BTreeMap<u32, String>,

    pub permissions: Vec<String>,
    pub features: Vec<String>,
    pub features_not_required: Vec<String>,
    pub screen_sizes: Vec<String>,
    pub native_codes: Vec<String>,
    pub locales: Vec<String>,
    pub densities: Vec<u32>,

    pub launchable_activities: Vec<ActivityInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityInfo {
    pub name: Option<String>,
    pub label: Option<String>,
    pub icon: Option<String>,
}

/// 资源表条目的聚合视图（按资源 ID 合并所有 config 变体），供图标渲染器解析引用。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceInfo {
    pub resource_id: u32,
    pub type_name: String,
    pub key_name: String,
    /// 字符串值（对 drawable/mipmap 即文件路径），去重保序。
    pub file_paths: Vec<String>,
    /// `#aarrggbb` 颜色值，去重保序。
    pub colors: Vec<String>,
    /// 引用到的其它资源 ID，去重保序。
    pub references: Vec<u32>,
}

impl ResourceInfo {
    /// `type/key`
    pub fn name(&self) -> String {
        format!("{}/{}", self.type_name, self.key_name)
    }
    /// `0x%08x`
    pub fn id_hex(&self) -> String {
        format!("0x{:08x}", self.resource_id)
    }
}
