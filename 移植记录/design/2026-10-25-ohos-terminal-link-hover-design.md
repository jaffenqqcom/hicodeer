# 终端链接悬停与 Ctrl 点击打开设计

日期：2026-10-25
状态：已实现（编译通过，设备验证通过）
提交：b521569

## 1. 目标

终端里的链接（URL、文件/目录路径、OSC 8 超链接）在小屏触控/指针混合设备上的交互，对齐 Warp：

- 指针进入链接任意位置，立刻出现手型光标、链接下划线和提示气泡，不需要按任何键
- 提示只显示一句 `Ctrl+Click to open`，走 i18n
- 指针在同一个链接内移动时，提示不消失、不闪断
- 指针移出链接（含移出终端）后，手型恢复、下划线和提示同时立刻消失
- 不按 Ctrl 的点击或拖动仍然是选词/拖选/复制，不能误打开
- 不提供长按打开
- 下划线出现时文字同时变成终端调色板里的亮蓝色，便于在长路径中辨认
- Ctrl+点击目录时交给系统文件管理器打开，不再切到 hicodeer 项目面板（所有平台一致）

## 2. 现状与约束

### 2.1 上游已有的链路

链接能力分三段，都是现成的：

- 检测：`crates/terminal/src/alacritty/hyperlinks.rs`，OSC 8 从单元格取，URL 用正则，路径用正则（含 `path:line:col`）
- 分发：`Event::Open` 在 `crates/terminal_view/src/terminal_view.rs` 里分派到 `cx.open_url` 或 `open_path_like_target`
- 打开：OHOS 侧的 `open_url`、`reveal_path`、`open_with_system` 已在 `crates/gpui_ohos/src/ohos/platform.rs` 实现

缺的只是「指针悬停」这一段的触发条件，以及提示的呈现方式。

### 2.2 三个关键约束

**（1）上游的悬停一律要求修饰键**

`crates/terminal/src/terminal.rs:2614` 原本是 `!modifiers.secondary()` 直接放弃探测；`crates/terminal_view/src/terminal_element.rs` 里光标与提示也都以 `window.modifiers().secondary()` 为前提。指针设备平时不会按住 Ctrl，所以悬停永远不会发生。

**（2）探测有节流，且「跳过」等于「保持旧状态」**

`schedule_find_hyperlink` 在 5px / 100ms 内不重复探测。节流跳过时，上一次的 `last_hovered_word` 与 `TerminalView::hover` 都保持不变。结果是：指针移出链接的那次移动被吞掉后，手型和下划线会一直留着，直到下一次足够大的移动。

**（3）提示若用 gpui tooltip，生命周期绑在鼠标移动上**

`div().tooltip(...)` 的请求会在指针每次移动时重新评估并重建。指针在同一个链接内移动，提示会被拆掉再排队，表现为「一动就消失，移出再移入才出现」。上游能接受，是因为桌面端本来就是 Ctrl+悬停的短暂查看。

## 3. 方案

### 3.1 悬停探测不再要求修饰键

`crates/terminal/src/terminal.rs:2614`：

```rust
let wants_link_probe = cfg!(target_env = "ohos") || modifiers.secondary();
```

非 OHOS 保持上游「Ctrl+悬停」不变。

### 3.2 OHOS 下取消探测节流

`crates/terminal/src/terminal.rs:2652`：

```rust
#[cfg(target_env = "ohos")]
let throttle = false;
```

探测本身只扫描指针所在的一行，代价可忽略；换来的是「进入即显示、离开即清除」的确定性。这也顺带解释了此前「必须按一下 Ctrl 才出现」：进入链接的那次移动被节流吞掉，之后没有新的移动事件，直到修饰键变化触发重绘才补上。

### 3.3 下划线、提示、光标共用同一个悬停目标

`crates/terminal_view/src/terminal_element.rs:1435` 起，OHOS 分支直接以 `TerminalView::hover` 为准，不再比对链接 id：

```rust
if hover_probe_enabled
    && bounds.contains(&window.mouse_position())
    && let Some(registered_hover) = self.terminal_view.read(cx).hover.as_ref()
{
    (
        Some(registered_hover.tooltip.clone()),
        Some(registered_hover.hovered_word.word_match),
    )
}
```

上游的 id 比对是为了防止旧 tooltip 残留，但同一链接内移动时 `last_hovered_word` 会短暂换 id，导致下划线与提示闪断。改用「hover 目标本身」后，三者同源、同帧消失。

**判定「在链接上」的语义仍由 `hover` 决定**：只有 `resolve_open_target` 成功（URL 可解析、路径真实存在）才会设置 `hover`，因此指向普通文字或不存在路径时不会误显示手型。

### 3.4 提示改为终端自绘浮层

`crates/terminal_view/src/terminal_element.rs:1493` 起：不再注册 tooltip，而是构造一个小 `div`，在指针右下方 12px、16px 处 `prepaint_as_root`，并在 paint 末尾随其他浮层一起 `paint`。

- 内容只有 `localization::localized_str!("Ctrl+Click to open")`，不显示目标地址
- 越界时翻到指针左上
- 因为不参与 tooltip 生命周期，指针在链接内移动时不会被重建
- 浮层只在 `hover` 存在且指针在终端区域内时构造，移出即不再绘制

非 OHOS 分支保留原样的 `Tooltip::text(...)`。

### 3.5 光标

`crates/terminal_view/src/terminal_element.rs:1799`：

```rust
let cursor_over_link = (cfg!(target_env = "ohos") || window.modifiers().secondary())
    && bounds.contains(&window.mouse_position())
    && self.terminal_view.read(cx).hover.is_some();
```

`bounds.contains` 保证指针移出终端时不残留手型。

### 3.6 打开仍然必须按 Ctrl

`crates/terminal/src/terminal.rs:2760` 的 `mouse_down` 条件没有放开，仍是 `MouseButton::Left && e.modifiers.secondary()`；因此普通单击、拖动、双击选词全部保持原行为。

路径类目标另有一处约束：`crates/terminal_view/src/terminal_path_like_target.rs:118` 原本在 `hover` 为空时直接放弃打开。指针可能在异步解析完成前就按下，OHOS 下跳过这条守卫，文件是否存在仍由 `resolve_open_target` 校验。

### 3.7 悬停文字改为亮蓝色

`crates/terminal_view/src/terminal_element.rs:1307` 的 `link_style` 原来只设置下划线、不改文字颜色（避免长路径染色后难读）。现在悬停命中链接时，文字与下划线一起变色：

```rust
color: Some(theme.colors().terminal_ansi_bright_blue),
```

取色用终端调色板的 bright blue，而不是固定的暗蓝：深色主题下是 `#3b9eff` 这类高亮蓝，在 `terminal_ansi_background` 上对比度足够，也跟随用户切换的终端主题。下划线仍用 `link_text_hover`。

### 3.8 目录交给系统文件管理器

`crates/terminal_view/src/terminal_path_like_target.rs:209` 在 `open_resolved_target` 之前分流：只要目标是目录，就直接调用 gpui 的平台抽象 `cx.reveal_path(&path)`，不再经 workspace 打开（之前会激活项目面板）。所有平台一致，不按 OHOS 门控。

`reveal_path` 是 `App::reveal_path` → `Platform::reveal_path`，OHOS 实现落在 `crates/gpui_ohos/src/ohos/platform.rs:766`，内部转成 `reveal_in_file_manager`。终端侧不出现任何 `@ohos.*` 或文件管理器专用调用，具体用哪个文件管理器由各平台的 `Platform::reveal_path` 实现决定。

## 4. 取舍

**为什么不用 gpui tooltip 加 0 延迟**

0 延迟只解决「出现慢」，解决不了「移动即重建」。实测提示仍会在链接内移动时闪断，因此必须绕开 tooltip 生命周期。

**为什么改节流而不是加几何判定**

几何判定需要在 paint 里把像素换算成网格点并与 `word_match` 比较，多一处坐标系耦合；取消节流直接让既有状态机每个移动事件都跑一遍，行为更直白，代价是一行正则扫描。

**为什么 OSC 8 的超链接点击不加 Ctrl 限制**

OSC 8 是程序显式标记的链接（单元格自带 URI），上游语义就是单击打开，且不参与「悬停提示」这条路径。保持原样，避免破坏终端协议行为。

**已知限制**

文件路径类链接的手型要等异步磁盘校验完成才出现（URL 类无此延迟）。这是路径存在性校验的固有成本，实测可接受，不额外优化。

## 5. 门控位置

- `crates/terminal/src/terminal.rs:2611` — 悬停探测不要求修饰键
- `crates/terminal/src/terminal.rs:2647` — 取消探测节流
- `crates/terminal_view/src/terminal_element.rs:1438` — 以 hover 目标直接驱动下划线与提示
- `crates/terminal_view/src/terminal_element.rs:1488` — 自绘提示浮层
- `crates/terminal_view/src/terminal_element.rs:1792` — 指针悬停显示手型
- `crates/terminal_view/src/terminal_path_like_target.rs:118` — 打开路径不依赖已解析的 hover
- `crates/terminal_view/src/terminal_element.rs:1307` — 悬停链接文字改为亮蓝色
- `crates/terminal_view/src/terminal_path_like_target.rs:209` — 目录交给系统文件管理器（全平台）

前六项均为 `cfg!(target_env = "ohos")` 或 `#[cfg(target_env = "ohos")]`，macOS / Linux 仍走「Ctrl+悬停 + gpui tooltip」。文字颜色与目录分流两项不做 OHOS 门控：前者是终端链接的通用视觉，后者对所有平台都改用 `Platform::reveal_path`。

## 6. 验证记录

设备实测（`./script/bundle-ohos` → `--hap-only` → `install-local.sh` 覆盖安装）：

- 指针移入 URL、文件名、目录名：立即出现手型、亮蓝色文字、下划线与提示，无需按键
- 在链接内移动：提示保持显示
- 移出链接、移出终端：文字颜色、下划线与提示同时消失，光标恢复 I 形
- Ctrl+点击：URL 进浏览器，文件在编辑器中打开并跳到指定行，目录在系统文件管理器中打开
- 普通单击、拖动、双击：选词与复制正常
