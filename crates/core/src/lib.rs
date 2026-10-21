//! apkinfo-core：零 UI 依赖的 Android 安装包解析库。
//!
//! 模块按解析流水线组织：`zip` 读包 → `axml` / `arsc` 解二进制格式 → `manifest` 产出
//! [`ApkMeta`] → `apk` 汇总。上层应用只消费 `info::ApkInfo`。

pub mod reader;

pub use reader::ByteReader;
