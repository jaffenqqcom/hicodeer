# 终端浮动控件设计（跳到底部 / 插入文件路径）

日期：2026-10-25 初版，2026-10-26 扩展并去平台门控
状态：已实现（release / debug 均编译通过，设备验证通过）

## 1. 目标

在终端右下角提供常驻的浮动控件，解决两类「键盘够不到」的操作：

1. **跳到底部**：滚回看历史后一键回到实时提示符
2. **插入文件路径**：调起系统文件选择框，把选中文件的全路径插到光标处

两者都必须是**通用能力**，不依赖某个平台：判断依据是「终端当前处于什么运行状态」，而不是「编译在哪个平台上」。

同时把按钮的样式、定位、事件拦截收敛成一个可复用接口，后续新增控件只需描述「图标 + 坐标 + 回调」。

## 2. 与初版的差异

初版（提交 `87bc62a`）只有一个跳底按钮，并且：

- 用 `#[cfg(target_env = "ohos")]` 门控，非 OHOS 平台不编译
- 只在「已离开底部」时显示
- 点击只清终端自身的 `display_offset`

扩展版去掉了这些限制，原因与代价见第 5 节「演进记录」。**现版本 `crates/terminal` 与 `crates/terminal_view` 内与本特性相关的门控为 0 处。**

## 3. 方案

### 3.1 浮动控件通用接口

```rust
/// 距视图右下角的距离，新增控件只需挑坐标、图标与回调。
struct FloatingButton {
    id: &'static str,
    icon: IconName,
    bottom: Pixels,
    right: Pixels,
}

impl TerminalView {
    fn render_floating_controls(&self, cx: &mut Context<Self>) -> Vec<AnyElement>;
    fn render_floating_button(
        &self,
        button: FloatingButton,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement;
}
```

- 每个按钮独立 `absolute`，用 `bottom` / `right` 定位，互不依赖排列顺序
- 元素 id 是 `(button.id, terminal.entity_id())`，分屏时多个终端互不串扰
- 样式（40px 圆形、`element_background` @ 0.7、`border` @ 0.4、`shadow_sm`、hover @ 0.85）与事件拦截集中在 `render_floating_button`，新增控件不再复制
- 回调以参数传入，因为不同控件要调不同的 `&mut self` 方法

当前两个控件：

| 控件 | id | 图标 | bottom | right |
| --- | --- | --- | --- | --- |
| 插入文件路径 | `terminal-insert-files` | `FolderOpen`（`assets/icons/folder_open.svg`） | 16px | 76px |
| 跳到底部 | `terminal-scroll-to-bottom` | `ArrowDown`（`assets/icons/arrow_down.svg`） | 16px | 28px |

间距 8px，从右往左排：28 → 76。

只在 `TerminalMode::Standalone` 渲染；`Embedded`（Agent panel 的只读输出）不出现，避免与面板自身的滚动控件重叠。

控件**常驻**，不再随 `scrolled_to_bottom()` 显隐：触摸设备没有 `shift-end`，全屏应用又始终让终端偏移为 0，两者都需要一个始终可点的入口。

### 3.2 跳到底部：单一路径

```rust
fn jump_to_bottom(&mut self, cx: &mut Context<Self>) {
    if let Ok(keystroke) = Keystroke::parse("end")
        && self.process_keystroke(&keystroke, cx)
    {
        cx.notify();
    }
}
```

没有 `ALT_SCREEN` / `MOUSE_MODE` 分支，依据是两条既有事实：

1. `Terminal::write_input`（`crates/terminal/src/terminal.rs`）里无条件排入 `InternalEvent::Scroll(Scroll::Bottom)` —— **任何按键输入都会让终端滚到底**。普通 shell 场景由此覆盖。
2. 全屏应用（herdr 等）跑在备用屏上，终端没有 scrollback、`display_offset` 恒为 0，只能由应用自己滚。它们把 `End` 当作「跳到最新输出」，收到 `\x1b[F` 即自行回底。

`End` 的字节由 `to_esc_str`（`crates/terminal/src/mappings/keys.rs`）决定：`APP_CURSOR` 下是 `\x1bOF`，否则 `\x1b[F`。

键盘动作 `terminal::ScrollToBottom`（`assets/keymaps/default-ohos.json` 中绑定 `shift-end`）与按钮共用 `jump_to_bottom`。

### 3.3 插入文件路径

```text
on_click → TerminalView::insert_picked_paths
         → cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true })
         → 路径经 shlex::try_quote 转义，", " 连接，末尾补一个空格
         → Terminal::paste(&text)
         → window.focus(终端) 收回焦点
```

- **走平台抽象**：`cx.prompt_for_paths` 是 `gpui` 的 `Platform` 接口，OHOS 侧映射为系统文件选择器并按 `multiple` 打开 `allow_many`，桌面侧沿用系统对话框。业务层不出现任何平台判断。
- **转义用 `shlex::try_quote`**：与仓库里「拖拽文件进终端」（`TerminalView::add_paths_to_terminal`）同一套逻辑，含空格的路径会得到单引号包裹，避免被 shell 拆成多个参数。
- **用 `paste` 而非 `input`**：应用开启 bracketed paste 时，路径会包在 `\x1b[200~ … \x1b[201~` 里，shell 不会把路径中的特殊字符当成编辑指令。
- **末尾空格**：插入后可以直接继续输入下一个参数。

### 3.4 消除一帧延迟

`TerminalView::render` 读的是 `terminal.last_content`，而它由 `Terminal::sync` 在 `TerminalElement::prepaint` 里重建；同一帧里 render 先于 prepaint，于是滚动后按钮/内容会晚一帧。

`prepaint` 在 sync 前后各读一次 `scrolled_to_bottom`，翻转时 `cx.notify(terminal_view_entity_id)` 补一帧。只在变化时触发，稳定后不再产生额外帧。

### 3.5 事件拦截

按钮浮在终端元素之上，二者是兄弟节点，终端元素的 `mouse_down` 监听仍会收到同一坐标的按下。若不处理，点按钮会顺带在终端最右下角开始一次选区。

因此每个浮动按钮都带：

- `on_mouse_down(Left, cx.stop_propagation())` —— 让 click 判定记下按下的同时截断，终端收不到按下
- `on_click` 内先 `cx.stop_propagation()` —— 终端收不到抬起，不会终结选区

日志验证过：点击按钮时 `Terminal::mouse_down` 不会被调用，鼠标事件没有泄漏给应用。

## 4. 关键实现位置

| 能力 | 位置 |
| --- | --- |
| 浮动控件接口 | `crates/terminal_view/src/terminal_view.rs`：`FloatingButton` / `render_floating_controls` / `render_floating_button` |
| 跳底统一入口 | 同文件 `jump_to_bottom`；动作处理 `scroll_to_bottom` 仅转调 |
| 插入文件路径 | 同文件 `insert_picked_paths` |
| 按键到字节的映射 | `crates/terminal/src/mappings/keys.rs`：`to_esc_str` |
| 输入即回底 | `crates/terminal/src/terminal.rs`：`write_input` 排 `Scroll::Bottom` |
| 一帧补刷 | `crates/terminal_view/src/terminal_element.rs`：`prepaint` 内 sync 前后比较 |

## 5. 演进记录（勿重蹈）

| 尝试 | 结果 | 结论 |
| --- | --- | --- |
| 点击只清 `display_offset` | 普通 shell 有效；herdr 完全无反应 | herdr 在备用屏，终端没有 scrollback 可回 |
| 判断 `ALT_SCREEN` 后直接 `cx.propagate()` | 同上，按钮点了什么都不发生 | 备用屏不是「无事可做」，而是「该由应用做」 |
| 注入 `Shift+End`（`\x1b[1;2F`） | herdr 能回底，但 pane 内的 agent 把参数 `1;2F` 当文本吃进输入框 | 带参数的 CSI 对下游解析器更脆弱，不能用于跨应用通用操作 |
| 注入无修饰 `End`（`\x1b[F`） | herdr 回底，且无字符残留 | 无参数的标准序列，下游不会误吃 |
| 按 `ALT_SCREEN && MOUSE_MODE` 分两支 | 能工作 | 多余：`write_input` 本身就会排 `Scroll::Bottom`，无需判断谁拥有视口 |
| 用 `Modes::contains(MOUSE_MODE)` | 条件恒为假 | `MOUSE_MODE` 是三个位的组合，应为 `intersects` |
| herdr 是否使用备用屏（靠搜 `EnterAlternateScreen`） | 误判为「不使用」 | 它调的是 `ratatui::init()`，该便捷函数内部会 `EnterAlternateScreen` |

## 6. 取舍

**按钮常驻而非按需显隐**：常驻后「已在底部」时点击是无操作（幂等），但换来两个场景的统一入口。滚动状态不再是显隐条件，也就不需要为它做补帧之外的额外处理。

**不用 `Tooltip`**：提示改为自绘浮层，跟随指针即时出现，避免 tooltip 请求每帧重建造成的闪烁。

**不新增动作命令**：跳底复用既有 `terminal::ScrollToBottom`，插入文件路径只挂在按钮上。前者已有键位绑定，后者是纯鼠标/触摸交互，暂不需要命令面板入口。

**坐标而非流式排列**：`FloatingButton` 用 `bottom`/`right` 而不是 `flex_row` + `gap`，新增控件不受既有控件的顺序与宽度影响。

## 7. 已知限制

- **普通 shell 的副作用**：`End` 会把命令行光标移到行尾。这与他手动按 End 的行为一致，属于该键的固有语义。
- **非 UTF-8 路径**：`shlex::try_quote` 需要 `&str`，`to_str()` 失败的路径会被跳过（静默丢弃）。
- **按钮遮挡**：停靠右下角，会盖住最后一行右侧约两格；浮动控件的固有代价。
- **`Embedded` 模式不显示**：Agent panel 的终端仍需用面板自身的滚动控件。
- **多指/长按**：沿用 `gpui_ohos` 的触摸判定，慢点（>220ms）不算点击。

## 8. 验证

- `./script/bundle-ohos`（debug）与 `--release` 均 `HAP BUILD SUCCESSFUL`，覆盖安装成功
- 普通终端：向上滚动后点按钮回到实时底部；`End` 注入同时把输入光标移到行尾（预期）
- herdr：备用屏 + 鼠标捕获，点按钮由 herdr 收到 `\x1b[F` 自行回底，输入框无字符残留
- 文件选择：系统选择器可多选，路径以 `a, b, c␠` 插入到光标处，含空格路径带引号
- 点击两个按钮都不会在终端里留下选区（`mouse_down` 未泄漏）
- 诊断日志（`[diag]`）已全部移除，正常路径无新增日志
