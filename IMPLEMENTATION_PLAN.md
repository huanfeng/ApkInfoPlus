# 实施计划

设计见 `docs/superpowers/specs/2026-10-21-apkinfoplus-design.md`。每个阶段一个或多个提交。

## Stage 1: 工作区脚手架
**Goal**: Cargo workspace（core + app）、设计文档、README、最小 windui 窗口可运行。
**Success Criteria**: `cargo build` 通过；`cargo run -p apkinfoplus -- --screenshot x.png` 出图。
**Tests**: 无（脚手架）。
**Status**: Complete

## Stage 2: 内置解析引擎
**Goal**: zip 读取、二进制 XML、resources.arsc、Manifest → ApkMeta、ApkReader、`apkinfo` CLI。
**Success Criteria**: 对本机数个真实 APK，包名 / 版本 / SDK / 权限 / 标签 / 图标路径与 `aapt2 dump badging` 一致；混淆容错逻辑与 Dart 版等价。
**Tests**: 字符串池（UTF-8 / UTF-16、长串）、ResTable_config（BCP-47 packed）、颜色转换、Manifest 启发式、合成二进制 XML 的解析。
**Status**: In Progress

## Stage 3: 图标
**Goal**: 候选收集、VectorDrawable → SVG、adaptive-icon 合成、位图解码、PNG / SVG / 原始导出。
**Success Criteria**: 测试 APK 的图标渲染正确（自适应 / 矢量 / 位图三类各至少一个），导出的 SVG 能被 resvg 回读。
**Tests**: 颜色解析、SVG 转换片段、候选排序、dpi 提取。
**Status**: In Progress

## Stage 4: 归档包、签名、哈希
**Goal**: XAPK / APKM / APKS / ZIP 解析；v1 / v2 / v3 证书提取；流式 MD5 / SHA1。
**Success Criteria**: 归档包解析出 base 信息 + 分包 + OBB；签名指纹与 `apksigner verify --print-certs` 一致；哈希与 `md5sum` / `sha1sum` 一致。
**Tests**: SplitType 分类、locale/ABI 推断、manifest.json 容错、签名块定位。
**Status**: Complete

## Stage 5: 主窗口
**Goal**: windui 主窗口：工具栏、拖放、打开、概览头、标签页、状态栏、i18n（zh-CN / en）、主题。
**Success Criteria**: 拖入 APK 后各页信息正确显示；截图验证布局；空闲零 CPU。
**Tests**: 译文一致性 lint；格式化函数单测。
**Status**: Complete

## Stage 6: 设置与配置
**Goal**: 设置窗口（外观 / 功能 / 工具 / 关于）、TOML 配置持久化、日志。
**Success Criteria**: 改设置即时生效并在重启后保留。
**Tests**: 配置读写回环。
**Status**: Complete

## Stage 7: 安装
**Goal**: adb 设备列表、安装对话框、普通 / 分包安装、OBB 推送、取消。
**Success Criteria**: 命令序列与设计一致；无设备时有明确提示；进度与结果按设备展示。
**Tests**: 命令参数拼装、设备行解析、OBB 路径推导。
**Status**: Complete

## Stage 8: 其余功能
**Goal**: 重命名、文本信息、图标导出菜单、文件菜单、快捷键、单实例与命令行参数、platform-tools 下载。
**Success Criteria**: 原版功能清单逐条勾完。
**Tests**: 重命名校验、下载索引解析。
**Status**: Complete

## Stage 9: 收尾
**Goal**: 其余语言（zh-TW / zh-HK / ja / ko）、README、打包说明、应用图标。
**Success Criteria**: `cargo clippy --all-targets` 零警告，`cargo test` 全绿。
**Status**: In Progress
