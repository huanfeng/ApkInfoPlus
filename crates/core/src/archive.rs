//! 归档包（XAPK / APKM / APKS / ZIP）：读 manifest.json，定位 base APK，列出分包与 OBB。
//! 移植自 Dart 版 `xapk_info.dart` 与 `apk_info.dart` 的归档分支，但 base APK 用内置引擎解析。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::info::{
    is_abi_token, is_base_split, is_language_token, native_libs_from_entries, split_stem,
    ArchiveKind, NativeLib, ObbEntry, SplitEntry, SplitType,
};
use crate::model::ApkMeta;

/// manifest.json / info.json 里我们关心的字段（全部可选、类型宽容）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArchiveManifest {
    pub package_name: Option<String>,
    pub name: Option<String>,
    pub version_code: Option<i64>,
    pub version_name: Option<String>,
    pub min_sdk: Option<i64>,
    pub target_sdk: Option<i64>,
    pub total_size: Option<u64>,
    pub permissions: Vec<String>,
    pub split_configs: Vec<String>,
    pub split_apks: Vec<String>,
    pub icon: Option<String>,
    pub locales_name: BTreeMap<String, String>,
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn as_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 列表字段：JSON 数组或逗号分隔字符串。
fn as_string_list(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(as_string).collect(),
        Value::String(s) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn parse_manifest_json(text: &str) -> Option<ArchiveManifest> {
    let v: Value = serde_json::from_str(text).ok()?;
    let obj = v.as_object()?;
    let get = |keys: &[&str]| keys.iter().find_map(|k| obj.get(*k));
    let mut m = ArchiveManifest {
        package_name: get(&["package_name", "packageName"]).and_then(as_string),
        name: get(&["name", "app_name", "apk_name"]).and_then(as_string),
        version_code: get(&["version_code", "versionCode"]).and_then(as_i64),
        version_name: get(&["version_name", "versionName"]).and_then(as_string),
        min_sdk: get(&["min_sdk_version", "minSdkVersion"]).and_then(as_i64),
        target_sdk: get(&["target_sdk_version", "targetSdkVersion"]).and_then(as_i64),
        total_size: get(&["total_size", "totalSize"])
            .and_then(as_i64)
            .filter(|x| *x > 0)
            .map(|x| x as u64),
        permissions: get(&["permissions"])
            .map(as_string_list)
            .unwrap_or_default(),
        split_configs: get(&["split_configs"])
            .map(as_string_list)
            .unwrap_or_default(),
        split_apks: Vec::new(),
        icon: get(&["icon"]).and_then(as_string),
        locales_name: BTreeMap::new(),
    };
    if let Some(Value::Object(names)) = get(&["locales_name"]) {
        for (k, v) in names {
            if let Some(s) = as_string(v) {
                m.locales_name.insert(k.clone(), s);
            }
        }
    }
    if let Some(Value::Array(list)) = get(&["split_apks", "apks", "splits"]) {
        for item in list {
            match item {
                Value::String(s) if !s.is_empty() => m.split_apks.push(s.clone()),
                Value::Object(o) => {
                    let file = ["file", "path", "apk", "name"]
                        .iter()
                        .find_map(|k| o.get(*k))
                        .and_then(as_string);
                    if let Some(f) = file {
                        m.split_apks.push(f);
                    }
                }
                _ => {}
            }
        }
    }
    Some(m)
}

/// 挑 base APK：名字叫 base.apk → 第一个不含 `config.` 的 → 第一个。
pub fn find_base_apk(apks: &[String]) -> Option<String> {
    apks.iter()
        .find(|a| {
            let b = a.rsplit(['/', '\\']).next().unwrap_or(a).to_lowercase();
            b == "base.apk" || is_base_split(split_stem(&b))
        })
        .or_else(|| apks.iter().find(|a| !a.to_lowercase().contains("config.")))
        .or_else(|| apks.first())
        .cloned()
}

/// 从分包名推断 locale 与 ABI（aapt 对 split 包解析不到这些）。
pub fn infer_from_splits(splits: &[String]) -> (Vec<String>, Vec<String>) {
    let mut locales = Vec::new();
    let mut abis = Vec::new();
    for s in splits {
        let stem = split_stem(s).to_lowercase();
        let Some(suffix) = stem
            .strip_prefix("split_config.")
            .or_else(|| stem.strip_prefix("config."))
        else {
            continue;
        };
        if is_abi_token(suffix) {
            let abi = suffix.replace('_', "-");
            if !abis.contains(&abi) {
                abis.push(abi);
            }
        } else if suffix.ends_with("dpi") {
            continue;
        } else if is_language_token(suffix) && !locales.contains(&suffix.to_string()) {
            locales.push(suffix.to_string());
        }
    }
    (locales, abis)
}

/// 打开归档包后得到的原料。
pub struct ArchiveContents {
    pub manifest: Option<ArchiveManifest>,
    pub apks: Vec<(String, u64)>,
    pub obbs: Vec<(String, u64)>,
    pub base_entry: Option<String>,
    pub base_bytes: Option<Vec<u8>>,
    /// manifest.icon 或 icon.png 的字节。
    pub icon_bytes: Option<Vec<u8>>,
    /// ABI 分包里的原生库（base APK 自己的由 `ApkReader::native_libs` 给出）。
    pub split_native_libs: Vec<NativeLib>,
    /// zip 根目录含 AndroidManifest.xml：其实是改了扩展名的 APK。
    pub is_plain_apk: bool,
}

pub fn read_archive(path: &Path) -> Result<ArchiveContents, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut zip =
        crate::zip::open_archive(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let mut apks = Vec::new();
    let mut obbs = Vec::new();
    let mut is_plain_apk = false;
    let mut names = Vec::new();
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index_raw(i) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let Ok(name) = entry.name() else { continue };
        let name = name.into_owned();
        let lower = name.to_lowercase();
        if lower == "androidmanifest.xml" {
            is_plain_apk = true;
        } else if lower.ends_with(".apk") {
            apks.push((name.clone(), entry.size()));
        } else if lower.ends_with(".obb") {
            obbs.push((name.clone(), entry.size()));
        }
        names.push(name);
    }
    if is_plain_apk {
        return Ok(ArchiveContents {
            manifest: None,
            apks,
            obbs,
            base_entry: None,
            base_bytes: None,
            icon_bytes: None,
            split_native_libs: Vec::new(),
            is_plain_apk: true,
        });
    }

    // 每类条目各有上限：manifest 与图标只会是几 MB，base.apk 放到 2 GiB（再大也装不进内存，
    // 那就按「找不到 base」处理，而不是被 zip bomb 吃光内存）。
    let mut read = |name: &str, limit: u64| -> Option<Vec<u8>> {
        let mut f = zip.by_name(name).ok()?;
        if f.size() > limit {
            return None;
        }
        let mut buf = Vec::with_capacity(f.size().min(64 * 1024 * 1024) as usize);
        f.by_ref().take(limit + 1).read_to_end(&mut buf).ok()?;
        (buf.len() as u64 <= limit).then_some(buf)
    };
    const SMALL: u64 = 16 * 1024 * 1024;
    const BASE: u64 = 2 * 1024 * 1024 * 1024;
    let manifest = ["manifest.json", "info.json"]
        .iter()
        .find_map(|n| read(n, SMALL))
        .and_then(|b| parse_manifest_json(&String::from_utf8_lossy(&b)));

    let apk_names: Vec<String> = apks.iter().map(|(n, _)| n.clone()).collect();
    let base_entry = find_base_apk(&apk_names);
    let base_bytes = base_entry.as_deref().and_then(|n| read(n, BASE));

    let mut icon_bytes = None;
    if let Some(m) = &manifest {
        if let Some(icon) = &m.icon {
            icon_bytes = read(icon.trim_start_matches('/'), SMALL);
        }
    }
    if icon_bytes.is_none() {
        icon_bytes = read("icon.png", SMALL);
    }

    // 原生库通常在 ABI 分包里：逐个读进内存列中央目录，用完即丢。
    let mut split_native_libs = Vec::new();
    for name in &apk_names {
        if Some(name) == base_entry.as_ref() || SplitType::from_split_name(name) != SplitType::Abi {
            continue;
        }
        let Some(bytes) = read(name, BASE) else {
            continue;
        };
        match crate::zip::Archive::from_bytes(bytes) {
            Ok(mut split) => {
                split_native_libs.extend(native_libs_from_entries(split.file_sizes(), Some(name)))
            }
            Err(e) => log::warn!("split {name} unreadable: {e}"),
        }
    }

    Ok(ArchiveContents {
        manifest,
        apks,
        obbs,
        base_entry,
        base_bytes,
        icon_bytes,
        split_native_libs,
        is_plain_apk: false,
    })
}

/// 把 manifest.json 的字段覆盖到 base APK 的元信息上（只覆盖有效值）。
pub fn apply_manifest(meta: &mut ApkMeta, m: &ArchiveManifest) {
    if let Some(p) = &m.package_name {
        meta.package_name = Some(p.clone());
    }
    if let Some(v) = m.version_code.filter(|v| *v > 0) {
        meta.version_code = Some(v);
    }
    if let Some(v) = &m.version_name {
        meta.version_name = Some(v.clone());
    }
    if let Some(v) = m.min_sdk.filter(|v| *v > 0) {
        meta.min_sdk_version = Some(v);
    }
    if let Some(v) = m.target_sdk.filter(|v| *v > 0) {
        meta.target_sdk_version = Some(v);
    }
    if meta.label.is_none() {
        meta.label = m.name.clone();
    }
    if meta.permissions.is_empty() {
        meta.permissions = m.permissions.clone();
    }
    for (k, v) in &m.locales_name {
        meta.labels.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

pub fn build_splits(apks: &[(String, u64)]) -> Vec<SplitEntry> {
    let mut v: Vec<SplitEntry> = apks
        .iter()
        .map(|(p, size)| SplitEntry {
            path: p.clone(),
            size: *size,
            kind: SplitType::from_split_name(p),
        })
        .collect();
    // base 在前，其余按类型再按名字。
    v.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.path.cmp(&b.path)));
    v
}

pub fn build_obbs(obbs: &[(String, u64)]) -> Vec<ObbEntry> {
    obbs.iter()
        .map(|(p, size)| ObbEntry {
            path: p.clone(),
            size: *size,
        })
        .collect()
}

/// 归档包类型最终判定：`.zip` 里若没有 APK 条目则无法解析。
pub fn effective_kind(
    kind: ArchiveKind,
    contents: &ArchiveContents,
) -> Result<ArchiveKind, String> {
    if contents.is_plain_apk {
        return Ok(ArchiveKind::Apk);
    }
    if contents.apks.is_empty() {
        return Err("archive contains no .apk entries".into());
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_json_tolerant() {
        let text = r#"{
            "xapk_version": "2", "package_name": "com.x", "name": "X App",
            "version_code": "123", "version_name": "1.2.3", "min_sdk_version": 21, "target_sdk_version": "34",
            "total_size": 1000, "permissions": "android.permission.A,android.permission.B",
            "split_configs": ["arm64_v8a", "xxhdpi"],
            "split_apks": [{"file": "base.apk", "id": "base"}, {"file": "config.arm64_v8a.apk", "id": "config.arm64_v8a"}, {"id": "nofile"}],
            "icon": "icon.png", "locales_name": {"zh-CN": "应用"}
        }"#;
        let m = parse_manifest_json(text).unwrap();
        assert_eq!(m.package_name.as_deref(), Some("com.x"));
        assert_eq!(m.version_code, Some(123));
        assert_eq!(m.target_sdk, Some(34));
        assert_eq!(m.permissions.len(), 2);
        assert_eq!(m.split_apks, vec!["base.apk", "config.arm64_v8a.apk"]);
        assert_eq!(
            m.locales_name.get("zh-CN").map(|s| s.as_str()),
            Some("应用")
        );
    }

    #[test]
    fn base_apk_selection() {
        let apks = vec!["config.en.apk".to_string(), "x/base.apk".to_string()];
        assert_eq!(find_base_apk(&apks).as_deref(), Some("x/base.apk"));
        let apks = vec!["config.en.apk".to_string(), "com.foo.apk".to_string()];
        assert_eq!(find_base_apk(&apks).as_deref(), Some("com.foo.apk"));
        let apks = vec!["config.en.apk".to_string()];
        assert_eq!(find_base_apk(&apks).as_deref(), Some("config.en.apk"));
    }

    #[test]
    fn infer_locales_and_abis() {
        let splits = vec![
            "base.apk".to_string(),
            "split_config.arm64_v8a.apk".to_string(),
            "config.xxhdpi.apk".to_string(),
            "config.en.apk".to_string(),
            "config.zh.apk".to_string(),
            "config.pt-rbr.apk".to_string(),
        ];
        let (locales, abis) = infer_from_splits(&splits);
        assert_eq!(locales, vec!["en", "zh", "pt-rbr"]);
        assert_eq!(abis, vec!["arm64-v8a"]);
    }
}
