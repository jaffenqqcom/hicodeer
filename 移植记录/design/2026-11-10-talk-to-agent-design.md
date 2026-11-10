# Talk to Agent 对话框设计

- 日期：2026-11-10
- 状态：设计待确认（未开始编码）
- 范围：HiCodeer 编辑器右键菜单入口 + 独立对话框 + 发送到 Agent Panel

## 1. 结论摘要

- 入口：编辑器右键菜单第一项 `Talk to Agent`，快捷键 `Ctrl+Shift+Y`。
- 归属：**对话框与发送逻辑放 `crates/agent_panel`**；`crates/editor` 只增加一个门控取数入口与一个菜单项。
  `agent_panel` **不新增对外接口**，只消费 action 携带的数据。
- 覆盖 git 对比窗口（`text_diff_view` / `multi_diff_view` / `solo_diff_view`），左右两侧都能取到正确的内容与行号。
- 消息格式定稿见第 4 节。
- 新增 UI 文案全部走 i18n；不跑提取脚本，直接手工写入 `assets/locales/*.json`（13 个语种），先按语种翻译，等确认后再统一润色。
- 发送按钮**不置灰、不禁用**；不满足发送条件时点击给出提示，不关闭窗口。

## 2. 已确认的产品决策

1. 消息四要素（路径、框选行范围、原文、用户输入）的串联格式由本设计定稿。
2. 行号显示统一为 `Line x to Line y`；单行时显示 `Line x`。
3. 发送按钮需要 i18n；代码里写英文 key；按钮与文字不做灰化处理。
4. 功能主体放 agent panel，编辑器侧只保留最少的门控改动。
5. 必须覆盖“从 git 对比窗口发起”的场景。
6. 新增 UI 文本全部 i18n，先翻译、后确认。
7. 本设计文档落在 `移植记录/design/`。

## 3. 关键事实（本轮核查）

- 编辑器右键菜单构建点：`crates/editor/src/mouse_context_menu.rs:246`。菜单项是链式 `.action(label, action)`，首个 `.on_blur_subscription(...)` 之前就是“最前面”。
- git 提交对话框：`crates/git_ui/src/commit_modal.rs`。容器参数在 `:26-60`（50 字符宽、编辑区 300px、footer 24px、圆角 12、内边距 5）；底部左侧分支选择器在 `:398-436`（`Button::new(...).start_icon(IconName::GitBranch)` + `PopoverMenu`，anchor BottomLeft、offset y=-2）；右下提交按钮在 `:455-500`。
- 提交框构造：`crates/git_ui/src/git_panel.rs:1151` 的 `commit_message_editor`；字体来自 `git_commit_editor_style`（buffer 字体 + `git_commit_buffer_font_size`）；占位文案默认复用 key `Enter commit message`。
- 发送图标已存在：`crates/icons/src/icons.rs:261` 的 `Send`，对应 `assets/icons/send.svg`（16x16，线宽 1.2）。
- Agent 数据模型：`crates/agent_panel/src/snapshot.rs:94` 的 `Agent`（`pane_id` / `name` / `kind` / `focused` / `tab_id`）；`Space` 含 `id` / `label` / `focused`。
- 发送通道：`crates/agent_panel/src/client.rs:132` 的 `prompt_agent(pane_id, text)`（`agent.prompt`）。这是 agent panel 内部路径，不新增通道。
- **决定性事实**：`crates/editor/src/split.rs:2089` 的 `SplittableEditor::act_as_type` 在请求 `Editor` 类型时**固定返回 `rhs_editor`**。git 对比视图用的正是 `SplittableEditor`（`text_diff_view.rs:185`、`multi_diff_view.rs:212`、`solo_diff_view.rs:136`）。
  推论：如果由 agent panel 通过 `workspace.active_item(cx).act_as::<Editor>()` 取选区，用户在对比窗口**右键左侧（旧版本）时会拿到右侧编辑器**，路径、原文、行号全错。
- 对比视图没有设置自定义右键菜单（全仓 `set_custom_context_menu` 只出现在 `git_panel.rs:1179` 的提交框与 `agent_ui` / `collab_ui`），因此对比视图走默认菜单，新增项会自然出现。
- 坐标映射：`crates/multi_buffer/src/multi_buffer.rs:2008` 的 `text_anchor_for_position` 可把 MultiBuffer 坐标转成 `(Entity<Buffer>, Anchor)`；`Anchor::to_point(&buffer_snapshot)` 得到**文件内行号**。对比视图的 display 坐标是多 excerpt 合并行号，不能直接当文件行号用。
- 键位现状：`assets/keymaps/default-ohos.json` 的 `Editor` 段在 `:55`；`ctrl-shift-y` 在三份默认键位表（ohos / linux / windows）中均未被占用。
- **旧侧 buffer 无文件**：`SplittableEditor::split` 给左侧用的是 `BufferDiff::base_text_buffer()`（`crates/editor/src/split.rs:728-751`），该 buffer 由 `Buffer::local(base_text)` 创建（`crates/buffer_diff/src/buffer_diff.rs:1707`），没有 `file()`，自身解析不出绝对路径。
- **旧侧判定**：`crates/editor/src/split.rs:751` 只对左侧调用 `set_diff_hunk_action_target(Some(rhs_editor))`，右侧与普通编辑器均为 `None`（字段定义 `crates/editor/src/editor.rs:1183`）。因此 `diff_hunk_action_target.is_some()` 可作为“当前 Editor 是 diff 左侧”的判据，同时该 target 就是取路径的来源。

## 4. 消息格式（定稿）

### 4.1 模板

`````
<用户输入>

File: /absolute/path/to/file.rs (before changes)
Line 120 to Line 135
````rust
<选中文本原文>
````
`````

单行选区时，第二行变为 `Line 120`。

### 4.2 消歧规则

- **顺序**：用户输入在前（它是指令），引用信息在后（它是材料）。agent 先读到要做什么，再读到对什么做。
- **路径独立成行**：`File: ` 前缀 + 绝对路径。路径里允许有空格，不会有“路径和行号粘在一起”的歧义。
- **行范围独立成行**：`Line x to Line y`，1-based 闭区间，与编辑器状态栏一致。只有一行时写 `Line x`，不写 `Line x to Line x`。
- **原文用四个反引号围栏**：选中文本自身含四个连续反引号时不会截断代码块。围栏后跟语言名（取 buffer 的语言名，取不到则留空）。
- **原文原样保留**：不 trim 行首缩进、不补行号、不做语法高亮转换；仅统一末尾补一个换行。
- **用户输入 trim 首尾空白**后拼接；为空时不发送（见 9.2）。
- **对比窗口的语义**：行号是“被引用那一侧 buffer 的文件内行号”。旧侧内容可能已与磁盘不同，但原文随消息一起发出，接收方不会误读。
- **旧侧标注**：触发位置是 diff 的左侧（修改前）时，路径行末尾追加 `(before changes)`，形如 `File: /abs/path/file.rs (before changes)`。新侧与普通编辑器不加。该短语走 i18n key `(before changes)`，与其它新增文本同一套机制。
- **旧侧路径来源**：旧侧 buffer 是 `BufferDiff::base_text_buffer()` 用 `Buffer::local(base_text)` 造的无文件 buffer（`crates/buffer_diff/src/buffer_diff.rs:1707`），自身解析不出绝对路径。此时路径取自 `SplittableEditor` 为左侧保留的 `diff_hunk_action_target`（右侧编辑器，`crates/editor/src/split.rs:751`）。

### 4.3 示例（修改后的文件）

`````
把这段改成迭代器写法

File: /storage/Users/currentUser/workspace/hicodeer/crates/agent_panel/src/snapshot.rs
Line 94 to Line 110
````rust
pub(crate) struct Agent {
    pub id: String,
    ...
}
````
`````

## 5. 架构与数据流

```
[右键菜单项 / Ctrl+Shift+Y]
      -> dispatch agent::TalkToAgent（无参）
      -> Editor 元素级 handler（门控注册）
           读当前 Editor 实例的主选区：
             - MultiBuffer 原文（text_for_range）
             - text_anchor_for_position -> Buffer 内行号
             - 是否 diff 左侧：diff_hunk_action_target.is_some()
                 - 是：路径取该 target（右侧编辑器）的绝对路径，标记 is_before_change = true
                 - 否：路径取自 Buffer -> ProjectPath -> 绝对路径
           再 dispatch agent::SendSelectionToAgent{path, start, end, text, is_before_change}
      -> AgentPanel 注册的 workspace handler
           -> 打开 TalkToAgentModal（携带上述数据）
                -> space / agent 选择器（读 AgentPanel 快照）
                -> 消息输入框
                -> 发送：AgentPanel::send_message(pane_id, 拼好的消息)
                      -> 内部 client::prompt_agent -> herdr
                -> 成功：关闭
                -> 失败：留在框内显示错误
```

为什么取数必须留在 editor 内：

- 元素级 `register_action` 挂在**正在绘制的那个 Editor 元素**上（`crates/editor/src/element.rs:381` 的 `register_actions`），对比视图左右两侧各有自己的元素，右键/聚焦哪侧就由哪侧处理。
- 走 `workspace.active_item` 反查会被 `SplittableEditor::act_as_type` 固定导向右侧，已在第 3 节取证。

为什么 agent panel 不需要对外接口：

- 选区数据由 action 参数带入，agent panel 只做展示与发送。
- space/agent 列表与选中项来自 panel 自己的 `spaces` 快照（同 crate 内可见），无需 `pub` 数据接口。
- 发送调用 `client::prompt_agent`，同样同 crate 内可见。

## 6. 模块与文件改动

### 6.1 新增

- `crates/agent_panel/src/talk_to_agent_modal.rs`
  - `TalkToAgentModal`：`ModalView` + `Render`
  - `TalkToAgentContext`：`path` / `start_line` / `end_line` / `selected_text`
  - 两个选择器（space、agent）与发送逻辑
  - 文件头注释：`// OHOS-only: the Talk to Agent dialog for the embedded agent panel.`
- `crates/editor/src/ohos_talk_to_agent.rs`
  - `impl Editor { pub(crate) fn talk_to_agent(...) }`
  - 只做取数与再派发，不涉及 UI
  - 取数顺序：主选区 -> MultiBuffer 原文 -> Buffer 内行号 -> 绝对路径（旧侧走 `diff_hunk_action_target` 取右侧路径）
  - 旧侧判定只读 `diff_hunk_action_target.is_some()`，不修改 `split.rs` 的既有行为
  - 文件头注释：`// OHOS-only: collects the editor selection for the Talk to Agent dialog.`

### 6.2 修改（全部门控）

- `crates/zed_actions/src/lib.rs`
  - `pub mod agent` 内、`actions!` 宏**之外**新增三个独立 item：
    ```rust
    #[cfg(target_env = "ohos")]
    #[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
    #[action(namespace = agent)]
    pub struct TalkToAgent;

    #[cfg(target_env = "ohos")]
    #[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
    #[action(namespace = agent)]
    #[serde(deny_unknown_fields)]
    pub struct SendSelectionToAgent {
        pub file_path: String,
        pub start_line: u32,
        pub end_line: u32,
        pub selected_text: String,
        /// True when the selection came from the left (pre-change) side of a diff.
        pub is_before_change: bool,
    }

    #[cfg(target_env = "ohos")]
    #[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
    #[action(namespace = agent)]
    pub struct SendTalkToAgentMessage;
    ```
  - `SendTalkToAgentMessage` 对应 git 提交框的 `git::Commit`：对话框打开后按 `ctrl-enter` 发送。
  - 不放进 `actions!` 宏体（宏体内加 cfg 不可靠）。
- `crates/editor/src/editor.rs`
  - 顶部加一行门控模块声明：`#[cfg(target_env = "ohos")] mod ohos_talk_to_agent;`
- `crates/editor/src/element.rs`
  - `register_actions` 内加一行：`#[cfg(target_env = "ohos")] register_action(editor, window, Editor::talk_to_agent);`
- `crates/editor/src/mouse_context_menu.rs`
  - 在 `ContextMenu::build` 闭包内、`let builder = menu` 之前插入 shadowing 绑定（不改任何已有行）：
    ```rust
    // [OHOS PORT BEGIN] Talk to Agent is the first entry of the editor context menu.
    #[cfg(target_env = "ohos")]
    let menu = menu
        .action(
            localization::localized_str!("Talk to Agent"),
            Box::new(zed_actions::agent::TalkToAgent),
        )
        .separator();
    // [OHOS PORT END]
    ```
- `crates/agent_panel/src/agent_panel.rs`
  - `register()` 内新增 `SendSelectionToAgent` 的 workspace handler（打开对话框）。
  - 新增 `pub(crate)` 取选中项的辅助函数（crate 内可见即可，不对外）。
- `assets/keymaps/default-ohos.json`
  - `Editor` 段新增 `"ctrl-shift-y": "agent::TalkToAgent"`（编辑器内打开对话框）。
  - 新增一段，与 git 提交框的 `GitCommit > Editor && mode == auto_height` 完全同构：
    ```json
    {
      "context": "TalkToAgentModal > Editor && mode == auto_height",
      "bindings": {
        "escape": "menu::Cancel",
        "enter": "editor::Newline",
        "ctrl-enter": "agent::SendTalkToAgentMessage"
      }
    }
    ```
  - 只在 `mode == auto_height` 段绑定，是因为对话框左侧只读编辑器是 `Full` 模式；与 git 提交框只给 AutoHeight 输入框绑键的做法一致。

### 6.3 不改动

- `crates/git_ui/**`：不改提交对话框，只参照其样式。
- `crates/agent_panel/src/client.rs`：复用 `prompt_agent`，不新增协议。
- `crates/editor/src/split.rs`：不修改 `act_as_type` 的既有行为。

## 7. UI 规格

### 7.1 外壳

- 尺寸：与 git 提交框逐项对齐。宽 = `50 字符宽 + 2 × 5px`（字符宽算不出时回退 460px），在对话框构建时算一次（与 `CommitModal` 同一时机，焦点仍在编辑器上）；总高 = `2×5 + 2×8 + (300+3+6) + 8 + 24 + 1.5×行高`，中间区用 `flex_1` 吸收顶部信息行的占用。内边距 5；圆角 12；`min_h_112`；边框 1px `border`；底色 `elevated_surface_background`。
- 内层内容区：底色 `editor_background`、圆角 9、边框 `border_variant`、内边距 8、`gap_2`、纵向排列。
- 高度：顶部自适应 + 中间区固定 300px + footer 24px，不设整体固定高度。
- 不设标题栏：打开的对话框第一行内容就是路径，与 git 提交框一样没有 title bar。

### 7.2 顶部（路径与行范围）

- 两行文本，纵向排列，`w_full`：
  - 第一行：绝对路径，等宽字体，允许折行，不设高度；**不加 `File: ` 前缀**，直接显示路径本身。
  - 第二行：`Line 120 to Line 135`（单行选区为 `Line 120`），字号 `Small`、颜色 `Muted`。
- 不设 `h()` 或 `max_h()`；高度由换行后的实际高度决定。
- 文本使用 `Label` + 自动换行，不使用 Editor（不需要滚动与选中）。
- 旧侧（修改前）在顶部**不加任何标注**：`(before changes)` 只出现在发给 agent 的消息正文里。

### 7.3 中间左（只读原文）

- 宽度：`(中间区宽 - gap) * 3/5`；高度 `h_full()`；外层容器带 1px `border_variant` 边框与 6px 圆角。
- 构造：新建 `Buffer`（内容为选中文本）+ `MultiBuffer::singleton` + `Editor::new(EditorMode::Full, ...)`。
- 配置：`set_read_only(true)`、`set_show_gutter(false)`、`set_show_line_numbers(false)`、`set_soft_wrap_mode(SoftWrap::None)`、`set_use_modal_editing(true)`。
- 样式：与 git 提交框同一份 `EditorStyle` 规则（buffer 字体、`git_commit_buffer_font_size`、`editor_background`）。
- 滚动：改为自绘只读文本（`StyledText` + `whitespace_nowrap`）+ `Scrollbars::always_visible(ScrollAxes::Both)`，横纵滚动条在内容超出时出现、不自动淡出。
- 不做额外左 padding，保证“最左一行顶住左边缘，其余行保留相对缩进”。

### 7.4 中间右（消息输入）

- 宽度：`flex_1()`（即剩余 2/5）；高度 `h_full()`；外层容器同样带 1px `border_variant` 边框与 6px 圆角。
- 构造照 `commit_message_editor` 的序列：`EditorMode::AutoHeight { min_lines: 18, max_lines: Some(18) }`、`set_use_autoclose(false)`、`set_show_gutter(false)`、`set_use_modal_editing(true)`、`set_show_wrap_guides(false)`、`set_show_indent_guides(false)`、绑定 project 作为 collaboration hub。
- 占位文案复用已有 key `Enter commit message`（`zh-CN` 已有译文“输入提交消息”）。
- 打开对话框后立即聚焦该编辑器；新建编辑器选区在 offset 0，即“光标默认在最前面”。
- 字体与同一框内的 Send 按钮一致：UI 字体 + `0.75rem`（等同 `LabelSize::Small`）。

### 7.5 底部

- 高度 24px，`h_flex().justify_between()`，`gap_1`。
- 左侧两个选择器，横向排列，`gap_1`：
  - space 按钮：`Button::new(...).label_size(LabelSize::Small).start_icon(IconName::Agent)`，外层 `PopoverMenu`，`anchor(Anchor::BottomLeft)`、`offset(y = -2px)`。
  - agent 按钮：同构造，图标取 agent 类型对应的图标，取不到用 `IconName::Agent`。
  - 列表宽度 `rems(20)`、行高与选中底色与 `git_ui::branch_picker` 的 Popover 列表一致；点击即选中并关闭弹层。
  - 默认选中项：`Space::focused` / `Agent::focused`；若均无焦点，取第一个 space 的第一个 agent。
- 右侧：
  - `KeybindingHint`（Esc / Cancel），与 git 提交框一致。
  - 发送按钮：`ButtonLike::new("talk-to-agent-send")` + `layer(ElevationIndex::ModalSurface)` + `size(ButtonSize::Compact)` + `start_icon(Icon::new(IconName::Send).size(IconSize::XSmall))` + `Label::new("Send").size(LabelSize::Small)`。
  - 不调用 `.disabled()`，文字颜色用默认前景色；16px 的 `send.svg` 以 `XSmall`（12px）渲染，与 Compact 按钮的 12px 标签匹配。
  - 点击后进入 `sending` 状态：外观不变，重复点击被忽略；成功关闭，失败恢复并显示错误。

### 7.6 键盘上下文

- 对话框根元素 `key_context("TalkToAgentModal")`。
- 快捷键绑定条件写 `Editor && !TalkToAgentModal`，避免在对话框输入框内再次触发。
- git 提交框本身就有 `ctrl-enter` → `git::Commit`（`assets/keymaps/default-ohos.json:794`，context 为 `GitCommit > Editor && mode == auto_height`）。本对话框**完全照抄这套做法**，不做第二种行为：
  - 键位表新增 `TalkToAgentModal > Editor && mode == auto_height` 段，`ctrl-enter` → `agent::SendTalkToAgentMessage`。
  - `escape` → `menu::Cancel`，`enter` → `editor::Newline`（与 git 一致：回车换行，不发送）。
  - 对话框根元素用 `.on_action` 处理 `SendTalkToAgentMessage`，与 `CommitModal` 用 `.on_action(Self::on_commit)` 处理 `git::Commit` 同构。

## 8. 快捷键

- 选定：`ctrl-alt-z`（`Editor` 上下文）。
- 与终端快捷输入的 `terminal::QuickInput` 同键不冲突：两者分别绑在 `Editor` 与终端上下文，按焦点各自生效。
- `default-ohos.json` 里 `ctrl-alt-z` 在 `Editor` 段原本未占用。
- 对话框内部的 `ctrl-enter` 发送、`escape` 关闭、`enter` 换行见 7.6，均照 git 提交框的键位表写法。

## 9. 行为与失败处理

### 9.1 取数

- 主选区为空（只有光标）：仍然弹窗；行号取光标所在行，起始行与结束行相同，左侧预览为空。
- 多光标 / 跨文件：只取 `selections.newest_anchor()` 对应的主选区；其余忽略。
- 路径解析顺序：
  - 普通编辑器与 diff 右侧：`Buffer::project_path(cx)` + `Project::absolute_path`；失败则 `Buffer::file().as_local().abs_path(cx)`。
  - diff 左侧：自身 buffer 没有文件，改从 `diff_hunk_action_target` 的右侧编辑器取上述路径。
  - 两者都失败：顶部显示 buffer 标题（文件名），不臆造绝对路径；消息体里同样只放该标题。
- 无法解析 buffer（例如面板/非文件 buffer）：不弹窗。

### 9.2 发送

- 用户输入 trim 后为空：不发送，footer 上方提示 `Enter a message to send`。
- 没有可用 agent：提示 `Select an agent to continue`。
- 面板尚未拿到快照：提示 `The agent panel is not ready yet`。
- herdr 拒绝（例如 agent 处于 blocked）：走通用 `Failed to send: {error}`，把服务端返回的原因原样展示。
- 发送走 `cx.background_spawn`，避免 loopback 超时（2s 连接 / 3s 读）卡住 UI 线程。
- 成功后 `cx.emit(DismissEvent)`；失败保留窗口与输入内容。

### 9.3 关闭

- Esc、点击外部、Cancel：直接关闭，不发送，草稿不保存。

## 10. 不在本次范围

- blame 视图（`blame_ui.rs:611` 传自定义菜单）不出现该项。
- 终端选区、面板内文本不出现该项（只在 Editor 元素上注册）。
- 不保存消息草稿；不做历史记录。
- 不改动 herdr 协议与 `client.rs`。

## 11. i18n

不运行提取脚本，直接手工维护 `assets/locales/*.json`。每个新增 key 需同时写入 13 个语种文件（`cs-CZ`、`de-DE`、`es-ES`、`fr-FR`、`it-IT`、`ja-JP`、`ko-KR`、`pl-PL`、`pt-BR`、`ru-RU`、`tr-TR`、`zh-CN`、`zh-TW`）。

新增 key 清单：

- 无参数（`messages` 段）
  - `Talk to Agent`
  - `(before changes)`
  - `Send`
  - `Enter a message to send`
  - `Select an agent to continue`
  - `The agent panel is not ready yet`
  - `Switch space`
  - `Switch agent`
  - `Select a space`
  - `Select an agent`
  - `No items`
- 带参数（`formats` 段）
  - `File: {path}`（仅消息体使用，顶部不显示前缀）
  - `Line {start} to Line {end}`
  - `Line {line}`
  - `Failed to send: {error}`

复用已有 key：

- `Enter commit message`（占位文字，`zh-CN` 已是“输入提交消息”）。
- `Cancel`、`Close`（如用到）。

约定：

- 代码里只写英文 key，用 `localization::localized_str!` / `format_message`。
- 不跑 i18n 生成/提取脚本，直接编辑 13 个 JSON；加完后用 `grep -c` 逐个 key 核对 13 个文件都有条目。
- 第一轮由我按语种给出译文（不追求完美），确认后再统一润色。

## 12. 验证

1. `./script/bundle-ohos` 通过（EXIT=0，HAP BUILD SUCCESSFUL）。
2. 普通文件：选中 1 行 / 多行，右键首项可见，快捷键可开；顶部路径与 `Line x` 正确。
3. 对话框：左侧只读、可水平与垂直滚动；右侧占位文字正确、光标在首、输入后占位消失。
4. 选择器：默认选中值等于面板当前 space/agent；切换后发送目标随之变化；样式与 git 分支切换一致。
5. 发送：agent 收到包含“用户输入 + File + Line + 原文”的消息；窗口关闭。
6. git 对比窗口：分别右键左侧与右侧，路径正确、行号为该侧文件行号、原文为该侧内容；左侧发出的消息路径行带 `(before changes)`，右侧不带。
7. 键位：`Ctrl+Enter` 发送、`Enter` 换行不发送、`Esc` 关闭，与 git 提交框手感一致。
8. 异常：空输入、无 agent、blocked agent 分别给出对应提示且窗口不关。
9. i18n：15 个新增 key 在 13 个 locale 文件中都有一条（`grep -c` 逐个核对），界面切到中文时显示中文。

## 13. 已确认决策（2026-11-10）

1. 顶部第一行**不加** `File: ` 前缀，直接显示路径；消息体里保留 `File: `。
2. **不设**标题栏，与 git 提交框一致。
3. **加** `Ctrl+Enter` 发送，与 git 提交框完全一致：git 已有 `ctrl-enter` → `git::Commit`（`default-ohos.json:794`），本对话框在键位表新增 `TalkToAgentModal > Editor && mode == auto_height` 段绑 `agent::SendTalkToAgentMessage`；同时照抄 `escape` 关闭与 `enter` 换行。
4. diff 左侧触发时，消息体的路径行追加 `(before changes)`；顶部不显示该标注。

## 14. 实现状态（2026-11-11）

已落地。

新增文件：

- `crates/agent_panel/src/talk_to_agent_modal.rs` — 对话框、选择器、消息拼装、发送；文件内含 3 个单元测试。
- `crates/editor/src/ohos_talk_to_agent.rs` — 从当前 Editor 实例取选区、文件内行号、绝对路径与旧侧标记。

修改文件：

- `crates/zed_actions/src/lib.rs` — 三个门控 action：`TalkToAgent` / `SendSelectionToAgent` / `SendTalkToAgentMessage`。
- `crates/editor/src/editor.rs` — 门控 `mod ohos_talk_to_agent;`。
- `crates/editor/src/element.rs` — 门控注册 `Editor::talk_to_agent`。
- `crates/editor/src/mouse_context_menu.rs` — 菜单首项（始终出现；无选区时按光标行打开）。
- `crates/agent_panel/src/agent_panel.rs` — 门控 `mod talk_to_agent_modal;` 与 `SendSelectionToAgent` 的 workspace handler。
- `crates/agent_panel/Cargo.toml`、`Cargo.lock` — 新增 `language`、`theme_settings` 依赖。
- `assets/keymaps/default-ohos.json` — 编辑器内 `ctrl-shift-y`；对话框内 `TalkToAgentModal > Editor && mode == auto_height` 段。
- `assets/locales/*.json` — 13 个语种各新增 15 条。
- `crates/terminal_view/src/quick_input_ui.rs` — 终端快捷输入弹窗改用同一套尺寸（50 字符宽、400px 高）。

与设计的差异：

- 尺寸由最初的 100 字符宽改为与 git 提交框一致的 50 字符宽；无选区时不再隐藏菜单项，改为取光标行、预览留空。

- 左侧只读编辑器显式开启 `set_show_vertical_scrollbar(true)` 与 `set_show_horizontal_scrollbar(true)`，保证两个方向都有滚动条（设计只写了依赖默认）。
- 消息体围栏使用四个反引号加语言名，实测输出形如四个反引号后跟 `TOML`。

验证证据：

- `cargo check -p agent_panel`：0 error、0 新警告。
- `cargo test -p agent_panel talk_to_agent`：3 passed。
- `./script/bundle-ohos`：`=== HAP BUILD SUCCESSFUL ===`。
- `python3 script/check-i18n`：`i18n check passed: 3705 keys, all bundles complete`。
- `./script/check-keymaps`：通过。
- `./install-local.sh`：安装并启动成功，进程存活。
- 人工实测：编辑器右键出现 `Talk to Agent`；发送后 agent 收到中文格式消息（文件：… / 第 3 行到第 6 行 / 四反引号围栏）。

尺寸与交互的后续修正：

- 宽度改为构建时一次计算，避免每帧按不同焦点字体重算导致宽度漂移。
- 总高按 git 公式固定，顶部信息行从中间区扣除，因此整体高度与 git 弹窗一致。
- 去掉了焦点丢失即关闭对话框的逻辑：选择 space/agent 时弹层关闭会让焦点短暂离开输入框，原逻辑会连带关掉整个对话框。
- 左右两个文本框各加一层 `border_variant` 边框。
- 终端快捷输入改为同一尺寸，并把垂直位置从“窗口正中”改为与 modal layer 相同的“距顶 5rem、水平居中”。

仍需人工确认的交互点：

- space / agent 两个选择器切换后发送目标是否随之变化。
- git 对比窗口左右两侧分别触发时，行号与 `(before changes)` 是否符合预期。
## 15. 最终实现修正（2026-11-11）

尺寸：

- 宽度按 UI 字体的 `em_width × 50 + 2×5px` 计算（回退 460px），高度 = `2×5 + 2×8 + (300+3+6) + 8 + 24 + 1.5×UI 行高`，与 git 提交框同构。
- 终端快捷输入复用同一组常量与同一字体基准，两个弹窗尺寸一致。

字体：

- 左右文本框、git 提交框、快捷输入输入框统一使用编辑器缓冲区字体与 `buffer_font_size`（原先的 git 专属字号偏小）。
- 快捷输入的“直接执行”一行保持原字体。

滚动条：

- 根因：`EditorStyle.scrollbar_width` 默认值为 0，编辑器在绘制前直接返回，因此任何 `show_scrollbars` 设置都不会出现滚动条。
- 修复：对话框的 `EditorStyle` 设 `scrollbar_width: ui::EDITOR_SCROLLBAR_WIDTH`。
- 可见性：`Editor` 新增 OHOS 门控字段 `force_scrollbar_visibility`，`element.rs` 用 shadowing 叠加到原有 `ShowScrollbar` 判断上；内容超出即绘制，闲置仍按 Auto 淡出。
- 行数对称：两个编辑器都 `set_vertical_scroll_margin(0)`，去掉默认 2 行滚动余量，右侧不再提前滚屏。

左右文本框：

- 共用 `Editor` 与同一份 `EditorStyle`：左侧只读、`SoftWrap::None`、纵横滚动条；右侧可编辑、`SoftWrap::EditorWidth`、仅纵向滚动条。
- 左侧只读但可聚焦选中、可复制。

选择器：

- 弹层宽度 `7rem ~ 30rem`，按最长名字自适应，超长截断。
- 弹层有 `elevated_surface_background`、1px `border`、6px 圆角与阴影；`on_mouse_down_out` 触发 `DismissEvent`，点击其他位置自动收起。

快捷键提示：

- “取消”左侧固定显示 `Escape`：不再向键位表查询（`menu::Cancel` 同时绑了 `escape` 与 `ctrl-escape`，查询结果不稳定），改为用 `KeyBinding::from_keystrokes` 构造单个 Escape 按键。

快捷输入自动关闭：

- 打开时订阅输入框的 `on_focus_out`，切换面板/窗口导致失焦即关闭；
- 渲染时若窗口已失活也关闭（兜底），关闭时释放订阅。

日志：

- 新增代码只保留异常路径的 `warn`，正常路径无日志。