# 第 11 章 遗留问题与下一步

本章是**待办清单**，也是以后加语言时最该先看的一章。

## 11.1 全量回注已完成（原为「尚未做（第二步）」）

- **已完成**：全量回注已执行——**9439 处包装、覆盖 90 个 crate**；改动规模 **455 文件 / +10301 / −5858**（2026-09-29 22:48 实测）。完整数字与判据见 **10.3**，执行过程与「四道闸」见**第 15 章**。
- **当初的有利条件仍然成立**：因为我们对 1.23 树重跑了 extract（第 5 章），**manifest 的字节偏移是对准 1.23 的**——这正是回注能精确落位的前提。
- 提醒：回注用 **`apply-universal`**（把英文字面量包成 `localized_str!`，运行时查表），**不是 `apply`**（后者直接把英文换成中文、破坏语言切换）。判据是 10.3 的「**新增行里的中文字符数 = 0**」。

## 11.2 「doc 注释按行拆断」的提取缺陷需先解决

- extract 会把 `///` 文档注释按行拆断，产生残句条目（第 6 章 6.5）。
- **必须先加「doc 段落合并」或过滤**，否则落地后**设置页会出现被剁碎的译文**。

## 11.3 OHOS 系统语言：通道早已铺好、但从未被消费（重要更新）

- **不能指望 `sys-locale` 自动获取系统语言**（第 9 章结论：OHOS 上它只读四个环境变量）。
- **重要更新（设计调研结论）**：OHOS 侧把系统语言送进应用的**通道其实早就有**，只是**没被消费**：
  - ArkTS 侧**启动**就把语言塞进 init context——**本机实测** `hap/entry/oh_modules/@ohos-rs/ability/src/main/ets/ability/NativeAbility.ets:146`：`preferredLocales: context?.config?.language ?? "",`（紧随 `:147` 是 `colorMode: ...`）。
    - 行号/路径差异如实标注：只读设计产出引用的是 `openharmony-ability-zed/.../NativeAbility.ets:147`，本机实测为 **:146**，且路径形态是 `oh_modules` 依赖（本机**不存在** `openharmony-ability-zed/**`）。
  - **运行期**配置变更也在读 `language`——设计产出引用 `openharmony-ability-zed/crates/ability/src/lifecycle.rs:132-170`。**未核实**：本机不存在该文件（`crates/ability/**`、`**/lifecycle.rs` 实测 glob 命中 0）。
  - **断点**：`crates/gpui_ohos/` **只消费了 colorMode、从没消费语言**——实测 `crates/gpui_ohos/src/ohos/platform.rs:556-562` 与 `crates/gpui_ohos/src/ohos/window.rs:1621-1657` 都只处理 color_mode；全 `crates/gpui_ohos/src` 搜 `language` / `preferred_locales` 无命中。
- **一个已定位的坑（`match_locale` 前缀歧义）**：`match_locale` 先精确匹配 id/alias，再按语言前缀匹配、且**要求前缀唯一命中**。若 OHOS 上报 **`zh-Hans-CN`**：精确匹配失败 → 前缀 `zh` 同时命中 `zh-CN` 与 `zh-TW` 两个 → **唯一性不成立 → 匹配失败回落英文**。（上报 `zh-Hans` 则命中 alias，正常。）OHOS 实际上报格式**未验证**，故该坑是否真实触发**未核实**。
- **结论**：要接入系统语言，需（a）让 `gpui_ohos` 消费 language 并通过桥交给 `localization`；（b）`localization` 目前**没有**接收系统语言的接口（`initialize` 内部硬编码 `sys_locale::get_locales()`，`InitRequest` 无该字段），需开口子；（c）处理 `match_locale` 歧义。**本阶段不做**（详见第 17.5 节）。

## 11.4 品牌词清洗

- 词表层含 `Zed` 的译文 **218 条**（基线；当前含补充词表为 **220** 条），另有 **17 条**具体品牌串（`zed.dev` **4** + `Zed 智能体` **9** + `Zed AI` **3** + `Zed Agent` **1**）。
- **需先定产品名再统一替换**（属产品命名决策，见第 14 章）。

## 11.5 11 个撞车文件（回注已执行；以后重跑仍需注意）

- **更新**：回注**已执行**。实测这 11 个文件里 **9 个被回注改动**，另 2 个未被改；**OHOS 移植标记全部原样保留**（实测 `git diff -U0` 删除行中含 `OHOS PORT` 的行数 = 0）。证据见 10.3(8)。
- **仍然要记住的**：以后重跑回注时，这批「我们改过 + 它要改」的交集文件**仍需注意**——它们的字节偏移会因我们的改动而失效（须重跑 extract），且工具的结构性补丁可能与我们手写的接线打架（见第 15 章第二道闸）。
- **11 个文件完整列出**（与第 4 章一致）：
  - `crates/agent_ui/src/conversation_view.rs`（被改）
  - `crates/editor/src/editor.rs`（被改）
  - `crates/git_ui/src/commit_view.rs`（被改）
  - `crates/gpui/src/window.rs`（未改）
  - `crates/settings_ui/src/pages/audio_test_window.rs`（被改）
  - `crates/settings_ui/src/settings_ui.rs`（被改）
  - `crates/terminal/src/terminal.rs`（未改）
  - `crates/terminal_view/src/terminal_panel.rs`（被改）
  - `crates/workspace/src/workspace.rs`（被改）
  - `crates/zed/src/main.rs`（被改）
  - `crates/zed/src/zed.rs`（被改）

## 11.6 三处待产品决策

1. **20 条枚举代号**（`Cpu4` / `Mem8` / `Disk256`）：保留英文，还是本地化成「4 核」/「8 GB」？
2. **5 条 `{file_label}` 量词**：源码里该占位符展开为英文 `File` / `Files`，中文句会中英混杂。
3. **品牌词**（本批 2 条 + 全量 218/220 条）：等统一改名方案。

## 11.7 `initialize` 的 `user_preference` 现由 `ui_locale` 设置项驱动（原为「硬编码」）

- **已完成**：不再硬编码。工具补齐了 `ui_locale` 设置项与语言选择器，`user_preference` 由它驱动。实测依据：
  - 设置项定义：`crates/settings_content/src/settings_content.rs:333` `pub ui_locale: Option<UiLocale>,`（类型 `UiLocale(pub String)`，默认 `"system"`，`settings_content.rs:368`）。
  - 语言选择器注册：`crates/settings_ui/src/page_data.rs:141`（`crate::components::locale_setting_item(),`）与 `crates/settings_ui/src/settings_ui.rs:556`（`.add_basic_renderer::<settings::UiLocale>(crate::components::render_locale_picker)`）。
  - 读取点：`crates/zed/src/zed/ui_locale.rs:8-11` 从 `SettingsStore::global(cx).raw_user_settings().content.ui_locale` 取 `user_preference`，再传给 `localization::initialize(...)`。
- **仍需做**：把默认值定为 `"zh-CN"`（见第 16 章）——因为 `sys-locale` 在 OHOS 上探测不到系统语言，默认 `"system"` 会回落英文。
