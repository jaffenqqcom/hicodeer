# Agent panel 快捷键被终端绑定遮蔽：tooltip 时有时无且按键不生效

## 问题描述

Agent panel 在状态栏（dock）的按钮 tooltip 有时只显示标题「Agent Panel」，不显示快捷键；同时 `Ctrl-Shift-A` 在焦点位于 Agent 终端时不触发面板开关。问题不是一直存在，取决于当前焦点所在的位置。

## 问题表现

- Agent panel 与 project panel 都关闭时，鼠标悬停状态栏的 Agent panel 按钮，tooltip 只有标题，没有键名。
- project panel 打开、Agent panel 关闭时，同一个按钮的 tooltip 又带上了快捷键。
- 焦点在 Agent 终端里按 `Ctrl-Shift-A` 不生效，既不开也不关面板。
- 对比：project panel 的按钮 tooltip 一直稳定显示 `Ctrl-Shift-E`，不会时有时无。

## 问题原因

根因是**同一个按键在不同 context 下被重复绑定，而 Zed 的 keymap 按 context 深度优先、外层的绑定会被更深层的同键绑定遮蔽**。

- `assets/keymaps/default-ohos.json:497` 在 `Workspace` 段把 `ctrl-shift-a` 绑给 `agent_panel::ToggleFocus`。
- 同文件 946 行在 `Terminal` 段把同一个 `ctrl-shift-a` 绑给 `editor::SelectAll`。

Agent panel 的 pane 本身就是终端，焦点落进去时 context 栈里 `Terminal` 比 `Workspace` 深，`ctrl-shift-a` 的最高优先级绑定是 `editor::SelectAll`。

这一点同时影响两处：

1. **按键不生效**：dispatch 时命中的是 `editor::SelectAll`，`agent_panel::ToggleFocus` 根本收不到。
2. **tooltip 不显示键名**：tooltip 走 `Tooltip::for_action`（`crates/workspace/src/dock.rs:1649`），最终到 `KeyBinding::resolve_keystrokes`（`crates/ui/src/components/keybinding.rs:196`）→ `Window::highest_precedence_binding_for_action`。该函数内部用 `binding_matches_predicate_and_not_shadowed`（`crates/gpui/src/key_dispatch.rs:432`）检查：只有「该键位在当前 context 栈下的最高优先级绑定就是目标 action」才算数，被遮蔽的绑定直接跳过。

所以焦点在终端里时，Agent panel 那条绑定被判 shadowed，tooltip 退化成纯标题；焦点在 project panel 这类没有占用 `ctrl-shift-a` 的 context 时不被遮蔽，tooltip 又恢复正常。

project panel 稳定显示的原因是对比出来的：它的 `ctrl-shift-e` 在 Terminal、Editor 等段都没有别的绑定，不存在遮蔽。

## 解决方案

改用未被任何 context 占用的组合，不动机制、也不动终端的快捷键。

先枚举内置 keymap 中 `ctrl-shift-*` 的占位：a/b/c/e/f/g/h/i/j/k/l/m/n/o/p/r/s/t/u/v/w/x/z 与 0、5 已占用；空着的是 `ctrl-shift-d`、`ctrl-shift-q`、`ctrl-shift-y` 以及数字 1/2/3/4/6/7/8/9。

最终把 Agent panel 的开关改为 `ctrl-shift-d`（`assets/keymaps/default-ohos.json:497`）。该组合不属于任何 context，终端里也不存在同键绑定，因此不会再被遮蔽，tooltip 稳定显示、按键在终端焦点下也生效。

排查过程中走过两条死路，均已回退：

- 先改 `crates/workspace/src/dock.rs`，在 OHOS 下绕过 `Tooltip::for_action`、直接从全局 keymap 取键名拼进标题。这属于改动既有 tooltip 机制，被否并回退——问题根因不在展示层。
- 再改 `assets/keymaps/default-ohos.json` 的 Terminal 段，把 `ctrl-shift-a` 从 `editor::SelectAll` 改给 Agent panel。虽然能让快捷键生效，但会让终端失去 Ctrl-Shift-A 全选，且仍是「抢键」思路，已回退。

## 修改文件

- `assets/keymaps/default-ohos.json` — `Workspace` 段的 Agent panel 开关从 `ctrl-shift-a` 改为 `ctrl-shift-d`，避开 Terminal 段被 `editor::SelectAll` 遮蔽的问题
