# APK Info Plus

轻量跨平台的 Android 安装包信息与安装工具（Windows / macOS / Linux），Rust + [windui](https://github.com/huanfeng/wind-ui-rust) 实现，
由 Flutter 版 [ApkInfoTool](https://github.com/huanfeng/ApkInfoTool) 重构而来。

- 拖进来就看：APK / XAPK / APKM / APKS 的包名、版本、SDK、权限、分包、OBB、签名证书、哈希
- 内置解析引擎，纯 Rust 解析二进制 XML 与资源表，不依赖 aapt2 / Java
- 图标预览与导出（PNG / SVG / 原始文件），支持自适应图标与矢量图标
- adb 一键安装（含分包与 OBB）

## 构建

```bash
cargo build --release -p apkinfoplus
cargo run -p apkinfoplus -- path/to/app.apk
cargo run -p apkinfo-core --bin apkinfo -- path/to/app.apk   # 命令行查看解析结果
```

工作区结构与设计见 `docs/superpowers/specs/2026-10-21-apkinfoplus-design.md`，阶段进度见 `IMPLEMENTATION_PLAN.md`。

## 许可证

MIT
