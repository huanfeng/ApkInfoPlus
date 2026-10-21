//! 命令行入口：开发期对照 aapt2 用。
//!
//! ```text
//! apkinfo <file.apk>                 # aapt2 dump badging 风格摘要
//! apkinfo xml <file.apk> [entry]     # 解码二进制 XML（默认 AndroidManifest.xml）
//! apkinfo res <file.apk>             # 列出资源表全部条目
//! ```

use std::fmt::Write as _;
use std::process::ExitCode;

use apkinfo_core::apk::ApkReader;
use apkinfo_core::axml;
use apkinfo_core::model::ApkMeta;

const USAGE: &str =
    "usage:\n  apkinfo <file.apk>\n  apkinfo xml <file.apk> [entry]\n  apkinfo res <file.apk>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["xml", apk] => cmd_xml(apk, "AndroidManifest.xml"),
        ["xml", apk, entry] => cmd_xml(apk, entry),
        ["res", apk] => cmd_res(apk),
        [apk] if *apk != "xml" && *apk != "res" => cmd_badging(apk),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn open(path: &str) -> Result<ApkReader, String> {
    ApkReader::open(path).map_err(|e| format!("cannot open {path}: {e}"))
}

fn cmd_badging(path: &str) -> Result<String, String> {
    let mut reader = open(path)?;
    let meta = reader.parse().ok_or("no parsable AndroidManifest.xml")?;
    Ok(badging_text(&meta))
}

fn cmd_xml(path: &str, entry: &str) -> Result<String, String> {
    let reader = open(path)?;
    let bytes = reader
        .read_file(entry)
        .ok_or_else(|| format!("entry not found: {entry}"))?;
    if !axml::is_binary_xml(&bytes) {
        return Err(format!("{entry} is not a binary xml"));
    }
    axml::decompress_to_text(&bytes).map_err(|e| e.to_string())
}

fn cmd_res(path: &str) -> Result<String, String> {
    let mut reader = open(path)?;
    reader.parse();
    if reader.resource_table().is_none() {
        return Err("resources.arsc not available".into());
    }
    let mut out = String::new();
    for info in reader.resources().values() {
        let refs: Vec<String> = info
            .references
            .iter()
            .map(|r| format!("0x{r:08x}"))
            .collect();
        let _ = writeln!(
            out,
            "{} {} -> {} | {} | {}",
            info.id_hex(),
            info.name(),
            info.file_paths.join(", "),
            info.colors.join(", "),
            refs.join(", ")
        );
    }
    Ok(out)
}

fn opt<T: ToString>(v: &Option<T>) -> String {
    v.as_ref().map(T::to_string).unwrap_or_default()
}

fn quoted<T: ToString>(items: &[T]) -> String {
    items
        .iter()
        .map(|i| format!("'{}'", i.to_string()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 仿 Dart `_generateTextInfo` 的 aapt2 `dump badging` 风格文本。
fn badging_text(meta: &ApkMeta) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "package: name='{}' versionCode='{}' versionName='{}' compileSdkVersion='{}' compileSdkVersionCodename='{}'",
        opt(&meta.package_name),
        opt(&meta.version_code),
        opt(&meta.version_name),
        opt(&meta.compile_sdk_version),
        opt(&meta.compile_sdk_version_codename),
    );
    if let Some(v) = meta.min_sdk_version {
        let _ = writeln!(s, "sdkVersion:'{v}'");
    }
    if let Some(v) = meta.target_sdk_version {
        let _ = writeln!(s, "targetSdkVersion:'{v}'");
    }
    if let Some(l) = &meta.label {
        let _ = writeln!(s, "application-label:'{l}'");
    }
    for (loc, l) in &meta.labels {
        let _ = writeln!(s, "application-label-{loc}:'{l}'");
    }
    for (dpi, p) in &meta.icon_paths {
        let _ = writeln!(s, "application-icon-{dpi}:'{p}'");
    }
    let _ = writeln!(
        s,
        "application: label='{}' icon='{}'",
        opt(&meta.label),
        opt(&meta.application_icon)
    );
    for a in &meta.launchable_activities {
        let _ = writeln!(
            s,
            "launchable-activity: name='{}' label='{}' icon='{}'",
            opt(&a.name),
            opt(&a.label),
            opt(&a.icon)
        );
    }
    for p in &meta.permissions {
        let _ = writeln!(s, "uses-permission: name='{p}'");
    }
    for f in &meta.features {
        let _ = writeln!(s, "uses-feature: name='{f}'");
    }
    for f in &meta.features_not_required {
        let _ = writeln!(s, "uses-feature-not-required: name='{f}'");
    }
    if !meta.screen_sizes.is_empty() {
        let _ = writeln!(s, "supports-screens: {}", quoted(&meta.screen_sizes));
    }
    let _ = writeln!(s, "locales: '--_--' {}", quoted(&meta.locales));
    if !meta.densities.is_empty() {
        let _ = writeln!(s, "densities: {}", quoted(&meta.densities));
    }
    if !meta.native_codes.is_empty() {
        let _ = writeln!(s, "native-code: {}", quoted(&meta.native_codes));
    }
    s
}
