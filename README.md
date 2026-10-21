# APK Info Plus

轻量跨平台的 Android 安装包信息与安装工具（Windows / macOS / Linux），Rust + [windui](https://github.com/huanfeng/wind-ui-rust) 实现，
由 Flutter 版 [ApkInfoTool](https://github.com/huanfeng/ApkInfoTool) 重构而来：功能保留，资源占用与原生感按桌面工具的标准重做。

## 功能

- **拖进来就看**：APK / XAPK / APKM / APKS（以及装着 APK 的 .zip）的包名、版本、SDK、权限、硬件特性、屏幕尺寸与密度、ABI、语言列表、启动 Activity
- **内置解析引擎**：纯 Rust 解析二进制 AndroidManifest.xml 与 resources.arsc，含混淆 APK 容错，不依赖 aapt2 / Java
- **归档包**：读取 manifest.json，列出分包（按基础 / 语言 / ABI / 密度分类）与 OBB，base APK 同样走内置引擎
- **图标**：位图 / VectorDrawable / adaptive-icon / layer-list / shape 全部渲染，可在多个候选间切换，导出 PNG / SVG / 原始文件
- **签名证书**：纯 Rust 提取 v1（PKCS#7）与 v2 / v3 / v3.1（APK Signing Block）证书，显示 DN、序列号、有效期、SHA-256 / SHA-1 / MD5 指纹、密钥算法与长度
- **哈希**：后台流式计算 MD5 / SHA1
- **adb 安装**：独立安装窗口，多设备、`install` / `install-multiple`、OBB 推送、`-r -d -t -g` 选项、分包勾选、进度日志与取消；概览头可先选好目标设备；可在设置里一键下载 platform-tools
- **多文件**：拖入 / 命令行 / 再次启动传入的多个文件同时打开，左侧侧栏列出全部（可搜索、单个或全部关闭），各文件记住自己停留的标签页
- **桌面化交互**：自绘标题栏 + 可拖侧栏 + 概览头卡片 + 带图标标签页，几乎所有字段都可选中复制（右键菜单），文件重命名（预设模板 + 校验），应用名多语言切换，文本信息（aapt2 badging 风格），完整快捷键，单实例（再次打开文件复用窗口）
- **外观**：浅色 / 深色 / 跟随系统，自定义强调色；界面语言 zh-CN / zh-TW / zh-HK / en / ja / ko

## 安装

从 [Releases](https://github.com/huanfeng/ApkInfoPlus/releases) 下载：

| 平台 | 文件 |
|---|---|
| Windows x64 | `ApkInfoPlus-<版本>-windows-x86_64.zip`（解压即用） |
| macOS（Apple Silicon / Intel 通用） | `ApkInfoPlus-<版本>-macos-universal.dmg` |
| Linux x64 | `ApkInfoPlus-<版本>-linux-x86_64.AppImage` 或 `.tar.gz` |

Windows 也可以用 [Scoop](https://scoop.sh)：

```powershell
scoop bucket add huanfeng https://github.com/huanfeng/scoop-bucket
scoop install apkinfoplus
```

macOS 版只做了 ad-hoc 签名，没有经过 Apple 公证：第一次打开时在 Finder 里右键「打开」，
或执行 `xattr -dr com.apple.quarantine "/Applications/APK Info Plus.app"`。

## 快捷键

| 键 | 作用 |
|---|---|
| Ctrl+O | 打开文件 |
| F5 | 重新解析 |
| Ctrl+I | 安装 |
| F2 | 重命名 |
| Ctrl+W | 关闭当前文件 |
| Ctrl+Tab / Ctrl+Shift+Tab | 切换文件 |
| Ctrl+, | 设置 |
| Esc | 关闭对话框 / 退出 |

## 构建

`windui` 默认用 crates.io 的发布版。要连同本地的 `wind-ui-rust` 源码一起改，在仓库根目录建
`.cargo/config.toml`（已在 .gitignore 里）：

```toml
[patch.crates-io]
windui = { path = "../wind-ui-rust" }   # 相对仓库根目录
```

Windows 可执行文件的图标由 `crates/app/build.rs` 在构建时嵌入，
需要 `rc.exe` / `llvm-rc` / `windres` 之一在 PATH 里，找不到时只是没有图标。

需要 Rust 1.85+（edition 2021，工作区使用 `std::sync::LazyLock` 等较新 API）。

```bash
cargo build --release -p apkinfoplus          # 桌面应用：target/release/apkinfoplus
cargo run -p apkinfoplus -- path/to/app.apk   # 直接打开文件
cargo run -p apkinfo-core --bin apkinfo -- path/to/app.apk   # 命令行查看解析结果（aapt2 badging 风格）
```

调试用参数（不写入配置）：`--lang zh-CN` 强制界面语言，`--theme dark|light` 强制主题，
`--screenshot out.png [--click X Y]` 离屏渲染一帧（windui 提供，用于视觉回归）。

Linux 运行期依赖：fontconfig（选字）；X11 或 Wayland（设 `WINDUI_BACKEND=wayland`）。无需 GTK。

## 仓库结构

```
crates/core   apkinfo-core：解析核心库（zip / axml / arsc / manifest / icon / signature / adb / installer / tools）+ apkinfo 命令行
crates/app    apkinfoplus：windui 桌面应用（ui / state / actions / config / i18n）
packaging/                                                 macOS Info.plist、Linux .desktop
scripts/                                                   版本号同步与各平台打包脚本（Release 工作流调用）
docs/superpowers/specs/2026-10-21-apkinfoplus-design.md   设计文档
```

## 发布

推送 `v*` 标签触发 `.github/workflows/release.yml`：三平台构建打包，创建 GitHub Release，
再通知 [scoop-bucket](https://github.com/huanfeng/scoop-bucket) 更新 manifest（需要仓库 Secret
`SCOOP_BUCKET_GITHUB_TOKEN`，未配置时跳过）。版本号以标签为准，由 `scripts/set-version.py` 写入
`Cargo.toml` 与 Windows 资源。

配置文件位于平台配置目录下的 `apkinfoplus/config.toml`（Linux `~/.config/apkinfoplus/`），
日志与下载的 platform-tools 位于数据目录 `apkinfoplus/`（Linux `~/.local/share/apkinfoplus/`）。

## 测试

```bash
cargo test --workspace                 # 单元测试（含译文一致性 lint）
cargo clippy --workspace --all-targets # 零警告
```

核心库里有几组 `#[ignore]` 的手工对照测试，通过环境变量指向本机文件运行：
`APKINFO_SIG_APK`（签名 vs apksigner）、`APKINFO_INSTALL_APK` + `APKINFO_INSTALL_DEV`（真机安装）、`APKINFO_DL_DIR`（联网下载）。

## 与 Flutter 版的差异

- 解析引擎只保留内置实现，不再提供 aapt2 切换；签名改为证书解析（不做完整性校验），不再需要 apksigner / Java。
- 长列表（权限、分包、签名）改为独立标签页，去掉了「列表最大行数」设置。
- 分包分类修正了 `config.en` 这类语言包被归为「其他」的问题；安装失败会显示 adb 给出的原因（含退出码为 0 但输出 `Failure` 的情况）。
- 重命名会拒绝非法字符与同名覆盖；导出文件名跟随当前选择的应用名语言。
- SVG 导出的 clip-path 真正生效。

## 许可证

MIT
