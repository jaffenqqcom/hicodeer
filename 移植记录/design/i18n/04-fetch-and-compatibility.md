# 第 4 章 拉取与适配评估

## 4.1 拉取命令（浅克隆）

```sh
git clone --depth 1 --progress https://github.com/LI-NA/zed-i18n.git /storage/Users/currentUser/tmp/zed-i18n
```

结果（实测）：

- 成功。
- 体积：`/storage/Users/currentUser/tmp/zed-i18n` 约 **20 M**（其中 `.git` 约 3.7 M）。
- tag：**`v1.21.0-i18n.1`**。
- 最后提交：**`c45ba0f`**，日期 **2026-09-25**，主题 `feat: update Zed to v1.21.0`。

## 4.2 版本绑定（原文）

`tmp/zed-i18n/config/project.toml` 原文：

```toml
zed_version = "v1.21.0"
zed_commit = "33c95853ed2b6956f339733c63a8220964ecbeb6"
zed_repository = "https://github.com/zed-industries/zed"
cache_dir = ".cache/zed"
```

- **它钉死上游 `v1.21.0`**，而我们当时是 **`1.23.0`**，差两档。
- 这个版本差是后面「必须重提取」的根本原因（见第 5 章）。

## 4.3 规模（1.21 基线）

- 覆盖源文件：**463 个**（实测：1.21 baseline manifest 的 occurrence 涉及的 `file` 去重 = 463）。
- 覆盖 crate：**约 110 个**（实测：1.23 树为 110；1.21 baseline 为 109。对话记录为 110，以实测为准，标注差异 1 个）。
- zh-CN 词表：**7316 条**（实测：`tmp/zed-i18n-baseline-1.21/translations/zh-CN.json` = 7316，与记录一致）。
  - **重要差异**：撰写时 `tmp/zed-i18n/translations/zh-CN.json` 已变为 **7428** 条（= 7316 + 112 补充词表），判定为并行 worker 已把补充词表合并进主词表（见第 10 章第 2 步）。故「7316」是 1.21 基线 / 合并前的数字，「7428」是合并后的现状。
- 语言数：**13 种**（见第 3 章 3.3.3）。

## 4.4 与我们的改动冲突评估（撞车清单）

判定手段（三种取**并集**）：

1. 路径含 `ohos`。
2. 文件内容含 OHOS 移植标记 / `cfg(target_env = "ohos")`。
3. git 提交改动清单。

**撞车的 11 个文件（完整列出）**：

- `crates/agent_ui/src/conversation_view.rs`
- `crates/editor/src/editor.rs`
- `crates/git_ui/src/commit_view.rs`
- `crates/gpui/src/window.rs`
- `crates/settings_ui/src/pages/audio_test_window.rs`
- `crates/settings_ui/src/settings_ui.rs`
- `crates/terminal/src/terminal.rs`
- `crates/terminal_view/src/terminal_panel.rs`
- `crates/workspace/src/workspace.rs`
- `crates/zed/src/main.rs`
- `crates/zed/src/zed.rs`

结论：

- 撞车 **11 / 463 = 2.38%**（11 除以 1.21 的 463 个覆盖文件）。
- 实测复核：这 11 个文件**全部**出现在 1.23 重提取结果的文件集合中（11 / 11 命中）。
- OHOS 专属文件（`crates/gpui_ohos/**` 约 73 个）与它**零重叠**——都是**新增文件**，不会与它的回注冲突。
  - 实测佐证：1.23 重提取结果里，`file` 含 `ohos` 的条目为 **0 条**。

## 4.5 品牌词规模

- zh-CN 词表里**含拉丁 `Zed` 的译文 218 条**（对话记录）。
  - 实测：当前 `translations/zh-CN.json` 译文中含 `Zed` 的为 **220 条**（= 218 基线 + 2 条来自补充词表，见第 8 章）。其余品牌串实测与记录一致：`zed.dev` **4** 条、`Zed 智能体` **9** 条、`Zed AI` **3** 条、`Zed Agent` **1** 条。
- 与「交付物不得含 Zed 字样」**冲突**，**必须清洗**。
- 替换目标属**产品命名决策**（改成什么名字尚未定），在决策落定前不得擅自替换（见第 11 章遗留问题）。
