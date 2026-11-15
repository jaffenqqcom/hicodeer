# Agents TUI 启动后不自动获得键盘焦点

## 问题描述

Agents TUI 是 workspace 里固定的中心终端 tab，运行 `hicodeerd agent-tui` 客户端。它由 Agent 面板（`crates/agent_panel`）在连接建立后自动部署，用户不经过任何点击就应该能直接打字。实际是要么不给焦点（首次部署），要么被面板的焦点处理抢走（点面板空白、按钮之后），必须再点一下终端才能输入。本次要把这两条路径都改成"焦点自动落在 TUI 上"，同时保证面板里真正需要输入的控件（就地重命名）仍能接收键盘。

## 问题表现

- 应用启动、Agent 面板首次同步完成后，Agents TUI 出现在中心区，但直接敲键盘没有任何反应，必须先点一下终端区域。
- 从其他 tab 切回 Agents TUI 是正常的：中心 tab 栏和面板里的 tab 行都会走 `activate_item(.., true, true)`，焦点会落到终端。
- 点击 Agent 面板的空白处、滚动条、新建/刷新按钮、折叠箭头之后，键盘输入进不了 TUI。表现为焦点指示消失、终端不接收字符，必须再点一次终端。
- 点击面板里的行（agent 行、space 行、tab 行）后反而正常，因为这些行的回调里会 `reveal_agent_terminal`。
- 点击就地重命名的输入框后输入正常，输入进的是那个编辑器而不是终端。
- 把应用切到后台再切回来，焦点保持切走前的位置：切走前在终端就还在终端，否则不会主动回到终端。

## 问题原因

### 1. 首次部署显式要求不聚焦

`TerminalView::deploy_agent_terminal` 把新终端插到活动 pane 的第 0 位时，第三个参数（`focus_item`）传的是 `false`：

```rust
// crates/terminal_view/src/terminal_view.rs（修改前）
workspace.add_item_to_active_pane(Box::new(view), Some(0), false, window, cx);
```

`Workspace::add_item_to_active_pane` 把 `focus_item` 原样透传给 `Pane::add_item`，最终 `Pane::activate_item` 里的 `if focus_item { self.focus_active_item(window, cx); }` 被跳过。于是 tab 插进来了、画面也切过去了，但键盘焦点还停在原处（启动时通常没有焦点）。

### 2. 面板根容器 track_focus 会在点击时主动抢焦点

面板的根元素同时挂了 `.id("agent-panel")`、`.key_context("AgentPanel")` 和 `.track_focus(&self.focus_handle)`（`crates/agent_panel/src/agent_panel.rs:3488` 附近）。在 gpui 里，`track_focus` 不只是"跟随焦点状态画样式"，它还会给该元素注册一个鼠标按下的默认行为：

```rust
// crates/gpui/src/elements/div.rs:2770 附近
if let Some(focus_handle) = self.tracked_focus_handle.clone() {
    window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble
            && hitbox.is_hovered(window)
            && !window.default_prevented()
        {
            window.focus(&focus_handle, cx);
            window.prevent_default();
        }
    });
}
```

所以点击面板内任何没有自己 `track_focus` 的地方（空白、滚动条、图标按钮、折叠箭头），焦点都会落到面板的 `focus_handle` 上。面板持有焦点意味着键盘事件进入面板的 key 上下文，而终端收不到任何字符。

行点击之所以正常，是因为行的 `on_click` 会调用 `focus_agent` / `focus_space` / `focus_tab_entry`，三者都会先 `reveal_agent_terminal`，再 `activate_item(&view, true, true)`，把焦点重新交给终端。

重命名输入框之所以正常，是因为编辑器的 `track_focus` 在更内层先触发，执行了 `window.focus(编辑器)` 并 `prevent_default()`，根容器的默认聚焦因此被跳过。

### 3. 窗口切回前台不会主动改焦点

OHOS 的 `GainedFocus` 只做三件事：把窗口置为 active、清掉 IME 的缓存决定、通知 `active_status_change`（`crates/gpui_ohos/src/ohos/window.rs:1557`）。GPUI 侧 `finish_frame` 也是用"上一帧的焦点路径"来判定焦点变化（`crates/gpui/src/window.rs:3441`），没有任何"重新选一个焦点"的逻辑。因此切前台只能回到切走前的位置，不会把焦点送给 TUI。

## 解决方案

### 1. 部署时聚焦终端（共享代码用互斥常量门控）

`deploy_agent_terminal` 是上游共享代码里新增的 OHOS 逻辑，但文件本身不在 OHOS 专属目录。按项目规则不能在共享代码里直接改行为，所以用一对互斥常量把两种行为分开：

```rust
// crates/terminal_view/src/terminal_view.rs
#[cfg(target_env = "ohos")]
const AGENT_TERMINAL_TAKES_FOCUS_ON_DEPLOY: bool = true;
#[cfg(not(target_env = "ohos"))]
const AGENT_TERMINAL_TAKES_FOCUS_ON_DEPLOY: bool = false;

workspace.add_item_to_active_pane(
    Box::new(view),
    Some(0),
    AGENT_TERMINAL_TAKES_FOCUS_ON_DEPLOY,
    window,
    cx,
);
```

选常量而不是在调用点写 `#[cfg]`，是因为 `cfg` 不能加在值位置（项目规则），互斥常量对两种目标都能编译，且读代码时一眼能看出这是平台差异。

副作用：`focus_item=true` 同时会把活动 tab 切到 Agents TUI。启动场景没有问题；但 daemon 断线重连触发重新部署时也会这样，可能把用户正在看的编辑器切走。这一点没有额外处理，属于已知取舍。

### 2. 面板点击后把焦点还给终端

只改 `crates/agent_panel`，不动 gpui：

- 新增 `return_focus_to_agent_terminal`：三个就地重命名编辑器（`rename_editor`、`agent_rename_editor`、`history_rename_editor`）任一存在时直接返回，否则调用已有的 `reveal_agent_terminal`。
- 在根容器上加 `.on_mouse_down(MouseButton::Left, ..)`，回调里用 `cx.defer_in` 延后一拍再判断。

```rust
.on_mouse_down(
    MouseButton::Left,
    cx.listener(|_this, _: &MouseDownEvent, window, cx| {
        cx.defer_in(window, move |this, window, cx| {
            this.return_focus_to_agent_terminal(window, cx);
        });
    }),
)
```

为什么延后一拍：同一次点击里可能先产生新的焦点对象。点重命名入口时 `begin_rename` 在 `on_click` 里创建编辑器并聚焦，如果鼠标按下就同步判断，会看到 `rename_editor` 仍是 `None` 而把焦点抢回终端；延后一拍两种顺序都能得到正确结果——如果编辑器已创建就不动，如果还没创建，随后 `begin_rename` 的 `editor.focus_handle(cx).focus(..)` 仍会覆盖回编辑器。

监听注册顺序也保证了这一点：`paint_mouse_listeners` 先注册 `track_focus` 的默认聚焦、再注册用户监听，所以我们的回调总能拿到"焦点已被面板抢走之后"的状态。

没有选择的方案：

- 去掉根容器的 `track_focus`：能治本，但面板会完全失去键盘上下文，将来若有面板级快捷键会失效，且改动面比"点完再还回去"大。
- 用 `window.query_accepts_text_input()` 判断"当前焦点是不是文本输入"：该 API 内部会 `cx.update(...)`，在鼠标回调里调用会重入窗口更新，不可用。改用"面板自己的三个重命名状态是否为空"这一判据，覆盖了当前面板内全部输入控件；TalkToAgentModal 是独立的 modal 层，不经过面板的鼠标监听。

## 修改文件

- `crates/terminal_view/src/terminal_view.rs` — 新增 `AGENT_TERMINAL_TAKES_FOCUS_ON_DEPLOY` 互斥常量（OHOS 为 true，其它平台为 false），部署 Agent 终端时用它作为 `focus_item`
- `crates/agent_panel/src/agent_panel.rs` — 新增 `return_focus_to_agent_terminal`，并在面板根容器加 `on_mouse_down`：点击后延后一拍，在没有就地重命名时把焦点交还 Agent 终端

[[ohos-debug-lessons]]
