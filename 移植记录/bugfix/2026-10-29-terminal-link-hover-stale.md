# 终端链接悬停状态在内容变化后不刷新（手型/下划线残留）

## 问题描述

终端里鼠标停在链接（URL 或文件路径）上会触发链接识别：文字变亮蓝色、加下划线、光标变手型（PointingHand）、弹出 tooltip。当鼠标**完全不动**、但终端因新输出而滚屏（或全屏 TUI 原地重绘）时，指针下的文字已经变了，链接识别却不会重新执行：手型光标与下划线会一直停留在原处，直到鼠标移动一下才恢复。

本仓的链接识别是「指针驱动」的（见提交 `3ee5b2f`），悬停探测、视图 hover 状态、下划线渲染三者分散在 `terminal` 与 `terminal_view` 两个 crate，本 bug 正是三处状态没有对齐导致的。

## 问题表现

- 鼠标停在链接上（手型 + 下划线出现），保持不动；
- 终端持续输出、画面向上滚屏，指针下的字符变成普通文本；
- 手型光标不消失，下划线/tooltip 也仍在（指向一段已经不是链接的文字）；
- 只有移动鼠标（哪怕 1px）才会重新识别并纠正；
- 反向情形同样存在：原地重绘把普通文本改成链接时，手型不会出现。

复现步骤：

1. 在一个会产生长输出的终端里（如 `for i in $(seq 1000); do echo ...; done`）先把指针停在一行链接文字上；
2. 不动鼠标，让输出继续滚动；
3. 观察指针下的文字与光标形状：文字已不是链接，光标仍是手型。

## 问题原因

三处原因叠加，缺一不可。

### 原因一：悬停状态被清空，但没有通知视图

`Terminal::sync()` 在内容变化判定为 `GridLinesChange::Changed` 时，`make_content()` 会把 `last_content.last_hovered_word` 置为 `None`，随后调用 `refresh_hovered_word()` 重新探测。

但视图侧的 `terminal_view.hover` 只会被 `Event::NewNavigationTarget(None)` 清除，而唯一发送该事件的 `clear_hyperlink()` 以「能 take 出 `Some`」为前提：

```rust
fn clear_hyperlink(&mut self, cx: &mut Context<Self>) {
    if self.last_content.last_hovered_word.take().is_some() {
        cx.emit(Event::NewNavigationTarget(None));
    }
}
```

走到这里时 `last_hovered_word` 已经是 `None`，`take()` 返回 `None`，事件永远发不出去。而手型是每帧按 `terminal_view.hover.is_some()` 绘制的（`terminal_element.rs` 的 `paint`），于是手型永久卡住。

### 原因二：复用判据只看行数与显示偏移，不看指针下的字符

`adjusted_last_hovered_word()`（`crates/terminal/src/alacritty.rs`）决定旧链接能否复用，判据只有三项：网格尺寸、`total_lines` 增量、`display_offset` 增量。

全屏 TUI（alt screen）与原地重绘（`\r` 进度条、重写同一行）这两种情况下，行数与偏移都不变 → 判为 `Unchanged` → 直接 `clone()` 旧链接继续用。旧链接的 `id` 与视图 hover 记录的 `id` 仍然相等，于是下划线与 tooltip 一起卡住。

```text
行数不变、偏移不变，但指针所在行的字符被重写
        │
        ├─ 旧逻辑：判 Unchanged → clone 旧 HoveredWord → 视图认为还是同一个链接
        └─ 期望：字符变了 → 判 Changed → 清 hover → 重探
```

### 原因三：内容变化触发的重探会被节流拦掉，且没有重试

`schedule_find_hyperlink()` 的节流条件是：

```rust
let distance_moved = (position - last_pos).abs() > FIND_HYPERLINK_THROTTLE_PX;
let time_elapsed = now - last_mouse_move_time > FIND_HYPERLINK_THROTTLE;
let throttle = !(distance_moved || time_elapsed);
```

指针不动时位移恒为 0；若距上次成功探测不足 100ms，这次「内容变化」触发的重探会被直接 `return`，且没有任何后续重试。输出停止后不会再有 `sync()`，指针下明明是链接，手型却一直缺失。

### 被证伪的假设（死胡同，勿重复走）

最初判断是「终端底部跟随输出滚屏时，`total_lines` 与 `display_offset` 同步增加，导致 `grid_visible_lines_changed == false`，从而错误复用」。

实测不成立：alacritty 的 `Grid::scroll_up` 只在 `display_offset != 0` 时才自增偏移。用户在底部时 `display_offset` 恒为 0 → `total_lines` 增加、偏移不变 → 两个增量不等 → 既有代码**已经**判 `Changed`。

「两个增量相等」恰好是用户往回滚看历史、视口内容确实没变的情形，那里的平移复用是正确的，不能删。仓库既有测试（`crates/terminal/src/terminal.rs` 的 `test_hover_with_changing_content` 系列）也把这条断言写死了。

## 解决方案

核心思路：**复用旧链接的前提是「指针覆盖的字符没变」，而不是「行数没变」**；同时内容变化触发的重探必须保证最终一定执行一次。

### 1. 用上一帧渲染的单元格判断内容是否真的变了

新增 `hover_cells_match()`（`crates/terminal/src/alacritty.rs`）：拿 `last_content.cells`（上一帧渲染出的可见单元格）与当前 grid 在链接范围内逐字符比对。

不比对 `HoveredWord.word` 的原因：OSC 8 链接的 `word` 存的是 URI，不是屏幕上显示的文字，比它无法发现「显示文字被改写」。

```rust
// 修改前：没有任何内容比对
let grid_lines_change = if grid_size_changed || grid_visible_lines_changed {
    GridLinesChange::Changed
} else {
    GridLinesChange::Unchanged
};

// 修改后：内容被原地改写也算 Changed
let hovered_content_changed = !grid_size_changed
    && total_lines_delta == 0
    && last_content
        .last_hovered_word
        .as_ref()
        .is_some_and(|word| !hover_cells_match(grid, last_content, &word.word_match));

let grid_lines_change =
    if grid_size_changed || grid_visible_lines_changed || hovered_content_changed {
        GridLinesChange::Changed
    } else {
        GridLinesChange::Unchanged
    };
```

`hover_cells_match` 只遍历链接覆盖的格子（`partition_point` 二分定位起点 + `take_while` 到终点），不是整屏扫描 —— 一屏约 200×60 格，直接全扫会比链接长度多两个数量级。同时保留 `topmost_line..=bottommost_line` 的越界保护，避免历史被裁剪后索引 panic。

### 2. 内容变化清空悬停时，同步通知视图

```rust
// sync() 内
let had_hover_before = self.last_content.last_hovered_word.is_some();
self.last_content = make_content(&terminal, &self.last_content);
if self.last_content.grid_lines_change == GridLinesChange::Changed {
    debug_assert!(self.last_content.last_hovered_word.is_none());
    if had_hover_before {
        cx.emit(Event::NewNavigationTarget(None));
    }
    self.refresh_hovered_word(window, cx);
    ...
}
```

视图收到 `None` 后立刻清 `hover`，手型与下划线在下一帧消失；若重探发现新链接，随后发出的 `Some(...)` 会在同一帧内覆盖回来。

### 3. 内容变化的重探只受时间闸约束，被挡则延迟重试

`schedule_find_hyperlink()` 改为返回 `bool`（是否真的入队），并在节流判据中放行「内容变化」这一次的距离闸：

```rust
let throttle = throttle
    && !(reprobe_after_content_change
        && now.duration_since(self.last_mouse_move_time) > FIND_HYPERLINK_THROTTLE);
```

`refresh_hovered_word()` 在探测没有入队时，安排一次延迟重试：

```rust
fn refresh_hovered_word(&mut self, window: &Window, cx: &mut Context<Self>) {
    if self.schedule_find_hyperlink(window.mouse_position(), false, cx) {
        return;
    }
    self.schedule_hyperlink_reprobe(window, cx);
}
```

`schedule_hyperlink_reprobe()` 用一个 `bool`（`hover_retry_pending`）保证同一时刻只有一个待执行的重试；定时器到期后清掉 `last_hyperlink_search_position` 再探一次，这样即使指针不动，闸门也必然打开。重试位置取 `window.mouse_position()`，即使用户期间移动过鼠标也不会用旧坐标。

保留时间闸的原因：路径候选探测包含文件系统存在性检查，输出刷屏时不能每帧都做，100ms 一次是可接受的上限。

**为什么不用其他方案**
- 只清 hover 不重探：手型会消失，但指针下仍是链接时无法恢复，出现「该有手型却没有」的反向 bug。
- 内容变化完全跳过节流：输出密集时每帧都会跑文件系统检查，代价过高。
- 给 `GridLinesChange` 新增第三种枚举值：该枚举还被其他渲染路径消费，扩大语义会影响无关逻辑，改为在判定处并入即可。

## 验证

- 新增 3 个单元测试（`crates/terminal/src/terminal.rs` 测试模块）：
  - `test_hover_invalidated_when_row_is_rewritten_in_place`：原地用 `\x1b[1;1H\x1b[K` 重写指针所在行，行数与偏移都不变；断言 `hovered_word` 归零，且确实发出 1 次 `NewNavigationTarget(None)`。
  - `test_hover_reprobed_after_content_change_with_stationary_pointer`：把指针下的内容换成另一条链接并 `sync`，断言重探出新链接。
  - `test_hover_reprobed_after_throttled_content_change`：时间闸关闭时重探被丢弃，断言 `hover_retry_pending` 为真；推进测试时钟后断言重探成功且标志被清。
- 反向验证：临时退掉两处修复（不并入 `hovered_content_changed`、节流不放行），两个用例分别失败于 `left: Some("https://zed.dev/") right: None`，证明用例确实卡在「复用旧链接」上。
- `RUSTUP_TOOLCHAIN=1.97.1 cargo check -p terminal -p terminal_view --target aarch64-unknown-linux-ohos`：0 error。
- `bash script/bundle-ohos` → `=== HAP BUILD SUCCESSFUL ===`；`bash install-local.sh` 安装并启动成功。

## 修改文件

- `crates/terminal/src/alacritty.rs` — 新增 `hover_cells_match()`（按链接范围二分定位后逐字符比对上一帧单元格）；`adjusted_last_hovered_word()` 把「指针下内容变化」并入 `Changed` 判据，保留行数/偏移相等时的平移复用。
- `crates/terminal/src/terminal.rs` — `sync()` 内容变化时若此前有悬停链接则补发 `Event::NewNavigationTarget(None)`；`schedule_find_hyperlink()` 返回是否入队并放行内容变化探测的距离闸；新增 `schedule_hyperlink_reprobe()` 与字段 `hover_retry_pending` 做延迟重试；`refresh_hovered_word()` 串联重试；新增 3 个单元测试。
- `crates/terminal/src/alacritty/hyperlinks.rs` — 删除未使用的 `info` 与 `Instant` 导入（清理编译警告，非本 bug 修复）。
- `crates/terminal_view/src/terminal_path_like_target.rs` — 删除 `open_path_like_target()` / `possibly_open_target()` 未使用的 `terminal_view` 参数（清理编译警告，非本 bug 修复）。
- `crates/terminal_view/src/terminal_view.rs` — 配合上一行的调用点调整。

> 说明：本次改动是通用链接行为，**未加** `cfg(target_env = "ohos")` 门控，全平台一致生效（与提交 `3ee5b2f` 的去门控方向一致）。
