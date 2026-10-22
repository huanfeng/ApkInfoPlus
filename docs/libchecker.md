# LibChecker 库识别

来源需求：huanfeng/ApkInfoTool#6。用户希望像 LibChecker 一样，直接看到 APK 里有哪些原生库和第三方 SDK，并且复用 LibChecker 社区维护的规则，不自己维护规则。

本文记录数据来源、匹配语义、规则如何内置和更新、界面形态，以及许可要求。实现分五个阶段完成（组件与原生库提取、规则加载与匹配、规则同步与 CI、「库」标签页、运行时更新），后续阶段见 §7。

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

**支持范围**：0–4、9（与上游 tgbot 的能力集 `[0,1,2,3,4,9]` 一致）加上 DEX（5）。其余类型按契约要求**过滤掉**，不能挪作他用。

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
     - `libjiagu*.so`（360 加固）：需要 DEX 里定义了 `com.qihoo.util.*` 或 `com.tianyu.util.*` 类。
     - `libDexHelper*.so` / `libdexjni.so`（梆梆加固）：需要 DEX 里定义了 `com.secneo.apkwrapper.*` 类。
     - `libapp.so`：需要同包里有 `libflutter.so`，或 DEX 里有 `io.flutter.FlutterInjector`。
     - `libmain.so`：需要同包里有 `libunity.so`。
5. **DEX（type 5）**：
   - 上游 LibChecker 安卓端的 DEX 匹配不在开源代码里（`PackageUtils.getDexList` 只有 `Not implemented`），这里按规则数据的形态自行定义：type 5 规则名都是包名前缀，如 `com.tencent.smtt`、`androidx.lifecycle`。
   - 对 DEX 里每个定义了类的包，先用包名本身、再逐级用上级包做精确匹配，取最长命中。
   - 都不中时，用完整包名试正则规则（上游有 8 条，如 `okhttp3\.(.*)`、`kotlin\.(.*)`）。
   - 一个 SDK 分组里列出命中的全部包，并按包数排序。
6. **技术栈标记**（与 JS 参考实现一致）：
   - Kotlin：字符串表里有 `Lkotlin/Metadata;`，或定义了 `kotlin.*` 类，或包里有 `kotlin-tooling-metadata.json` / `META-INF/*.kotlin_module`。
   - Jetpack Compose：定义了 `androidx.compose.*` 类，或有 `META-INF/androidx.compose.*.version`。
   - 360 加固 / 梆梆加固：上面的壳类。
7. **汇总**：命中结果按 `label + iconId` 分组。原生库统计去重后的库名数、文件数和 ABI 集合；组件按四类分别计数。组之间按命中数降序排列，数量相同时按 label 排序。

## 3. 数据提取（apkinfo-core）

- `ApkMeta.components`：四类组件全部提取，每个组件记录规范化后的全名、短名（去掉包名前缀）、`exported` 和 intent-filter 里的 action 列表。`activity-alias` 归入 activity。
- `ApkInfo.native_libs`：`lib/<abi>/<name>.so` 条目，记录 abi、文件名、解压后大小。
  - 对归档包（XAPK/APKM/APKS），除了 base APK，还要读 ABI 分包（`config.arm64_v8a.apk` 等）的目录，因为原生库通常在分包里。
  - 只读分包的 ZIP 中央目录，不解压 .so 文件。
- `ApkInfo.dex`：`apkinfo_core::dex` 读根目录的 `classes*.dex`，一次只在内存里放一个 dex。
  - 只读 header、string_ids、type_ids、class_defs，取出每个定义类的描述符，按包名计数，并顺带收上面的标记。不解析代码。
  - 每次读都做边界检查，坏文件只是少收一部分。
  - 加载时同步扫描：217MB 的微信输入法（4 个 dex、3 万个类）约 0.2 秒。
  - 归档包只扫 base APK；特性分包里的 dex 暂不扫。
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

- 设置 →「工具」里有「LibChecker 规则」卡片，显示当前规则的版本、条数和来源（内置或已下载）。
- 点「检查更新」后，后台线程调用 `libchecker::update_rules` 完成更新，步骤如下：
  1. 拉清单，与当前版本比较 `dataVersion`，不比当前新就直接结束。
  2. 下载 portable-v5.zip，校验 size 和 sha256，再试加载一次，确认包里的 `dataVersion` 与清单一致。
  3. 先写 `.part` 临时文件，再改名为 `<数据目录>/libchecker/portable-v5.zip`。校验失败时不动已有文件。
- 更新成功后立即换上新规则，并把已打开的文件重新识别一遍；之后打开的文件也用新规则。
- 加载器两种布局都认：
  - 重新打包后的布局：`details.json`；
  - 上游原始布局：`details/<uuid>/<id>.json`。
- 启动时比较用户目录和内置两份的 `dataVersion`，用版本高的那份；版本相同时用内置的。用户目录那份损坏或版本不兼容时，回退到内置版本。
- 不自动联网检查更新。

## 5. 界面

新增「库」标签页，位置在「权限」之后。页面内容如下：

- **顶部摘要**：「识别到 N 个 SDK」，并注明规则版本（dataVersion）和来源。
- **原生库区**：按 SDK 分组，每组显示图标（单色图标按主题文字色着色）、label、命中库名、ABI。
- **组件区**：
  - 按 SDK 分组，预览前 3 个命中组件（短名），并按类别计数；
  - 未命中的组件不逐条展示，只给数量。
- **DEX 中的 SDK**：按 SDK 分组，预览前 3 个包并显示包数；底部注明 DEX 文件数、类数和包数。
- **技术栈**：在顶部摘要里用徽章显示 Kotlin / Jetpack Compose / 加固。
- **未识别的原生库**：放在页面最后，按库名合并 ABI，显示大小。放在最后是为了不把组件区挤到很下面。
- **详情**：
  - 点击某个 SDK 弹出详情，内容包括描述、开发团队、来源链接、命中项和规则贡献者。
  - 界面语言是中文时显示 `zh-Hans`，其他语言显示 `en`。
  - 正则规则，或者只命中一项的分组，才显示所用的规则。
- **标签标题**：带识别出的 SDK 数。英文等较长的界面语言下，标签条放不下时，windui 会从右往左把未选中的标签收成图标。
- **页脚**：「规则与图标来自 LibChecker-Rules（Apache-2.0）」，带链接。

## 6. 许可与署名

- LibChecker-Rules 为 Apache-2.0，可以内置和再分发，但必须满足以下几点：
  - 附带许可文本：脚本把上游 `LICENSE` 放进 `rules.zip`，发布包另附 `LICENSE-LibChecker-Rules.txt`；
  - 注明来源：页脚和关于页；
  - 说明是否修改过：我们只重新打包，不改内容。在 README 写明。
- 图标是各厂商商标，仅用于标识对应 SDK，不暗示任何背书。

## 7. 后续阶段（暂不做）

- **DEX 进阶**：扫描特性分包里的 dex；在 dex 容器格式（v41）里只读第一个 header。
- **ELF 分析**：16KB 页对齐、是否 strip、ZIP 对齐。
- **其他类型规则**：permission / metadata / static。上游 tgbot 也不支持，暂不跟进。
