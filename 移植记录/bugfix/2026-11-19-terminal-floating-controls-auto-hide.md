# 终端浮动快捷按钮按内容自动隐藏

## 问题描述

Standalone 终端右下角常驻三个浮动圆钮（Quick Input 闪电 / Insert Files 文件夹 / Bottom 箭头），是 HiCodeer 自加的定制功能（git blame 为 `04b17c8f`、`d3554f65`，非上游 Zed 代码）。它们以 `absolute` 定位浮在 `TerminalElement` 之上、始终显示。当终端输出（命令、长行、全屏 TUI 状态栏）刚好写到右下角那几行时，按钮会把文字盖住，用户需要的内容看不全。本轮让这三个按钮作为一个整体按其覆盖区域是否有输出自动隐藏：区域有输出则整组消失，无输出则恢复显示；隐藏期间快捷键照常可用。

## 问题表现

- 三个圆钮固定在终端右下角（`bottom: 16px`、`right: 28/76/124px`、直径 `40px`），无论下面有没有内容都显示。
- 长命令、长输出行或全屏 TUI 的底部状态栏延伸到右下角时，被圆钮遮挡，字符读不到。
- 圆钮组的覆盖区约从右边缘内 `28~164px`、底边缘内 `16~56px`，正好是终端右下角最常用的落字区域之一。

## 问题原因

### 1. 按钮是无条件渲染的 overlay

`render_floating_controls` 原样返回三个按钮的 `Vec<AnyElement>`，`render()` 里通过 `.children(floating_controls)` 直接挂到视图根部。渲染路径里没有任何"下方是否有内容"的判断，所以按钮恒显。

### 2. 按钮与终端内容是两套坐标系

按钮用 GPUI 的 `absolute` + `right`/`bottom`（距视图右下角的像素距离）；终端内容用网格坐标（`cell.point.line` / `cell.point.column`）。要判断"按钮覆盖区有没有输出"，必须把两边的坐标系统一到像素空间：`x = column * cell_width`、`y = (line + display_offset) * line_height`，再和按钮覆盖矩形相交测试。

其中 `point.line` 是受 scrollback 影响的网格行号（向上滚动时可为负），与屏幕行的换算是 `screen_row = point.line + display_offset`，这与 `terminal_element.rs` 里 `DisplayCursor::from` 的既有换算一致。

### 3. 关键结构：按钮列表原本是内联的

三个按钮原本内联写在 `render_floating_controls` 里。如果隐藏逻辑另起一份"按钮区域清单"，以后新增第四、第五个按钮就必须改两处，极易漏改导致新按钮不被隐藏判定覆盖。

## 解决方案

### 1. 按钮定义成为唯一来源

把内联的按钮 `vec![...]` 提取成 `floating_control_specs()`，每项为 `(FloatingButton, Rc<dyn Fn>)`——几何（`bottom`/`right`）、图标、tooltip、action、点击回调绑在一起。`render_floating_controls` 改为遍历这个列表渲染。渲染和隐藏判定都读同一份定义，新增按钮只需在这一处加一项，隐藏区域自动覆盖。

### 2. 覆盖区域从按钮列表自动派生

`floating_controls_footprint_from` 遍历按钮列表，取所有 `right`/`bottom` 的最小与最大值，加上圆钮直径 `FLOATING_CONTROL_SIZE`（原硬编码 `px(40.)` 提为常量），得到整组占用的矩形（以距右/底边缘的距离表示）。直径与底距都提为命名常量，按钮渲染与区域计算共用，消除魔法数字。

### 3. 内容相交检测

`floating_controls_region_occupied` 把覆盖矩形换算成以终端左上角为原点的像素范围，遍历 `terminal.last_content.cells`：

```rust
let screen_row = cell.point.line + display_offset;
if screen_row < 0 || screen_row >= screen_lines { return false; }
let x = cell.point.column as f32 * cell_width;
let y = screen_row as f32 * line_height;
x + cell_width > region_left
    && x < region_right
    && y + line_height > region_top
    && cell.cell.character() != ' '
```

任一可见 cell 的非空格字符落入该区域即判定占用。非 `Standalone` 模式、cell 尺寸非法、空列表直接返回 false（不隐藏）。

### 4. 隐藏按钮但保留快捷键

`render()` 里：

```rust
// before
let floating_controls = self.render_floating_controls(cx);
// after
let floating_controls = if self.floating_controls_region_occupied(cx) {
    Vec::new()
} else {
    self.render_floating_controls(cx)
};
```

整组按钮由同一个 `Vec` 控制，一起显示、一起消失。快捷键注册在视图根部的 `.on_action`（`QuickInput` / `InsertFilePaths` / `ScrollToBottom` 等）与按钮渲染是两条独立链路，一行未动——所以隐藏期间 `Ctrl+Alt+Z` 等快捷键照常工作。三个按钮作为整体隐藏，不出现只缺一个的中间态。

### 5. 单测

新增 `test_floating_controls_footprint_tracks_button_list`：构造两个按钮，往列表里再加一个更靠左的按钮，断言派生出的 `left_from_right` 自动从 `164px` 扩大到 `212px`，证明"新增按钮无需改判定逻辑、隐藏区域自动扩展"。

### 门控说明

三个浮动按钮整段是 HiCodeer 自加的无门控定制功能，本轮是在这段定制代码内部做增强，不涉及上游 Zed 代码，也不改变其他平台的功能（改动对所有构建目标一致生效），故不引入 `#[cfg(target_env = "ohos")]` 门控。此判断在方案确认时已获用户认可。

## 修改文件

- `crates/terminal_view/src/terminal_view.rs` — 把三个浮动按钮内联列表提取为 `floating_control_specs()` 作为唯一来源；新增 `FloatingControlsFootprint` 与 `floating_controls_footprint_from()` 从按钮列表自动派生覆盖矩形；新增 `floating_controls_region_occupied()` 做 cell 与覆盖区的像素相交检测；`render()` 按检测结果整组隐藏/显示按钮（快捷键 `.on_action` 不变）；直径与底距提为命名常量；新增区域随按钮列表自动扩展的单测

## 验证状态

- debug 构建中 `terminal_view` 编译通过（`Finished dev profile`），本改动无编译错误。
- 后续的 hicodeerd 构建与 HAP 打包未在本机跑完：被并行 agent 的 `cargo test -p terminal_view` 长时间持有构建目录锁阻塞，打包步骤未产出最终 HAP；应待锁释放后重跑 `./script/bundle-ohos` 完成端到端验证。
- 新增单测 `test_floating_controls_footprint_tracks_button_list` 需在锁释放后单独跑一次确认。

[[ohos-debug-lessons]]
