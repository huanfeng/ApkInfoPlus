//! 运行时更新规则：拉上游清单，有更新版本时下载 portable-v5.zip，校验后原样存到本地。
//! 流程与 `scripts/sync-libchecker-rules.py` 一致（只是不重新打包）。

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Deserialize;
use sha2::Digest;

use super::rules::{RuleSet, RulesMeta};

/// 上游发布根目录；清单里的 artifact 路径相对它解析。
pub const RULES_ROOT: &str =
    "https://raw.githubusercontent.com/LibChecker/LibChecker-Rules/rules-data/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// 本地已是最新（附上游当前版本）。
    UpToDate(u32),
    /// 已下载新版并写入目标文件。
    Updated(u32),
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(flatten)]
    meta: RulesMeta,
    artifacts: Artifacts,
}

#[derive(Debug, Deserialize)]
struct Artifacts {
    portable: Artifact,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    path: String,
    sha256: String,
    size: u64,
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let m: Manifest = serde_json::from_slice(bytes).map_err(|e| format!("manifest: {e}"))?;
    m.meta.check_supported().map_err(|e| e.to_string())?;
    if m.artifacts.portable.path.contains("..") || m.artifacts.portable.path.starts_with('/') {
        return Err(format!(
            "unsafe artifact path: {}",
            m.artifacts.portable.path
        ));
    }
    Ok(m)
}

fn verify(bytes: &[u8], a: &Artifact) -> Result<(), String> {
    if bytes.len() as u64 != a.size {
        return Err(format!("size mismatch: {} != {}", bytes.len(), a.size));
    }
    let digest = hex::encode(sha2::Sha256::digest(bytes));
    if !digest.eq_ignore_ascii_case(&a.sha256) {
        return Err(format!("sha256 mismatch: {digest} != {}", a.sha256));
    }
    Ok(())
}

/// 检查并更新：上游 `dataVersion` 高于 `current` 时下载、校验、试加载，
/// 通过后先写临时文件再改名到 `dest`，中途失败不会留下半截文件。
pub fn update_rules(dest: &Path, current: u32) -> Result<UpdateOutcome, String> {
    update_from(RULES_ROOT, dest, current)
}

fn update_from(root: &str, dest: &Path, current: u32) -> Result<UpdateOutcome, String> {
    let never = Arc::new(AtomicBool::new(false));
    let manifest = parse_manifest(&crate::tools::fetch_bytes(
        &format!("{root}manifest.json"),
        &never,
        |_| {},
    )?)?;
    let version = manifest.meta.data_version;
    if version <= current {
        return Ok(UpdateOutcome::UpToDate(version));
    }
    let art = &manifest.artifacts.portable;
    let bytes = crate::tools::fetch_bytes(&format!("{root}{}", art.path), &never, |_| {})?;
    verify(&bytes, art)?;
    let loaded = RuleSet::from_zip(bytes.clone()).map_err(|e| e.to_string())?;
    if loaded.meta().data_version != version {
        return Err(format!(
            "dataVersion mismatch: package {} != manifest {version}",
            loaded.meta().data_version
        ));
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("zip.part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(UpdateOutcome::Updated(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(
        version: u32,
        schema: u32,
        minimum: u32,
        path: &str,
        sha: &str,
        size: u64,
    ) -> String {
        format!(
            r#"{{"schemaVersion":{schema},"dataVersion":{version},"ruleCount":1,"minimumReader":{{"android":5,"portable":{minimum}}},
            "artifacts":{{"portable":{{"path":"{path}","sha256":"{sha}","size":{size},"schemaVersion":5,"minimumReaderVersion":5}}}}}}"#
        )
    }

    #[test]
    fn manifest_checks_schema_reader_and_path() {
        let ok = parse_manifest(
            manifest_json(48, 5, 5, "releases/48/portable-v5.zip", "ab", 1).as_bytes(),
        )
        .unwrap();
        assert_eq!(ok.meta.data_version, 48);
        assert_eq!(ok.artifacts.portable.size, 1);
        assert!(parse_manifest(manifest_json(48, 6, 5, "a.zip", "ab", 1).as_bytes()).is_err());
        assert!(parse_manifest(manifest_json(48, 5, 6, "a.zip", "ab", 1).as_bytes()).is_err());
        assert!(parse_manifest(manifest_json(48, 5, 5, "../x.zip", "ab", 1).as_bytes()).is_err());
    }

    #[test]
    fn verify_checks_size_and_digest() {
        let data = b"hello";
        let sha = hex::encode(sha2::Sha256::digest(data));
        let a = |sha: &str, size| Artifact {
            path: String::new(),
            sha256: sha.into(),
            size,
        };
        assert!(verify(data, &a(&sha, 5)).is_ok());
        assert!(verify(data, &a(&sha.to_uppercase(), 5)).is_ok());
        assert!(verify(data, &a(&sha, 4)).is_err());
        assert!(verify(data, &a("00", 5)).is_err());
    }

    /// 用本地目录模拟上游：内嵌规则包当作 portable 包发布。
    #[test]
    fn updates_from_local_root_only_when_newer() {
        let Some(bundled) = crate::libchecker::bundled() else {
            return;
        };
        let version = bundled.meta().data_version;
        let pkg = include_bytes!(concat!(env!("OUT_DIR"), "/libchecker-rules.zip"));
        let dir = std::env::temp_dir().join(format!("apkinfo-rules-update-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("releases")).unwrap();
        std::fs::write(dir.join("releases/p.zip"), pkg).unwrap();
        let sha = hex::encode(sha2::Sha256::digest(pkg));
        std::fs::write(
            dir.join("manifest.json"),
            manifest_json(version, 5, 5, "releases/p.zip", &sha, pkg.len() as u64),
        )
        .unwrap();
        let root = format!("{}/", dir.display());
        let dest = dir.join("out").join("portable-v5.zip");

        assert_eq!(
            update_from(&root, &dest, version).unwrap(),
            UpdateOutcome::UpToDate(version)
        );
        assert!(!dest.exists());
        assert_eq!(
            update_from(&root, &dest, version - 1).unwrap(),
            UpdateOutcome::Updated(version)
        );
        assert_eq!(std::fs::read(&dest).unwrap(), pkg);

        // 校验和不符时拒绝，不碰已有文件
        std::fs::write(
            dir.join("manifest.json"),
            manifest_json(version + 1, 5, 5, "releases/p.zip", "00", pkg.len() as u64),
        )
        .unwrap();
        assert!(update_from(&root, &dest, version).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), pkg);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 联网冒烟：`cargo test -p apkinfo-core -- --ignored libchecker_update_online`
    #[test]
    #[ignore]
    fn libchecker_update_online() {
        let dest =
            std::env::temp_dir().join(format!("apkinfo-rules-online-{}.zip", std::process::id()));
        let got = update_rules(&dest, 0).unwrap();
        assert!(
            matches!(got, UpdateOutcome::Updated(v) if v >= 48),
            "{got:?}"
        );
        assert!(RuleSet::from_zip(std::fs::read(&dest).unwrap()).is_ok());
        let _ = std::fs::remove_file(&dest);
    }
}
