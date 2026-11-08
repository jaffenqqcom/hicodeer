# Agent TUI 里 Ctrl-B 被 herdr 默认 prefix 吞掉

## 问题描述

HiCodeer 把 herdr 作为编辑器内的 Agent TUI 运行（`crates/agent_panel` 的终端视图里跑的就是 herdr client）。herdr 的默认 prefix 键是 `Ctrl-B`（tmux 风格前缀），导致 Agent pane 里运行的 agent 收不到 `Ctrl-B`：这一下按键被 herdr 截走用于进入 prefix 模式，后续按键也被当成 herdr 快捷键解释。

## 问题表现

- 在 Agent TUI 里按 `Ctrl-B`，状态栏进入 `PREFIX` 模式，pane 里的 agent 没有任何反应。
- 想给 agent 发送 `0x02`（Ctrl-B）时无法做到，按一次只会切到 prefix 模式。
- 独立运行的 herdr 不受影响，问题只出现在「herdr 被当作编辑器内 Agent TUI」的形态。

## 问题原因

输入路由在 `herdr/src/client/shell/input.rs:679`。agent pane 处于 `ClientShellMode::Terminal` 时，按键按下面的顺序处理：

1. 查 direct binding，命中就执行动作；
2. 查 `matches_prefix(key)`，命中就切到 `ClientShellMode::Prefix` 并 return，按键被吞掉；
3. 两者都不命中，才返回 `ClientInputTarget::Pane`，由上层写入 pane 的 PTY。

`DEFAULT_PREFIX` 在 `herdr/src/config/keybinds.rs:14` 定义为 `(KeyCode::Char('b'), KeyModifiers::CONTROL)`。只要 prefix 是 `Ctrl-B`，第 2 步就必然先命中，Ctrl-B 永远到不了第 3 步。

这套 prefix 体系是为独立运行的 terminal workspace manager 设计的；嵌入编辑器后，快捷键入口已经由编辑器的 Agent panel 和状态栏提供，不需要 herdr 再占用 `Ctrl-B`。

## 解决方案

按「只删默认键」处理，保留用户显式配置的能力：

- `herdr/src/config/model.rs` 新增两个互斥的 `default_prefix_binding()`：非 OHOS 仍是 `ctrl+b`，OHOS 返回 `BindingConfig::empty()`，这样默认值在嵌入形态下为空。
- `parse_prefix_keys` 在 OHOS 下跳过空字符串条目，且当最终列表为空时不再回退到 `DEFAULT_PREFIX`，返回空列表而不是继续占用 `Ctrl-B`。
- 展示层同步：`primary_prefix_label`、`format_prefix_combos` 在 OHOS 且 prefix 为空时返回空串，状态栏和帮助面板不再显示 `ctrl+b`。

prefix 列表为空后，`matches_prefix` 永不命中，第 2 步直接跳过，`Ctrl-B` 落到 `ClientInputTarget::Pane`，以 `0x02` 写入 PTY。

已知副作用：默认情况下 herdr 自己的 `prefix+X` 快捷键（切 tab、复制模式、关 pane、设置）全部不可达。这是「删除该键」的直接结果，属于预期行为；用户在配置里显式写 `keys.prefix` 时仍然照常生效。

## 修改文件

- `herdr/src/config/model.rs` — 新增按平台互斥的 `default_prefix_binding()`，OHOS 默认 prefix 为空，`KeysConfig::default()` 改用它
- `herdr/src/config/keybinds.rs` — `parse_prefix_keys` 在 OHOS 下忽略空条目且不再回退默认前缀；`primary_prefix_label` 与 `format_prefix_combos` 在空 prefix 时返回空串
