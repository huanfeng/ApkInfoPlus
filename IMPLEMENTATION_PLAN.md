# LibChecker 库识别实现计划

设计见 `docs/libchecker.md`。全部阶段完成后删除本文件。

## Stage 1: 组件与原生库提取
**Goal**: `ApkMeta.components`（四类组件 + action）与 `ApkInfo.native_libs`（含归档包 ABI 分包）
**Success Criteria**: 真实 APK / XAPK 上能列出组件和 .so；既有 `apkinfo` 输出不变
**Tests**: manifest 组件提取与名字规范化单测；native_libs 解析单测
**Status**: Complete

## Stage 2: 规则加载与匹配
**Goal**: `apkinfo_core::libchecker`：`RuleSet::load`（两种布局）、`find`、`analyze`
**Success Criteria**: 上游 `matching-fixtures.json` 全部通过；对样本 APK 给出合理报告
**Tests**: fixtures、`\d` 规范化、组件 action 回退、原生库校验、汇总分组排序
**Status**: Complete

## Stage 3: 规则同步与内置、CI
**Goal**: `scripts/sync-libchecker-rules.py`、core `build.rs` 内嵌、ci/release 工作流同步上游、发布包附许可
**Success Criteria**: 无规则文件时可编译并有 warning；CI 拉到最新规则并跑通测试
**Tests**: 本地跑脚本 + `cargo test`；CI 绿
**Status**: Complete

## Stage 4: 「库」标签页
**Goal**: 原生库 / 组件分组展示、SDK 详情、图标、署名页脚、i18n
**Success Criteria**: Windows / Linux 实机界面可用
**Tests**: i18n 完整性测试；实机截图核对
**Status**: Not Started

## Stage 5: 运行时更新规则
**Goal**: 设置页「更新规则」、用户目录规则优先、版本比较与回退
**Success Criteria**: 更新后无需重启即可在新打开的文件上生效
**Tests**: 版本选择逻辑单测；实机更新一次
**Status**: Not Started
