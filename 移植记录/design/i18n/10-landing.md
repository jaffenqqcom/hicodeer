# 第 10 章 落地（骨架 + 语言包 + 编译 + 装包）

## 10.1 本章状态（如实说明）

- 本章先写**落地方案与步骤**（方法论部分，**完整**，可照做），10.3 节写**已确定结果**与**进行中项**（带时间戳）。
- **截至 2026-09-29 22:4x 的进展**：全量回注已完成、骨架与语言包已落地、回注后 **debug 构建已成功**；**release 构建正在跑**，装包与线上取证**未做**。
- 撰写本章时的早期旁证（已过时数字，保留供对照）：`assets/locales/zh-CN.json` 曾为 `messages` 7434 / `formats` 477；**实测现为 7558 / 485**（见 10.3）。
- 注：本目录原描述「落地工作正在并行进行、结果待补」已不准确，现以 10.3 的实测结果为准。

## 10.2 落地方案与步骤

### 第 1 步：先备份要改的每个仓库文件

- 把本次要改的每个仓库文件**先备份**到 `tmp/i18n-landing-backup/`。
- **原因**：git 使用规则禁止 `checkout` / `restore` 回退，回退只能靠**备份**（见第 14 章决策记录）。

### 第 2 步：合并词表

- 把 `translations/zh-CN.json`（基线 **7316** 条）与补充词表（**112** 条，见第 8 章）合并。
- **冲突时以原词表译文为准**（保持风格统一），差异**记录下来**。
- 实测现状：`tmp/zed-i18n/translations/zh-CN.json` 已为 **7428** 条（= 7316 + 112），判定本步已由并行 worker 完成。

### 第 3 步：生成运行时语言包

- 用现成 venv 跑它的 `generate-runtime-bundles` 子命令：

```sh
/storage/Users/currentUser/tmp/zed-i18n-venv/bin/python <driver.py> \
    --root /storage/Users/currentUser/tmp/zed-i18n \
    generate-runtime-bundles <相关参数>
```

- 同样**可能遇到工作区守卫**（`ensure_inside_workspace`）→ 用与 extract 相同的手法：**运行时替换函数绕过**（不改项目任何文件）。
- 产出：语言包文件（每个 locale 一个 JSON）。

### 第 4 步：语言包放入仓库

- 把生成的语言包放入仓库 `assets/locales/`。
- 实测现状：已放入，含 14 个文件（13 语言 + `index.json`）。

### 第 5 步：接入运行时骨架

用 overlay 里的既有件（源在 `tmp/zed-i18n/tools/zed_i18n/runtime_overlay/crates/`，实测该目录含 `language_selector` / `localization` / `settings_ui` / `title_bar` / `zed` 五个 crate）：

- `crates/localization`（**已接**）。
- `crates/zed` 的 `ui_locale.rs`。
- `crates/title_bar` 的菜单标签。
- `crates/settings_ui` 的 `locale_picker.rs`（语言选择器）。
- `crates/language_selector` 的 `localized_language_name.rs`（语言名本地化）。

### 第 6 步：启动时初始化

- 在**最早的启动路径**调 `localization::initialize(...)`。
- 本轮 `user_preference` 先**硬编码 `Some("zh-CN")`**。
- **后续必须改成从系统 / 设置接入**——因为第 9 章的结论（`sys-locale` 在 OHOS 上拿不到系统语言，必须显式传入）。

### 第 7 步：编译

- 命令：`script/bundle-ohos`（debug）。
- 判据：**EXIT=0** 且出现**构建成功标志**（HAP BUILD SUCCESSFUL）。

### 第 8 步：覆盖安装到设备

- **严禁卸载 / 重装**，只能用**不带卸载参数**的 `install-local.sh`（覆盖安装，保留沙箱数据）。
  - 原因：卸载会清空沙箱数据，用户手工配置（如全局快捷键开关）会丢失（全局规则第 21 条）。

### 第 9 步：取证

- 用 `hdc hilog` **按程序名过滤**看本地化初始化结果（如 `localization initialized: requested=..., system=..., resolved=..., source=..., fallback=...`，该日志格式见 `crates/localization/src/localization.rs:312`）。
- **界面效果需人工确认**（**禁止截图**，全局规则第 13 条）。

## 10.3 落地结果

本节分**已确定**与**进行中**两部分。所有实测数字带时间戳（2026-09-29 22:4x）；仓库此时仍被另一个 worker 改写（release 构建 + 默认配置本地化），读数为**时刻快照**。

### 10.3.1 已确定

**（1）全量回注已执行**

- 工具输出：`Applied universal localization to 9439 occurrences across 90 crates`（引自 `tmp/i18n-apply-run2.log`）。
- 幂等标记 `.zed-i18n-universal.json`（仓库根，未跟踪，1496 字节）实测内容摘要：`applied_occurrences=9439`、`dependency_crates` 90 项、`manifest_sha256=1d20987876eef2b2c616bb46ca5d3776f57e91e38ec2633f2d160f7ad58ec911`、`schema_version=1`。
- 退出码：脚本正常退出（日志无 traceback），rc=0。
- 耗时：对话记录为 **109 秒**；**本次未从留存日志中核到对应耗时**，标注「未核实」。

**（2）改动规模（2026-09-29 22:48 实测）**

- `git diff --numstat` 汇总：**455 个文件 / +10301 / −5858**（二进制文件 0）。
  - **与对话记录不一致**：记录为 457 文件 / +10207 / −5857；**以本次实测为准**（差异：文件 2 个、增 94 行、删 1 行；且仓库正被另一 worker 改动，数字会变）。
- `git status --porcelain`：**463 条目** = ` M`（已跟踪被改）**455** + `??`（未跟踪）**8**。
- 按目录分类（`git diff --numstat` 按顶层目录）：`crates` 449、根 `(root)` 3（`Cargo.toml` / `Cargo.lock` / `install-local.sh`）、`hap` 2、`assets` 1（`assets/settings/default.json`）。

**（3）质量关键证据：新增行里的中文字符数 = 0**

- 实测：`git diff -U0` 的新增行共 **10301 行，其中中文字符数 = 0**；新增行里含 `localized_str!` 的有 **4694 行**。
- **为什么这条是关键判据**：它证明回注的形态是「**把英文字面量包成 `localization::localized_str!("...")`（运行时按当前 locale 查表）**」，而**不是**「把英文直接替换成中文」。
  - 前者：英文原文**原样保留**成为查表 key，运行时按 locale 查表 → **切换语言 / 增加语言都不必再改源码**。
  - 后者：英文被从源码里抹掉 → 语言切换失效、加语言要重新改源码。
  - 所以「新增行 0 个中文」＝「用对了 `apply-universal`，没有误用 `apply.py`」的**可机检判据**（工具选择理由见第 15 章）。

**（4）骨架由工具补齐（4 个文件，均存在，均属 `??` 未跟踪）**

- `crates/zed/src/zed/ui_locale.rs`（85 行 / 2861 字节）
- `crates/settings_ui/src/components/locale_picker.rs`（163 行 / 6161 字节）
- `crates/language_selector/src/localized_language_name.rs`（71 行 / 2129 字节）
- `crates/title_bar/src/application_menu/menu_labels.rs`（157 行 / 5234 字节）

**（5）初始化挂点**

- `crates/zed/src/main.rs:516`：`zed::initialize_localization(fs.clone(), cx);`（**真实行号实测为 516**；上一行 515 为 `zed::watch_settings_files(...)`，513 为 `settings::init(cx)`）。
- 链路：`crates/zed/src/zed.rs:2-3`（`mod ui_locale;` + `pub use ui_locale::initialize_localization;`）→ `crates/zed/src/zed/ui_locale.rs:7` 的 `initialize_localization` 内真正调 `localization::initialize(...)`。

**（6）语言包**

- `assets/locales/`：**14 个文件**（`index.json` + 13 语言），**合计 11456091 字节**（约 11.46 MB / 10.9 MiB）。
- `assets/locales/zh-CN.json` 实测：`messages` **7558 条**、`formats` **485 条**、`locale = "zh-CN"`、`schema_version = 1`。
  - **与早期记录不一致**：此前观察为 `messages` 7434 / `formats` 477；**以本次实测为准**（推测与词表由 7316→7428 条后重跑 `generate-runtime-bundles` 有关，**未定论**）。
- `assets/locales/index.json`：`locales` 数组 **14 项**；`en-US` 的 `bundle` 为 `null`（英文走源码原文，不需包）。

**（7）词表合并**

- `tmp/zed-i18n/translations/zh-CN.json` 实测 = **7428 条**（= 基线 7316 + 补充 112，**与记录一致**）。

**（8）OHOS 适配未被破坏**

- **11 个「撞车文件」中 9 个确实被回注改动**（`git diff --numstat` 有非空 diff），另 2 个未被改动：
  - 被改动（9）：`crates/agent_ui/src/conversation_view.rs` / `crates/editor/src/editor.rs` / `crates/git_ui/src/commit_view.rs` / `crates/settings_ui/src/pages/audio_test_window.rs` / `crates/settings_ui/src/settings_ui.rs` / `crates/terminal_view/src/terminal_panel.rs` / `crates/workspace/src/workspace.rs` / `crates/zed/src/main.rs` / `crates/zed/src/zed.rs`。
  - 未改动（2）：`crates/gpui/src/window.rs`、`crates/terminal/src/terminal.rs`。
- **OHOS 移植标记全部原样保留**：实测 `git diff -U0` 的**删除行中含 `OHOS PORT` 的行数 = 0**——即回注没有删改任何 OHOS 标记行。
- `crates/gpui_ohos/**` 与 `patches/`：`git diff --numstat` **条目数 = 0**（零改动）。
- `install-local.sh` 与 `hap/`：回注前的基线（`tmp/i18n-apply-baseline.txt`，19:38:40）里这 **5 个文件本就是既有的本地改动**（非回注引入）：`install-local.sh`、`hap/AppScope/app.json5`、`hap/AppScope/resources/base/element/string.json`、`hap/build-profile.json5`、`hap/entry/src/main/resources/base/element/string.json`。
  - 当前（22:49）`git diff --numstat -- install-local.sh hap/` 只剩 **3 个**：`install-local.sh` 1/1、`hap/AppScope/resources/base/element/string.json` 1/1、`hap/build-profile.json5` 8/5——少的 2 个是另一 worker 变动的**中间态**，标注「读数可能为中间态」。

**（9）回注后 debug 构建已成功（旁证）**

- `tmp/bundle-ohos-after-i18n.log`（2026-09-29 20:11）：`=== HAP BUILD SUCCESSFUL ===` + `EXITCODE=0`。
- 这是 **debug** 构建；本轮要求的 **release** 构建另行进行中（见 10.3.2）。

### 10.3.2 进行中 / 未做（带时间戳）

- **release 编译：进行中，结果待补。** 截至 **2026-09-29 22:49**（只读观察）：
  - 进程 `bash ./script/bundle-ohos --release`（PID 54146）在跑；
  - 子进程 `cargo.real build --release --lib -p launch-zed --target aarch64-unknown-linux-ohos`（PID 54216）；
  - 多个 `rustc` 并行编译各 crate。
  - **耗时、退出码、是否出现构建成功标志（`=== HAP BUILD SUCCESSFUL ===`）——全部「进行中，待补」。**
- **装包：未做。**
- **取证（`hdc hilog` 看 `resolved_locale` 等字段）：未做。**
- **界面人工确认：未做**（禁止截图，须由用户提供）。
- **默认配置本地化：进行中**（见第 16 章）。用户要求的 7 个赋值项在 `assets/settings/default.json`（mtime 22:47:17，正被另一 worker 改动）中**已实测出现**，但「随 release 包生效」待装包后确认。
