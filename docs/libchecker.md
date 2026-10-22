# LibChecker 库识别

来源需求：huanfeng/ApkInfoTool#6。用户希望像 LibChecker 一样，直接看到 APK 里有哪些原生库和第三方 SDK，并且复用 LibChecker 社区维护的规则，不自己维护规则。

本文记录数据来源、匹配语义、规则如何内置和更新、界面形态，以及许可要求。实现进度见仓库根目录的 `IMPLEMENTATION_PLAN.md`。

## 1. 规则数据

规则来自 [LibChecker/LibChecker-Rules](https://github.com/LibChecker/LibChecker-Rules)，许可为 Apache-2.0。我们只用它的 **v5 portable** 发布格式，契约见上游 `docs/rules-v5-contract.md`。

| 项 | 值 |
|---|---|
| 清单 | `https://raw.githubusercontent.com/LibChecker/LibChecker-Rules/rules-data/manifest.json` |
| 规则包 | 清单里的 `artifacts.portable.path`，相对清单所在目录解析，形如 `releases/<dataVersion>/portable-v5.zip` |
| 校验 | 清单里同时给出 `sha256` 和 `size`，下载后必须逐项比对 |
| 读取器版本 | 我们实现的是 portable 读取器 **5**。`minimumReader.portable > 5` 或 `schemaVersion != 5` 时拒绝使用 |

portable-v5.zip 没有外层目录，内容如下：

- `metadata.json`：`dataVersion`、`ruleCount`、`sourceRevision`、`contentSha256` 等。
- `core.json`：`{"schemaVersion":5,"rules":[…]}`。每条规则包含 `id, uuid, name, label, type, iconId, isSimpleColorIcon, isRegexRule, regexName, priority, detailPath`。
- `details/<uuid>/<id>.json`：`{uuid, data:[{locale, data:{label, dev_team, description, source_link, rule_contributors}}]}`，locale 目前有 `zh-Hans` 和 `en`。
- `icons/<iconId>.svg`：厂商图标。
- `matching-fixtures.json`：上游给出的匹配语义测试用例，我们的单测直接跑它。

规则类型 `type`：0 native、1 service、2 activity、3 receiver、4 provider、5 DEX、6 static、7 permission、8 metadata、9 action。第 48 版共 3048 条：native 1633，activity 599，service 295，provider 173，receiver 145，action 107，DEX 82，static 14。

**支持范围**：一期支持 0–4 和 9，与上游 tgbot 的能力集 `[0,1,2,3,4,9]` 一致。DEX（5）留到 DEX 扫描阶段再做。其余类型按契约要求**过滤掉**，不能挪作他用。

## 2. 匹配语义

按上游契约和 tgbot 的 JS 参考实现（`packages/shared/src/sdk-markers.js`）执行：

1. **先做精确匹配**：同类型下名字完全相等即命中。所有规则都参与，包括正则规则——正则规则的 `name` 原文也作为字面量参与精确匹配，这是旧版行为。
2. **再做正则匹配**：只在精确匹配未命中、且该类别允许正则时进行。规则按 `(priority, id)` 升序，第一条整串匹配的规则胜出。
   - 正则编译成 `^(?:pattern)$`，要求整串匹配。Rust 的 `$` 不会在末尾换行前提前匹配，与契约「不接受末尾换行」一致。
   - 输入名字里含 LF、CR、U+0085、U+2028、U+2029 时跳过正则阶段。这对应契约里「点号不匹配行终止符」的要求，JS 参考实现也是这么做的。
   - **`\d` 规范化**：契约要求只认 ASCII 数字。做法是按词法逐字符扫描：字符类外未转义的 `\d` 换成 `[0-9]`，字符类内换成 `0-9`；`\\` 这类转义保持原样。不能整串替换。精确匹配仍然用原文。
   - 编译失败的正则直接跳过并记日志，与 JS 参考实现一致。
3. **组件**：
   - 名字先规范化。以 `.` 开头的补上包名前缀；不含 `.` 的补成 `包名.名字`。
   - 先用组件名在对应类型（activity/service/receiver/provider）里查，允许正则。
   - 查不到时，再用该组件 intent-filter 里的每个 action 去查 type 9，只做精确匹配。
4. **原生库**：
   - 用 `.so` 文件名（不含路径）匹配 type 0，允许正则。
   - 以下几个库名太常见，命中后还要过一道校验：
     - `libjiagu*.so`（360 加固）和 `libDexHelper*.so` / `libdexjni.so`（梆梆加固）：需要在 DEX 里找到对应类名才算数。DEX 扫描完成前一律不命中，宁缺毋滥。
     - `libapp.so`：需要同包里有 `libflutter.so`。原实现还会去 DEX 里找 FlutterInjector，这一步留到 DEX 阶段。
     - `libmain.so`：需要同包里有 `libunity.so`。
5. **汇总**：命中结果按 `label + iconId` 分组。原生库统计去重后的库名数、文件数和 ABI 集合；组件按四类分别计数。组之间按命中数降序排列，数量相同时按 label 排序。

## 3. 数据提取（apkinfo-core）

- `ApkMeta.components`：四类组件全部提取，每个组件记录规范化后的全名、短名（去掉包名前缀）、`exported` 和 intent-filter 里的 action 列表。`activity-alias` 归入 activity。
- `ApkInfo.native_libs`：`lib/<abi>/<name>.so` 条目，记录 abi、文件名、解压后大小。
  - 对归档包（XAPK/APKM/APKS），除了 base APK，还要读 ABI 分包（`config.arm64_v8a.apk` 等）的目录，因为原生库通常在分包里。
  - 只读分包的 ZIP 中央目录，不解压 .so 文件。
- `apkinfo_core::libchecker`：负责规则加载（`RuleSet`）、匹配（`RuleSet::find`），以及对一个 `ApkInfo` 出报告（`analyze`）。纯函数，不碰 UI。

## 4. 规则如何内置、更新

### 4.1 构建时内置（CI 同步上游）

- 规则数据**不提交进仓库**。由 `scripts/sync-libchecker-rules.py` 在构建前从上游拉取最新一版。
- 脚本只用 Python 标准库，执行以下步骤：
  1. 拉清单，检查 `schemaVersion` 和 `minimumReader.portable`。
  2. 下载 portable 规则包，校验 sha256 和 size。
  3. 重新打包成体积更小的 `crates/core/assets/libchecker/rules.zip`，并取上游 `LICENSE` 一起放入。
- 重新打包的原因：上游 zip 不压缩，4.6MB，有 3000 多个小条目。重新打包时：
  - `details/` 按 uuid 去重，合并成一个 `details.json`；
  - 全部条目改用 deflate 压缩；
  - 结果约 0.6MB。
- `crates/core/build.rs` 的处理：
  - 存在 `rules.zip` 时，把它嵌进二进制；
  - 不存在时嵌入空数据，并打印一条 `cargo:warning`。这种情况下程序照常可用，库页会提示「未内置规则，可在设置里下载」。
- `ci.yml` 和 `release.yml` 都在 cargo 之前执行这个脚本：
  - 每次发版都自动带上发版那一刻的最新规则；
  - 普通 CI 也用真实规则跑测试，包括上游 fixtures。
- 可以用环境变量 `LIBCHECKER_RULES_VERSION=<N>` 固定到某个 dataVersion，便于复现。不设时取清单里的最新版。
- 本地开发执行一次 `python3 scripts/sync-libchecker-rules.py` 即可。

### 4.2 运行时更新

- 设置页有「更新规则」按钮。它下载上游清单和 portable-v5.zip，校验通过后原样存到 `<数据目录>/libchecker/portable-v5.zip`。
- 加载器两种布局都认：
  - 重新打包后的布局：`details.json`；
  - 上游原始布局：`details/<uuid>/<id>.json`。
- 启动时比较用户目录和内置两份的 `dataVersion`，用版本高的那份。用户目录那份损坏或版本不兼容时，回退到内置版本。
- 不自动联网检查更新。

## 5. 界面

新增「库」标签页，位置在「权限」之后。页面内容如下：

- **顶部摘要**：「识别到 N 个 SDK」，并注明规则版本（dataVersion）和来源。
- **原生库区**：
  - 按 SDK 分组，每组显示图标、label、命中库名、ABI；
  - 未命中的库列在「未识别」小节；
  - 每个库显示大小。
- **组件区**：按 SDK 分组，列出命中的组件（短名）和类型；未命中的组件不在本页逐条展示，只给数量。
- **详情**：点击某个 SDK 弹出详情，内容包括描述、开发团队、来源链接、规则贡献者。界面语言是中文时显示 `zh-Hans`，其他语言显示 `en`。
- **页脚**：「规则与图标来自 LibChecker-Rules（Apache-2.0）」，带链接。

## 6. 许可与署名

- LibChecker-Rules 为 Apache-2.0，可以内置和再分发，但必须满足以下几点：
  - 附带许可文本：脚本把上游 `LICENSE` 放进 `rules.zip`，发布包另附 `LICENSE-LibChecker-Rules.txt`；
  - 注明来源：页脚和关于页；
  - 说明是否修改过：我们只重新打包，不改内容。在 README 写明。
- 图标是各厂商商标，仅用于标识对应 SDK，不暗示任何背书。

## 7. 后续阶段（暂不做）

- **DEX 扫描**：读 `classes*.dex` 的 class_defs，取出包名前缀，用来：
  - 匹配 type 5 规则；
  - 识别 Kotlin / Compose 技术栈标记；
  - 完成加固库和 FlutterInjector 的校验。
- **ELF 分析**：16KB 页对齐、是否 strip、ZIP 对齐。
- **其他类型规则**：permission / metadata / static。上游 tgbot 也不支持，暂不跟进。
