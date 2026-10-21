# APK Info Plus 设计文档

> 日期：2026-10-21
> 状态：已采纳（由原 Flutter 版 ApkInfoTool 重构而来）

## 1. 意图

ApkInfoTool 是一个「拖进来就看」的 Android 安装包信息工具：解析 APK / XAPK / APKM / APKS，
展示包名、版本、SDK、权限、签名、哈希、分包与 OBB，渲染并导出图标，并能通过 adb 一键安装。

原版基于 Flutter。Flutter 在桌面端资源占用偏高、原生感弱，因此改用 Rust + windui
（本人维护的轻量跨平台桌面 GUI 库）重写，目标是：

- **功能等价或更优**：复刻原版全部实用功能，尤其是**完全不依赖外部工具的 APK 解析**。
- **桌面化的交互与布局**：窗口更宽、信息分区、标签页承载长列表、设置独立窗口、键盘快捷键完整。
- **三平台**：Windows / macOS / Linux 同一份代码。
- **轻量**：无运行时、无 GC，空闲零 CPU，二进制与内存占用显著低于 Flutter 版。

## 2. 目标 / 非目标 / 反目标

**目标**

1. 内置解析引擎（纯 Rust）解析 AndroidManifest.xml（二进制 XML）与 resources.arsc，
   兼容混淆 APK（原版的全部容错启发式照搬）。
2. XAPK / APKM / APKS / ZIP 归档包：读取 manifest.json / info.json，定位 base APK 并用**内置引擎**解析
   （原版这里依赖 aapt2，本版去掉该依赖），列出分包与 OBB。
3. 图标：收集候选（位图 / 矢量 XML / 自适应图标），渲染预览，导出 PNG / SVG / 原始文件；
   支持 VectorDrawable 的 path / group / clip-path / 渐变 / trimPath，adaptive-icon 的前景背景合成。
4. 签名信息：纯 Rust 解析 v1（META-INF PKCS#7）与 v2/v3（APK Signing Block）证书，
   展示签名方案、证书 DN、SHA-256 / SHA-1 / MD5 指纹、密钥算法与长度。不再依赖 apksigner（它需要 Java）。
5. 文件 MD5 / SHA1（后台线程，流式）。
6. adb 安装：设备列表、普通 APK `install`、分包 `install-multiple`、OBB 推送、
   高级选项（-t / -d / -r）、分包勾选、取消。
7. 文件重命名（预设 + 自由编辑 + 非法字符校验）、文本信息页（aapt2 badging 风格）、复制、在文件管理器中显示。
8. 设置：解析/显示选项、主题（亮 / 暗 / 跟随系统 + 强调色）、语言、adb 来源与路径、platform-tools 下载。
9. 多语言：zh-CN、en 首发；zh-TW、zh-HK、ja、ko 随后移植。
10. 命令行参数打开文件；单实例（第二次启动把文件转给已开实例）。

**非目标**

- 不再提供 aapt2 解析引擎切换（内置引擎是唯一引擎；aapt2 仅作为开发期对照工具存在于测试脚本中）。
- 不做 apksigner 的完整签名校验（摘要验证 / 证书链）；展示的是「证书信息」并明确标注。
- 不做 APK 编辑、重签名、反编译。
- 第一版不实现 macOS 文件关联的原生 `openFile` 回调（以命令行参数 + 单实例为主；macOS Finder 打开走 `open -a` 传参）。

**反目标（做出来也算失败）**

- 解析一个常见 APK 需要用户先安装任何东西。
- 窗口空闲时占 CPU、或内存明显高于 windui 同规模示例。
- 界面仍是手机 App 的单列竖排。

## 3. 约束

- GUI 只用 windui（crates.io `windui = "0.25"`），遵守其 API_GUIDE：状态走 `Signal<T>`，颜色走 `Role`，
  后台线程经 `App::channel` 回到 UI 线程，阻塞的原生对话框走 `ctx.request_*` / `defer_blocking`。
- 图片解码：windui 核心只内置 PNG + SVG；WebP / JPEG 通过 `image` crate 实现 `ImageDecoder` 注册。
- 矢量渲染复用 resvg（windui 同源，版本对齐 0.48 / tiny-skia 0.12）。
- 不引入异步运行时；后台任务用 `std::thread` + `Sender`。
- 提交规范沿用 windui 的 AGENTS.md：简体中文 conventional commit，不含 AI 元信息。

## 4. 架构

```
apkinfoplus/
├── crates/core   # apkinfo-core：零 UI 依赖的库 + `apkinfo` 命令行
│   ├── zip.rs           ZipArchive 封装（按名查找、大小写不敏感回退、流式解压）
│   ├── axml/            二进制 XML：字符串池、资源 ID 映射、元素树、混淆容错
│   ├── arsc/            资源表：StringPool / Package / TypeSpec / Type / Config（BCP-47 packed）
│   ├── manifest.rs      Manifest → ApkMeta（含启发式）
│   ├── apk.rs           ApkReader：open / parse / 文件列表 / native code 扫描
│   ├── archive.rs       XAPK/APKM/APKS/ZIP：manifest.json、base 定位、分包/OBB、locale/ABI 推断
│   ├── info.rs          ApkInfo（UI 直接消费的汇总模型）+ 文本信息生成 + SDK 版本表 + 格式化
│   ├── icon/            候选收集、VectorDrawable→SVG、adaptive 合成、位图解码、导出
│   ├── signature.rs     v1 / v2 / v3 证书提取与指纹
│   ├── hash.rs          流式 MD5 / SHA1
│   ├── adb.rs           设备列表、install / install-multiple / push，带取消
│   ├── installer.rs     归档包安装流程（解压选中分包 → install-multiple → OBB）
│   ├── tools.rs         adb 查找（系统 PATH / 下载目录 / 自定义）、platform-tools 下载
│   └── bin/apkinfo.rs   命令行：dump 信息 / 导出图标，开发期对照 aapt2
└── crates/app    # apkinfoplus：windui 桌面应用
    ├── main.rs          启动：配置、日志、i18n、主题、单实例、参数
    ├── state.rs         AppState：所有 Signal 汇总 + 后台任务消息类型
    ├── ui/main_window.rs  工具栏 + 概览头 + 标签页
    ├── ui/overview.rs / permissions.rs / splits.rs / signature.rs / text_info.rs
    ├── ui/install.rs    安装对话框
    ├── ui/rename.rs     重命名对话框
    ├── ui/settings.rs   设置窗口（单例子窗，侧栏导航）
    ├── ui/about.rs
    ├── config.rs        TOML 配置（dirs 目录）
    └── i18n/*.toml      译文
```

核心库对 UI 的契约是一个 `ApkInfo` 值（`Clone`），UI 不触碰解析细节；所有耗时操作在 core 内以普通函数提供，
由 app 决定放到哪个线程。

## 5. 界面（桌面布局）

主窗口默认 960×640，最小 720×480，系统标题栏。

```
┌────────────────────────────────────────────────────────────────┐
│ [打开] [重新解析] [安装] [重命名] [文本信息] [关闭文件]  …  [⚙] │  工具栏
├────────────────────────────────────────────────────────────────┤
│ ┌──────┐  应用名称  [语言▾]                                      │
│ │ icon │  com.example.app              版本 1.2.3 (123)          │  概览头
│ │ 96px │  APK · 12.3 MB · minSdk 24 · targetSdk 34              │
│ └──────┘  候选图标 ◀ ▶  [导出▾]                                  │
├────────────────────────────────────────────────────────────────┤
│  概览 │ 权限(23) │ 分包/OBB │ 签名 │ 哈希 │ 文本信息             │  标签页
├────────────────────────────────────────────────────────────────┤
│  (各页内容，可滚动；字段为「标题 | 值 | 复制」三列)              │
│                                                                │
├────────────────────────────────────────────────────────────────┤
│  文件：/path/to/app.apk                       [复制路径][打开目录] │  状态栏
└────────────────────────────────────────────────────────────────┘
```

- 未打开文件时主体显示一个大的拖放区（虚线框 + 提示 + 「打开文件」按钮）。
- 全窗接收拖放；只取第一个文件；扩展名不支持时 toast 提示。
- 快捷键：Ctrl+O 打开，F5 重新解析，Ctrl+I 安装，F2 重命名，Ctrl+W 关闭文件，Ctrl+, 设置，Esc 关对话框 / 退出。
- 概览页字段：文件、大小、文件类型、包名、版本号、版本名、最低 SDK、目标 SDK、
  编译 SDK、应用类名、启动 Activity、屏幕尺寸、屏幕密度、ABI、语言列表、uses-feature。
- 权限页：列表 + 过滤框 + 复制全部。
- 分包页：分包表（文件名 / 类型 / 大小）、OBB 表。
- 签名页：方案徽章 + 每个签名者的证书信息卡。
- 哈希页：MD5 / SHA1（带复制）、计算中显示进度。
- 文本信息页：只读多行文本框（可选可复制）。
- 安装对话框：设备表（勾选 / 名称 / 状态 / 结果），高级选项，分包勾选（按类型分组），进度与取消。
- 设置窗口（单例，720×520）：侧栏「外观 / 功能 / 工具 / 关于」。

主题：`Theme::default()` / `Theme::dark()` + 用户强调色；跟随系统用 `on_system_theme_changed`。

## 6. 已定决策

| 决策 | 结论 | 理由 |
|---|---|---|
| 解析引擎 | 仅内置 | 用户目标「直接解析 APK」；aapt2 依赖 Android SDK |
| 归档包 base APK | 内置引擎解析（内存中打开） | 去掉 aapt2 |
| 签名 | 纯 Rust 提取证书，不做完整校验 | apksigner 需 Java；证书指纹是用户最常用的信息 |
| 矢量渲染 | VectorDrawable → SVG → resvg | 渲染与导出同一条路径，clip-path/渐变天然支持 |
| 位图图标 | `image` crate（png/webp/jpeg）注册为 windui 解码器 | 核心只有 PNG |
| 配置 | TOML，`dirs::config_dir()/apkinfoplus/config.toml` | 可读可手改 |
| 日志 | `log` + 文件（`dirs::data_dir()/apkinfoplus/debug.log`，2MB 滚动） | 与原版一致 |
| 工具下载 | 仅 platform-tools（adb），`ureq` + `zip` | build-tools 不再需要 |
| 长列表 | 标签页 + 滚动，去掉「列表最大行数」设置 | 桌面有足够空间 |
| 单实例 | `App::single_instance`，第二实例转发文件路径 | 文件关联打开时复用窗口 |

## 7. 交给实现者的空间

- 各页的具体间距、字号、卡片样式按 windui `settings` 示例的风格自行把握。
- VectorDrawable 的 trimPath 与 sweep 渐变：trimPath 按路径长度裁剪实现；sweep 用 conic 近似或退化为中心色。
- 安装对话框的设备刷新间隔、临时目录命名由实现者定。
- 测试：core 用合成的二进制 XML / arsc 片段做单测；对真实 APK 的对照（与 `aapt2 dump badging` 比对）写成忽略网络的本地脚本。
