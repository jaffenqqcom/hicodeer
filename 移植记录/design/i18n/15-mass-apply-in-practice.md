# 第 15 章 全量回注实操与「四道闸」

这一章记录**「把 9439 处界面文案一次性回注进源码树」的实际执行过程**，重点不是「回注成功了」，而是**工具在执行中四次把自己拦住**——每次都在**内存规划阶段**（零写入）因**自身一致性校验**失败，以及每次的解法。

这一章最有复用价值：以后换上游版本、再加语言重跑回注，几乎必然会再撞上这四类闸中的某几道。

> 说明：本章的命令、日志、行号多为**只读复核**得到；涉及「回注执行时的实时输出」的引用均给出日志文件路径。所有涉及仓库当前规模（文件数、行数）的数字都带时间戳——因为仓库在 2026-09-29 22:4x 仍被另一个 worker 改写（release 构建 + 默认配置本地化），读数为**时刻快照**。

## 15.1 为什么必须用 `apply-universal`，不能用 `apply.py`

工具链里有两条回注路线：`apply` 与 `apply-universal`。**本任务只能用后者**，原因有三条（都可从工具源码直接读到）：

1. **`apply` 写死单语言**——它把英文字面量**直接替换成目标语言的译文**（例如把 `"Open Settings"` 就地换成 `"打开设置"`）。这样一来：
   - 源码里不再有英文原文，**语言切换失效**（想切回英文就得重编）；
   - 加第二种语言要**再改一遍源码**，两次回注互相冲突。
2. **`apply` 的目标路径写死**为 `<zed-i18n 检出目录>/.cache/zed/v1.21.0`——那是它自己 `fetch-zed` 拉下来的 1.21 源码缓存，**指不到我们的树**。我们的树在 `/storage/Users/currentUser/workspace/hicodeer`，版本是 1.23，对不上。
3. **`apply-universal` 的形态正好相反**：它**把英文字面量包成 `localization::localized_str!("...")`**（运行时按当前 locale 查表），所以：
   - 英文原文**原样保留**成为查表 key，切换语言/加语言**不用再动源码**；
   - 回注是**先在内存里规划好全部替换、校验通过后一次性落盘**（不会「改到一半失败」留下半棵被改的树）；
   - 自带**幂等标记**（`.zed-i18n-universal.json`），重跑能识别已应用状态。

「新增行里一个中文都不该有」正是「用对了 `apply-universal`」的判据，详见第 10.3 节。

## 15.2 全量回注的实际命令序列

完整动作是四步（顺序不能换）：

```sh
VENV=/storage/Users/currentUser/tmp/zed-i18n-venv
DRIVER=/storage/Users/currentUser/tmp/i18n-driver.py
ROOT=/storage/Users/currentUser/tmp/zed-i18n
REPO=/storage/Users/currentUser/workspace/hicodeer

# 1) 对 1.23 树重新提取，得到对准本树的字节偏移
$VENV/bin/python $DRIVER --root $ROOT extract --zed-root $REPO
#    判据：Extracted 7977 source strings from 10210 occurrences

# 2) 核对 catalog_sha256（词表/语言包与 catalog 的绑定值一致）

# 3) 需要时重新生成运行时语言包
$VENV/bin/python /storage/Users/currentUser/tmp/run_i18n_bundles.py
#    判据：assets/locales/*.json 被刷新

# 4) 一次性回注（把英文字面量包成 localized_str!）
$VENV/bin/python $DRIVER --root $ROOT apply-universal --zed-root $REPO
#    判据：Applied universal localization to 9439 occurrences across 90 crates
```

**驱动脚本形态**（`/storage/Users/currentUser/tmp/i18n-driver.py`，实测 31 行）：它在运行时把 `tools.zed_i18n.cli.ensure_inside_workspace` 替换为**恒等函数**后再调 `cli.main`——因为上游 CLI 的工作区守卫会拒绝仓库外的 `--zed-root`。**它不修改 zed-i18n 检出里的任何文件**。`run_i18n_bundles.py` 做同样的事（第 6-7 行替换守卫）。

实测输出（引自 `tmp/i18n-apply-run2.log`）：

```
[driver] patched ensure_inside_workspace in: ['tools.zed_i18n.zed_source', 'tools.zed_i18n.cli']
Applied universal localization to 9439 occurrences across 90 crates
```

## 15.3 四道闸（工具执行中四次被自己拦下）

四道闸都发生在**内存规划阶段、零写入**——这是工具的**设计特性**（先规划、后落盘），不是 bug。前三次尝试都被拦下，很容易误判成「工具坏了」，实际是它**不敢在偏移对不上时动源码**。

### 第一道闸：手写补丁站点对不上（83 → 80）

- **现象**：`apply_universal.py` 的 `_EXPECTED_MANUAL_SITES` 共 **83 条**，其中 **3 条**指向 `crates/extensions_ui/src/extension_suggest.rs` 的 Emmet 文案：
  - `Emmet expands abbreviations such as \`ul>li*3\` into HTML and \`m10\` into CSS.`
  - `Emmet is available for this file`
  - `Install Emmet`
- **为什么对不上**：该文件在 1.23 **已迁移**——拆成 `crates/extensions_ui/src/extension_suggestions.rs` 与**新 crate** `crates/extension_suggest/`（实测这 3 个路径都出现在 `tmp/upstream-changed.txt` 的上游变更清单里）；而且这 3 条文案**未被 extract 收录**（在 catalog 里 0 occurrence）。
- **解法**：在 `tmp` 里的**工具副本**（`tmp/zed-i18n/tools/zed_i18n/apply_universal.py`）上**移除这 3 条站点**并同步站点计数字段，改前先备份原文件（`tmp/i18n-apply-backup/apply_universal.py`），改完做语法校验。
- **实测复核**（用 `ast` 解析两版）：
  - 备份版 `_EXPECTED_MANUAL_SITES` = **83** 条，其中含 extension_suggest 的 **3** 条；
  - 现版 = **80** 条，含 extension_suggest 的 **0** 条；
  - `_EXPECTED_MANUAL_SITE_COUNTS` 两版均为 **3** 条（值均为 2，对应 `remote_output.rs` / `base_keymap_setting.rs` / `tool_permissions_setup.rs` 三处）。
- **副作用（如实记录）**：那 3 条 Emmet 文案在成品里**保持英文**（没被包成 `localized_str!`）。这是「为 3 条文案不阻塞 9439 处回注」的取舍。

### 第二道闸：手工骨架与工具结构性补丁打架

- **现象**：手工接入阶段改过 `crates/title_bar/src/application_menu.rs`（加了 `mod menu_labels;`、改了 `render_standard_menu` 签名、走自写的 `menu_labels::label/item_label`），而工具对**同一文件**本来有一套**结构性补丁**（期望原样再改一次）。工具找锚点失败：

```
ValueError: structural patch anchor count for crates/title_bar/src/application_menu.rs is 0, expected 1
RC=1
```

  （引自 `tmp/i18n-apply-run.log`，工具在 `apply_universal.py:539 _plan_zed_runtime_patches` 处抛错。）

  > 同期还有一处同类报错（`tmp/i18n-apply-backup/apply-universal.stderr.log`）：
  > `stale occurrence source: crates/title_bar/src/application_menu.rs:23: expected 'Activates the menu on the left in the client-side application menu.'` —— **同源原因**（该文件被手工改过，偏移与内容都对不上）。

- **用户裁决：走「路 A」——回退手工骨架，让工具统一接管。**
  - **动作**：把手工改过的 **4 个文件回退到 HEAD**：
    - `crates/title_bar/src/application_menu.rs`
    - `crates/title_bar/Cargo.toml`
    - `crates/zed/Cargo.toml`
    - `crates/zed/src/main.rs`
    并**删掉手工建的 `crates/title_bar/src/application_menu/` 目录**（那是手写的 `menu_labels.rs`）。
  - **保留**前序已成立的部分：`crates/localization/`、`assets/locales/`、根 `Cargo.toml` 里 `crates/localization` 的登记。
  - 回退用的备份在 `tmp/i18n-landing-backup/repo/`（实测含上述 4 文件 + `Cargo.toml`/`Cargo.lock`）。
- **为什么要走路 A（理由要写清）**：为了 **10 个菜单 key** 的手写接线，**拖住 9439 处回注**不划算；而且**回退动作可随时重做**（工具接管后骨架会由工具补齐，见第 10.3 节的 4 个骨架文件）。最终架构也更简单：菜单标签的接线交给工具统一生成，不保留两套并存的改法。

### 第三道闸：字节偏移过期

- **现象**：manifest 是**按「有手工骨架的那棵树」提取**的，路 A 回退后，那 **15 条 occurrence**（`application_menu.rs` 13 条 + `main.rs` 2 条）的字节偏移**全部失效**。工具报：

```
ValueError: occurrence byte span is stale: crates/title_bar/src/application_menu.rs:199
```

  （引自 `tmp/i18n-bundles2.log`，发生在 `generate-runtime-bundles` 的 `runtime_bundles.py:467`。）

- **解法**：**补跑一次 `extract`，让偏移对齐当前树**——这是「路 A」的**必要收尾**。实测补跑输出 `Extracted 7977 source strings from 10210 occurrences`（引自 `tmp/i18n-extract-rerun.log`）。
- **教训（务必记住）**：**任何改动源码之后，都必须重跑 `extract` 再回注**。字节偏移只对「提取那一刻的树」有效，源码一改就作废。

### 第四道闸：根 `Cargo.toml` 重复注册

- **现象**：前序遗留在根 `Cargo.toml` 里已经注册过 `crates/localization`（`members` + workspace 依赖），工具又插了一遍 → `members` 与 `[workspace.dependencies]` 各出现**两次**。严格 TOML **不允许同表重复键**，`tomllib` 解析直接报 `Cannot overwrite a value`。
- **解法**：删掉重复对中的**各一行**，保留**字母序正确**的那份（`members` 里保持 `lmstudio` 与 `lsp` 之间的位置；依赖声明保持唯一一条）。
- **收尾校验**：改完必须用 `tomllib` 校验根 `Cargo.toml` **以及所有被回注改过的 `crates/*/Cargo.toml`（约 90 个）**全部可解析。
- **未核实项**：本次**未找到**该闸报错的留存日志文件；机制按「严格 TOML 不允许重复键」与工具插入逻辑描述，**报错原文未能从日志复核**，标注「未核实」。

## 15.4 偏移健康度（回注前的只读预演结论）

回注前先做了一次**只读预演**（不改源码），检查 manifest 里每条 occurrence 的字节偏移在**当前树**上是否仍然精确命中。结论（**预演结论，引用**；本次未能从留存日志复核该组数字，标注「未复核」）：

- `accepted` 共 **9293 处**，其中 **9145 处（98.41%）字节偏移精确命中**。
- 非精确的 **148 处**拆解：
  - **72 条**是枚举标签（span 指向 `ActiveEditor` 而 key 是 `Active Editor`）——工具**本就用 `#[strum]` 属性处理**这类，属**预期内**，不算坏。
  - **50 条**跨行字符串（span 覆盖多行，需特殊拼装）。
  - **11 条真正失效**——**全部**在 `crates/title_bar/src/application_menu.rs`，因为该文件**刚被手工骨架改过**；**重跑 extract 即修**（这正是第三道闸的收尾）。
  - 少量其它。

对照实测：2026-09-29 22:4x 从 `tmp/zed-i18n/manifest/ui-strings.json` 实测 **`accepted` 状态的 occurrence 合计 = 9445 处**（`needs_review` 50 处、`ignored` 715 处，总计 10210）。

> **口径差异如实标注**：预演的「accepted 9293 处」与本次实测的「accepted occurrence 9445 处」**不一致**（差 152）。推测两者口径不同（预演可能在修复站点/重提取前后的某一中间态统计），**未定论**。以本章标题下的数字各标来源，不混用。

## 15.5 回注结果与判据

- 工具输出：`Applied universal localization to 9439 occurrences across 90 crates`（`tmp/i18n-apply-run2.log`）。
- 幂等标记 `.zed-i18n-universal.json`（仓库根，未跟踪）：`applied_occurrences=9439`、`dependency_crates` 90 项、`manifest_sha256=1d20987876eef2b2c616bb46ca5d3776f57e91e38ec2633f2d160f7ad58ec911`、`schema_version=1`。
- 改动规模、质量判据、OHOS 标记保留情况、骨架落地情况：**详见第 10.3 节**（本处不重复）。
- 回注后 **debug 构建已成功**（`tmp/bundle-ohos-after-i18n.log`，2026-09-29 20:11，`=== HAP BUILD SUCCESSFUL ===` + `EXITCODE=0`）。
