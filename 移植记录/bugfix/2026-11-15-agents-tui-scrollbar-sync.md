# Agents TUI 滚动条与内容不同步

## 问题描述

Agents TUI（`hicodeerd agent-tui` 客户端）运行在 HiCodeer 的中心终端里，滚动由 TUI 自己处理，滚动条 thumb 却由宿主绘制。两者之间只靠宿主向 agent server 轮询 `pane.list` 的 `scroll` 字段对齐。用户上下滚动时，thumb 明显落后于内容：起步要等半秒才开始动，滚动过程中一格一格地跳，停止时内容已停而 thumb 还要再走一段。本轮把这条链路从固定周期轮询改成事件驱动的按需查询，并补上平滑与吸附。

## 问题表现

- 用滚轮或触摸滑动后，终端内容已经滚了一段，滚动条 thumb 才开始移动，起步延迟接近半秒。
- 滚动过程中 thumb 呈阶梯式跳动（每秒只有约 20 个离散位置），不是连续移动。
- 内容停止后，thumb 还会继续移动一小段才停。
- 用键盘翻页（PgUp/PgDn、Ctrl+B/F）滚动时，thumb 完全不动。
- 空闲不滚动时，宿主每秒仍在向 agent server 发两次 `pane.list` 请求。

## 问题原因

### 1. 固定 500ms 轮询是唯一的对齐手段

`start_agent_scroll_poll` 每 500ms 拉一次服务端指标，thumb 位置完全由这次结果决定。TUI 滚动时不会通知宿主，所以 thumb 最多落后 500ms。一次轮询的链路是：新建 TCP 连接到 agent bridge → 发送控制 token → 发送 `pane.list` → 读一行 JSON，本机 loopback 也有几毫秒往返。

### 2. 事件只打开"窗口"，不唤醒轮询任务

改成按需后，滚动事件只把 `active_until` 推后，轮询任务本身还在空闲定时器里睡着，最多要等一个完整周期才醒来发现窗口打开了。这是"起步慢"的真正原因：事件已经发生、内容已经滚了，查询却还在等定时器。

### 3. 键盘翻页根本不走 `key_down`

`PgUp`/`PgDn` 在 `assets/keymaps/default-ohos.json` 里绑的是 `terminal::SendKeystroke`。gpui 的按键分发顺序是先匹配 keymap action、再由 `finish_dispatch_key_event` 调用 `key_down` 监听；绑定命中后 action 先执行，`key_down` 里加的触发点对翻页键不会生效。所以只在 `key_down` 里做触发，键盘翻页时窗口从未打开。

### 4. 离散跳变与平滑尾巴

50ms 一次的采样直接写到 thumb 位置上，一秒只有 20 个位置，观感是"一格一格"。加入缓动后，内容停止时 thumb 还在追最后一段距离，于是"停止慢"。

### 5. 停止判定依赖下一次轮询

判断"已经停止"只能靠连续两次查到的 offset 相同，所以发现停止至少要等一个采样周期；这之前缓动会继续走。

### 死路记录

- 直接把固定轮询从 500ms 提到 100ms：起步延迟降到 0.1s，但每秒 10 次请求常驻，用户明确不接受"空闲也高频查询"。
- 保留 500ms 空闲 tick 作为兜底：正是它造成了起步延迟。
- 把兜底降到 10s：仍然存在"事件来了但要等定时器"的问题，最后改为 channel 唤醒、彻底删掉兜底定时器。
- 曾判断"触摸惯性超过 600ms 会导致 thumb 尾部停更"：实测 `crates/gpui_ohos/src/ohos/window.rs` 的惯性动画每帧都会派发 `ScrollWheelEvent`，惯性期间事件持续存在，这个担心不成立，结论作废。

## 解决方案

### 1. 事件驱动的按需轮询

`crates/terminal_view/src/agent_chrome.rs` 里定义三个时间常量：活跃窗口 600ms、窗口内采样 50ms、每帧收敛系数 0.5。轮询任务的结构改成：

```rust
loop {
    let active = view.update(cx, |view, _| view.agent_scroll.poll_is_active())?;
    if !active {
        // 阻塞在唤醒通道上，没有任何定时器
        if wakeup_rx.next().await.is_none() { break; }
        continue;
    }
    let metrics = background_spawn(hooks.scroll).await;
    // 更新 metrics / settled
    timer(AGENT_SCROLL_ACTIVE_INTERVAL).await;
}
```

`note_agent_scroll_activity` 做两件事：把 `active_until` 推后 600ms，并递增 `postage::watch` 通道的值唤醒任务。空闲时任务真正睡在通道上，没有任何周期性请求，也没有兜底定时器。

触发点共五个，各一行：

- `TerminalView::scroll_wheel` —— 滚轮与触控板/触摸滑动
- 终端视图的左键 `on_mouse_down` —— 可能是拖动 thumb 的起点
- `TerminalView::key_down` —— 非字符输入（功能键、修饰键组合）
- `TerminalView::send_keystroke` —— keymap 绑定的翻页键实际走这里
- `drag_agent_scroll` —— 按住拖动中的每次移动

纯指针移动不触发：没有拖拽时 `drag_agent_scroll` 直接返回。

### 2. 平滑跟随与同步停止

poll 结果不再直接写 thumb 位置，而是写成目标；渲染前由 `advance_agent_scroll_animation` 每帧把显示值向目标推进剩余距离的 50%（60fps 下约 60ms 收敛），剩余不足 1 行时直接吸附。以下任一条件成立时跳过缓动、立即吸附：

- 正在拖拽 thumb（跟手）
- 最近两次 poll 报告的 offset 相同（内容已停）
- 活跃窗口已过期（没有输入了）

后两条合起来解决了"内容停了 thumb 还在走"：内容一停，下一次采样就能发现，thumb 在同一帧落位。

### 3. 状态收敛成一个结构体

原来 `TerminalView` 上平铺着 8 个 `agent_scroll_*` 字段，全部各自 `#[cfg(target_env = "ohos")]`。本轮收成 `AgentScrollState`（`metrics` / `drag` / `writer` / `poll_task` / `active_until` / `visual_offset` / `wakeup` / `settled`），`#[derive(Default)]`，`TerminalView` 只留一个字段；两个纯状态查询（`poll_is_active`、`visual_rows`）变成它的方法。顺带去掉渲染时每帧 `self.agent_scroll.clone()` 复制整个结构体（包含 `pane_id` 的 String）的问题，改为借用。

### 4. 日志

正常路径的 `log::info`（拖动开始/结束、右键菜单打开、重命名开始/结束）全部删除；只保留异常路径的 warn / error（写入失败、轮询失败、菜单调用失败、重命名失败）。

## 修改文件

- `crates/terminal_view/src/agent_chrome.rs` — 新增 `AgentScrollState` 与按需轮询：活跃窗口、channel 唤醒、平滑推进、稳定吸附；删除正常路径日志；渲染改为借用 metrics
- `crates/terminal_view/src/terminal_view.rs` — 8 个滚动字段收敛为一个 `AgentScrollState`；在 `scroll_wheel`、左键按下、`key_down`、`send_keystroke` 四处补触发；渲染前调用平滑推进

[[ohos-debug-lessons]]
