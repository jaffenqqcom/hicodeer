# Agent TUI 启动时闪现 herdr 侧栏与占位帧

## 问题描述

打开编辑器的 Agents TUI（终端里运行 `hicodeerd agent-tui`，即内嵌的 herdr 客户端）时，画面上会先闪出一帧 herdr 自身的界面：整屏铺开，左侧是 spaces 列表、右侧是 agent 列表，随后才切到正常的 pane 内容。该现象在 11-04 的「Agent TUI 外围界面改由 HiCodeer 绘制」之后出现，用户怀疑与 11-08 的 Ctrl-B prefix 置空有关。

## 问题表现

- 每次新开 Agents TUI 都会闪一下，稳定复现，与 agent 是否在运行无关。
- 闪出的内容不是空白，而是 herdr 的展开态侧栏：space 列表与 agent 列表分列左右，几乎占满整屏。
- 修复第一处后仍会闪一下，只是不再出现列表，闪的是一帧底色与状态行。
- 独立运行的 herdr 表现为正常启动，无感知。

## 问题原因

### 与 Ctrl-B 改动无关

11-08 的 `3818dcd`（Ctrl-B prefix 置空）只改 `herdr/src/config/{model.rs,keybinds.rs}` 与默认键位，不触碰渲染路径。真正引入问题的是 11-04 的 `65f0a87`：它把 `crates/gpui_ohos/depend/cmd-agent/hicodeerd/conf/agent-config.toml` 的侧栏配置改成：

```toml
[ui]
sidebar_start_collapsed = true
sidebar_collapsed_mode = "hidden"
```

`hidden` 模式下折叠侧栏的宽度算成 0（`ClientShellConfig::layout`），而此前默认的 `compact` 模式宽度是 4，问题被一条窄边挡住，不明显。

### 第一处：占位帧把展开侧栏铺满整屏

客户端启动时有两段空窗：先收到 snapshot，后收到匹配的 pane surface（或反过来）。此时 `ClientShellState::compose` 会走 `compose_unavailable`。该函数先算侧栏矩形：

```rust
let sidebar = if layout.sidebar.width > 0 {
    layout.sidebar
} else {
    Rect::new(0, 1, cols, rows.saturating_sub(2))   // 整屏回退
};
```

侧栏宽度为 0 时落到整屏回退矩形，接着走 `endpoint_sidebar::render_expanded`，把展开态侧栏（spaces 左列 + agents 右列）画满整屏。这帧会被 `install_client_shell_snapshot` → `present_frozen_chrome` 立即写出，于是启动瞬间闪出一屏 herdr 界面。

### 第二处：占位帧本身仍会整屏刷新

第一处修掉后，占位帧仍会输出：整屏底色 + 顶部一行 "Local: … Select a connected machine." + mode bar。终端原本是空的，这一帧依然是一闪。根因是只要 snapshot 与 surface 不齐就无条件输出占位帧，而启动阶段这一帧没有任何信息价值。

## 解决方案

两处都按「启动阶段不画没有意义的帧」处理，且全部用 `target_env = "ohos"` 门控，非 OHOS 行为不变。

第一处，隐藏态不再绘制展开侧栏：

```rust
// [OHOS PORT BEGIN] ... 
#[cfg(target_env = "ohos")]
let render_expanded_sidebar = layout.sidebar.width > 0;
#[cfg(not(target_env = "ohos"))]
let render_expanded_sidebar = true;
// [OHOS PORT END]

if let Some(snapshot) = local_snapshot {
    render::render_sidebar(/* ... */);
} else if render_expanded_sidebar {
    super::endpoint_sidebar::render_expanded(/* ... */);
}
```

第二处，第一帧真实内容就绪前完全不输出帧：

```rust
if self.snapshot.is_none() || self.pane_surface.is_none() {
    #[cfg(target_env = "ohos")]
    if !self.has_composed_content_frame && self.endpoint_error.is_none() {
        return None;
    }
    return Some(self.compose_unavailable(cols, rows).into());
}
```

`has_composed_content_frame` 在 compose 通过 snapshot/surface 一致性校验、即将渲染真实内容时置位，因此「未呈现过真实帧」与「运行中断连」被区分开：启动阶段保持静默，断连时仍显示占位提示，不掩盖故障。

选择该方案的原因：闪帧来自客户端主动输出，最直接的办法就是不要输出；不改终端侧清屏逻辑，也不改 herdr 的连接与激活流程，改动面最小。

## 验证

- 新增单测 `startup_waits_for_real_content_instead_of_flashing_the_sidebar`（`herdr/src/client/shell/tests/chrome_context.rs`）：首帧必须 `None`；模拟断连后占位帧不得包含 " spaces" / " agents"，且 `hits.workspaces` 为空。
- `herdr/agent-tests/run-unit-tests.sh` 运行该单测通过（1 passed）。
- `./script/bundle-ohos` 构建成功，`./install-local.sh` 覆盖安装并重启应用；新拉起的 `hicodeerd agent-tui` 进程执行磁盘上的新二进制。

## 修改文件

- herdr/src/client/shell/composition.rs — 隐藏侧栏（宽度 0）时不再绘制展开态侧栏；snapshot/surface 未齐且尚未输出过真实帧时返回 None，启动阶段零输出
- herdr/src/client/shell/state.rs — 新增 OHOS 专属字段 `has_composed_content_frame`，记录是否已渲染过真实内容
- herdr/src/client/shell/tests/chrome_context.rs — 新增回归测试，覆盖启动静默与断连占位帧不含侧栏

## 备注

- 该 UI 位于 `hicodeerd agent-tui`（HNP 包内），覆盖安装 HAP 后新开的 TUI 才会用新二进制；daemon 本体不随覆盖安装重启。
- OHOS 专属行为已用 `target_env = "ohos"` 门控，其他平台编译与行为不变。

 [[ohos-debug-lessons]]
