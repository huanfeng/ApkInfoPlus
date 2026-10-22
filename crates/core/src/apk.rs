//! ApkReader：打开 APK（文件或内存），解析 Manifest + 资源表，提供文件访问。
//! 移植自 Dart 版 `apk_reader.dart`。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

use crate::arsc::ResourceTable;
use crate::axml;
use crate::info::{native_libs_from_entries, NativeLib};
use crate::manifest;
use crate::model::{ApkMeta, ResourceInfo};
use crate::zip::{Archive, ReadSeek, ZipError};

const MANIFEST: &str = "AndroidManifest.xml";
const RESOURCES: &str = "resources.arsc";

#[derive(Debug, thiserror::Error)]
pub enum ApkError {
    #[error(transparent)]
    Zip(#[from] ZipError),
}

pub struct ApkReader {
    // ZipArchive 读取需要 &mut；放进 Mutex 让读取接口保持 &self 且可跨线程共享
    archive: Mutex<Archive<Box<dyn ReadSeek>>>,
    files: Vec<String>,
    table: Option<ResourceTable>,
}

impl ApkReader {
    /// 从文件路径打开。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ApkError> {
        let file = std::fs::File::open(path).map_err(ZipError::from)?;
        Self::from_reader(Box::new(std::io::BufReader::new(file)))
    }

    /// 从内存字节打开（归档包里的 base.apk 等）。
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, ApkError> {
        Self::from_reader(Box::new(std::io::Cursor::new(data)))
    }

    fn from_reader(reader: Box<dyn ReadSeek>) -> Result<Self, ApkError> {
        let archive = Archive::new(reader)?;
        let files = archive.file_names();
        Ok(Self {
            archive: Mutex::new(archive),
            files,
            table: None,
        })
    }

    /// 解析基础信息。无 AndroidManifest.xml 或其无法解码时返回 `None`；
    /// resources.arsc 解析失败不致命，只是缺少应用名 / 图标 / locale 等。
    pub fn parse(&mut self) -> Option<ApkMeta> {
        let manifest_bytes = self.read_file(MANIFEST)?;
        let doc = match axml::parse_document(&manifest_bytes) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("manifest parse failed: {e}");
                return None;
            }
        };
        let mut meta = manifest::parse_manifest(&doc);

        if let Some(arsc) = self.read_file(RESOURCES) {
            match ResourceTable::parse(&arsc) {
                Ok(table) => {
                    resolve_label(&table, &mut meta);
                    resolve_icon_paths(&table, &mut meta);
                    meta.locales = table.locales();
                    meta.densities = table.densities();
                    self.table = Some(table);
                }
                Err(e) => log::warn!("arsc parse failed: {e}"),
            }
        }

        meta.native_codes = scan_native_codes(&self.files);
        Some(meta)
    }

    /// `parse` 之后可用的资源表。
    pub fn resource_table(&self) -> Option<&ResourceTable> {
        self.table.as_ref()
    }

    /// 释放已解析的资源表。大包（如 Chrome，arsc 25MB）的表常驻要几十 MB，
    /// 调用方把需要的东西取走之后就该放掉。
    pub fn release_resource_table(&mut self) {
        self.table = None;
    }

    /// 全部资源条目（按 ID 聚合）；资源表不可用时为空。
    pub fn resources(&self) -> BTreeMap<u32, ResourceInfo> {
        self.table
            .as_ref()
            .map(ResourceTable::all_resources)
            .unwrap_or_default()
    }

    /// 同 [`resources`](Self::resources)，字符串值按 [`ResourceTable::resources_with`] 过滤。
    pub fn resources_with(&self, keep: impl Fn(&str, &str) -> bool) -> BTreeMap<u32, ResourceInfo> {
        self.table
            .as_ref()
            .map(|t| t.resources_with(keep))
            .unwrap_or_default()
    }

    /// 读取条目内容；不存在或解压失败返回 `None`。
    pub fn read_file(&self, name: &str) -> Option<Vec<u8>> {
        let mut archive = self.archive.lock().unwrap_or_else(|e| e.into_inner());
        match archive.read(name) {
            Ok(b) => Some(b),
            Err(ZipError::NotFound(_)) => None,
            Err(e) => {
                log::warn!("read {name} failed: {e}");
                None
            }
        }
    }

    /// `lib/<abi>/*.so` 条目清单。
    pub fn native_libs(&self) -> Vec<NativeLib> {
        let mut archive = self.archive.lock().unwrap_or_else(|e| e.into_inner());
        native_libs_from_entries(archive.file_sizes(), None)
    }

    /// 全部文件条目名（不含目录）。
    pub fn list_files(&self) -> Vec<String> {
        self.files.clone()
    }
}

/// 解析 `@res/0x...` 引用；格式不符返回 `None`。
fn parse_res_ref(value: &str) -> Option<u32> {
    u32::from_str_radix(value.strip_prefix("@res/0x")?, 16).ok()
}

/// 应用名：默认配置 + 每个 locale 的本地化值。
fn resolve_label(table: &ResourceTable, meta: &mut ApkMeta) {
    let Some(res_id) = meta.label.as_deref().and_then(parse_res_ref) else {
        return;
    };
    if let Some(s) = table.string_value(res_id, None) {
        meta.label = Some(s);
    }
    for locale in table.locales() {
        if let Some(s) = table.string_value(res_id, Some(&locale)) {
            meta.labels.insert(locale, s);
        }
    }
}

fn resolve_icon_paths(table: &ResourceTable, meta: &mut ApkMeta) {
    let Some(res_id) = meta.application_icon.as_deref().and_then(parse_res_ref) else {
        return;
    };
    for (density, path) in table.all_file_paths(res_id) {
        meta.icon_paths.insert(density, path);
    }
}

/// `lib/<abi>/...` 中的 ABI 名，去重排序。
fn scan_native_codes(files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter_map(|f| {
            let mut parts = f.split('/');
            (parts.next()? == "lib").then_some(())?;
            let abi = parts.next()?;
            parts.next()?;
            Some(abi.to_string())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_codes_are_deduped_and_sorted() {
        let files: Vec<String> = [
            "lib/x86/a.so",
            "lib/arm64-v8a/b.so",
            "lib/x86/c.so",
            "lib/readme",
            "libfoo/x/y",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(scan_native_codes(&files), vec!["arm64-v8a", "x86"]);
    }

    #[test]
    fn res_ref_parsing() {
        assert_eq!(parse_res_ref("@res/0x7f010000"), Some(0x7f010000));
        assert_eq!(parse_res_ref("App"), None);
        assert_eq!(parse_res_ref("@res/0xzz"), None);
    }

    #[test]
    fn apk_without_manifest_parses_to_none() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            use std::io::Write;
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file(
                "lib/arm64-v8a/a.so",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let mut r = ApkReader::from_bytes(buf.into_inner()).unwrap();
        assert_eq!(r.list_files(), vec!["lib/arm64-v8a/a.so"]);
        assert!(r.parse().is_none());
        assert!(r.read_file("lib/arm64-v8a/a.so").is_some());
        assert!(r.resources().is_empty());
    }
}
