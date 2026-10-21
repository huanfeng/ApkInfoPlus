//! ZIP 读取封装：APK 本身就是 ZIP，归档包（XAPK 等）也是。
//! 提供按名查找（含大小写不敏感回退）、读取到内存、流式解压到文件。

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::{Component, Path};

#[derive(Debug, thiserror::Error)]
pub enum ZipError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("entry not found: {0}")]
    NotFound(String),
    #[error("unsafe path rejected: {0}")]
    UnsafePath(String),
    #[error("entry too large (> {limit} bytes): {name}")]
    TooLarge { name: String, limit: u64 },
}

/// 读进内存的单个条目上限。APK 内最大的条目是 resources.arsc / classes.dex，
/// 几十 MB 量级；归档包里的 base.apk 另走 `read_limited`。
pub const MAX_READ_BYTES: u64 = 512 * 1024 * 1024;

/// 可装箱的读取源；`Send` 让上层可以把读取器放进 `Mutex` 跨线程共享。
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// 读入内存时预分配容量的上限：条目头里的 size 来自用户文件，不可盲信。
const MAX_PREALLOC: u64 = 64 * 1024 * 1024;

pub struct Archive<R: Read + Seek> {
    inner: zip::ZipArchive<R>,
}

impl Archive<BufReader<File>> {
    /// 打开磁盘上的 ZIP 文件。
    pub fn open_file(path: impl AsRef<Path>) -> Result<Self, ZipError> {
        let file = File::open(path)?;
        Self::new(BufReader::new(file))
    }
}

impl Archive<Cursor<Vec<u8>>> {
    /// 从内存字节打开（归档包内嵌的 base.apk 等）。
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, ZipError> {
        Self::new(Cursor::new(data))
    }
}

impl<R: Read + Seek> Archive<R> {
    pub fn new(reader: R) -> Result<Self, ZipError> {
        Ok(Self {
            inner: zip::ZipArchive::new(reader)?,
        })
    }

    pub fn inner_mut(&mut self) -> &mut zip::ZipArchive<R> {
        &mut self.inner
    }

    /// 全部文件条目名（不含目录条目），保持中央目录顺序。
    pub fn file_names(&self) -> Vec<String> {
        self.inner
            .file_names()
            .filter_map(|n| n.ok())
            .filter(|n| !n.ends_with('/') && !n.ends_with('\\'))
            .map(|n| n.into_owned())
            .collect()
    }

    /// 解析出实际存在的条目名：精确 → 去前导 `/` → 大小写不敏感。
    /// 混淆或手工打包的 APK 偶尔出现大小写不一致或带前导斜杠的条目名。
    fn resolve_name(&self, name: &str) -> Option<String> {
        if self.inner.index_for_name(name).is_some() {
            return Some(name.to_string());
        }
        let trimmed = name.trim_start_matches('/');
        if trimmed != name && self.inner.index_for_name(trimmed).is_some() {
            return Some(trimmed.to_string());
        }
        let lower = trimmed.to_lowercase();
        self.file_names()
            .into_iter()
            .find(|n| n.trim_start_matches('/').to_lowercase() == lower)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.resolve_name(name).is_some()
    }

    /// 条目解压后的大小。
    pub fn entry_size(&mut self, name: &str) -> Option<u64> {
        let real = self.resolve_name(name)?;
        self.inner.by_name(&real).ok().map(|f| f.size())
    }

    /// 读取条目全部内容到内存（上限 [`MAX_READ_BYTES`]，防 zip bomb）。
    pub fn read(&mut self, name: &str) -> Result<Vec<u8>, ZipError> {
        self.read_limited(name, MAX_READ_BYTES)
    }

    /// 同 [`read`](Self::read)，但由调用方给上限。解压后超过上限返回 `TooLarge`，
    /// 而不是按条目头声明的大小盲信——声明值也可以造假。
    pub fn read_limited(&mut self, name: &str, limit: u64) -> Result<Vec<u8>, ZipError> {
        let real = self
            .resolve_name(name)
            .ok_or_else(|| ZipError::NotFound(name.to_string()))?;
        let mut file = self.inner.by_name(&real)?;
        if file.size() > limit {
            return Err(ZipError::TooLarge {
                name: name.to_string(),
                limit,
            });
        }
        let mut buf = Vec::with_capacity(file.size().min(MAX_PREALLOC) as usize);
        file.by_ref().take(limit + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > limit {
            return Err(ZipError::TooLarge {
                name: name.to_string(),
                limit,
            });
        }
        Ok(buf)
    }

    /// 流式解压条目到 `dest`（自动创建父目录）。
    /// 条目名或目标路径含 `..` 时拒绝，防止 zip-slip 写出预期目录之外。
    pub fn extract_to(&mut self, name: &str, dest: impl AsRef<Path>) -> Result<(), ZipError> {
        let dest = dest.as_ref();
        if has_parent_component(Path::new(name)) || has_parent_component(dest) {
            return Err(ZipError::UnsafePath(format!(
                "{name} -> {}",
                dest.display()
            )));
        }
        let real = self
            .resolve_name(name)
            .ok_or_else(|| ZipError::NotFound(name.to_string()))?;
        let mut file = self.inner.by_name(&real)?;
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut out = std::io::BufWriter::new(File::create(dest)?);
        std::io::copy(&mut file, &mut out)?;
        std::io::Write::flush(&mut out)?;
        Ok(())
    }
}

fn has_parent_component(p: &Path) -> bool {
    // 条目名可能用反斜杠分隔（Windows 打包工具），统一按两种分隔符检查
    p.components().any(|c| matches!(c, Component::ParentDir))
        || p.to_string_lossy()
            .split(['/', '\\'])
            .any(|seg| seg == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip() -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.add_directory("res/", opts).unwrap();
            w.start_file("res/a.txt", opts).unwrap();
            w.write_all(b"hello").unwrap();
            w.start_file("AndroidManifest.xml", opts).unwrap();
            w.write_all(b"mf").unwrap();
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn lists_files_and_reads_with_fallbacks() {
        let mut a = Archive::from_bytes(make_zip()).unwrap();
        assert_eq!(a.file_names(), vec!["res/a.txt", "AndroidManifest.xml"]);
        assert_eq!(a.read("res/a.txt").unwrap(), b"hello");
        assert_eq!(a.read("/res/a.txt").unwrap(), b"hello");
        assert_eq!(a.read("androidmanifest.XML").unwrap(), b"mf");
        assert!(a.contains("RES/A.TXT"));
        assert_eq!(a.entry_size("res/a.txt"), Some(5));
        assert!(matches!(a.read("nope"), Err(ZipError::NotFound(_))));
    }

    #[test]
    fn extract_rejects_parent_dir() {
        let mut a = Archive::from_bytes(make_zip()).unwrap();
        let dir = std::env::temp_dir().join(format!("apkinfo-zip-test-{}", std::process::id()));
        let bad = dir.join("..").join("x.txt");
        assert!(matches!(
            a.extract_to("res/a.txt", &bad),
            Err(ZipError::UnsafePath(_))
        ));
        let good = dir.join("sub").join("a.txt");
        a.extract_to("res/a.txt", &good).unwrap();
        assert_eq!(std::fs::read(&good).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
