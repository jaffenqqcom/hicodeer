# hezed × herdr 融合设计

日期：2026-10-21
状态：设计待确认

## 1. 目标

把 herdr 的 server 端与 client 端并入 hicodeer：

- hicodeerd 一个二进制、一个进程，同时提供现有 SSH 命令服务与 herdr server。
- hicodeer 的主工作区变成一个 terminal，里面跑 herdr TUI。
- herdr 原来的左侧 spaces / agents 栏由 GPUI 重写，数据与操作经控制通道送到 herdr server。
- 编辑器退为标签页，与 TUI 平级切换。
- herdr 的 client/server 协议不改。

## 2. 总体架构

进程与模块：

```
hicodeerd (单进程，单 tokio runtime)
├── SSH command listener 4022
├── SSH management listener 4023
├── herdr server (in-process future)
│   ├── API socket   (UDS, 仅本进程/同 uid 可访问)
│   └── client socket(UDS, 给 TUI client)
└── control listener 127.0.0.1:4024  ← 新增，桥到 herdr API socket

hicodeer (GPUI 应用)
├── GPUI 左侧栏  ← 连 4024，读写 spaces / agents
├── terminal 面板 ← 里面跑 `hicodeerd herdr-client`
└── Git 面板 / 状态栏（原有）

`hicodeerd herdr-client` (子进程，同 uid)
└── herdr TUI client，连 client socket
```

数据流：

- GPUI 侧栏 → 4024 → herdr API → server 状态变更 → 事件广播 → GPUI 侧栏与 TUI 同时更新。
- TUI 按键 → client socket → herdr server → pane；GPUI 不参与。
- GPUI 侧栏点击 space/agent → 4024 → server 切焦点 → TUI 画面随之切换。

## 3. hicodeerd 内的融合

### 3.1 herdr 库化

herdr 目前是单 binary crate，`main.rs` 里写满 `mod`。改造：

- 新增 `src/lib.rs`，承载现有全部 `mod` 声明与常量。
- `src/main.rs` 只保留 `fn main()`，调用 `herdr::cli` 的分派。
- `Cargo.toml` 增加 `[lib] name = "herdr" path = "src/lib.rs"`。
- 内部 `crate::` 路径不变。

### 3.2 嵌入方式

把 `server::headless::run_server()` 拆成两段：

- `build(config) -> io::Result<HeadlessServer>`：同步部分（配置、API listener、App 构造）。
- `run(self) -> io::Result<()>`：原来的 `server.run().await`。

hicodeerd 在自己的 `block_on` 里：

```rust
let herdr = herdr::server::embedded::build(herdr_config)?;
select! {
    r = accept_command(...) => ...,
    r = accept_management(...) => ...,
    r = herdr.run() => ...,
}
```

三者共享同一个 runtime，不新建 runtime、不另起线程、不 daemonize。

### 3.3 生命周期

按已确认的"同生共死"：

- hicodeerd 启动即构造 herdr server。
- hicodeerd 退出或崩溃，进程内所有 PTY 随之销毁。
- 不做 `--handoff-import`，不做热迁移。
- 需要显式关闭：在 select! 任一路径返回时调用 herdr 的 shutdown，先杀 pane 进程组再退出，避免僵尸。

### 3.4 路径与配置

- herdr 的 socket 目录与数据目录通过 `HERDR_SOCKET_PATH` / session 目录显式指到 hicodeerd 可写目录，不依赖 `$HOME`。
- 配置来自 herdr 的 `config.toml`；OHOS 默认值写进部署目录，不改用户配置语义。
- herdr 的 `init_logging` 与 hicodeerd 日志合并：统一写 hicodeerd 已有的日志出口，不额外常驻写盘（遵守"日志不持续写磁盘"）。

## 4. 控制通道

### 4.1 为什么不接 client

herdr client 不保存业务状态，它只是把按键转成对 server 的请求。把控制通道接在 client 上，等于让 client 再做一层转发；接在 server 上可以复用 herdr 已公开的 JSON API，且 herdr 代码零改动。

### 4.2 形态

- hicodeerd 新增 `127.0.0.1:4024` TCP listener。
- 双向字节流桥接到 herdr 的 API UDS；不做协议翻译。
- 启动时生成一次性 token，hicodeer 经现有 4023 管理握手取得；无 token 的连接立即关闭。
- 只绑 loopback，不对外。

### 4.3 能力

GPUI 只用到 herdr API 的一个子集：

- `workspace.list` / `workspace.focus` / `workspace.create` / `workspace.rename` / `workspace.close`
- `agent.list` / `agent.focus` / `agent.prompt` / `agent.send_keys`
- `events.subscribe`（workspace_*、pane.agent_status_changed、pane.exited）

若后续要扩展，直接加 API 方法，不改通道。

## 5. Agent panel（GPUI）

- 独立 dock panel，默认停左；Project / Git / Outline 默认停右（智能体模式）。
- 经典模式下，Agent panel 与 Project / Git 两组位置互换。
- 宽度 224px，可折叠。
- 内容为 space / agent 树：每个 space 名下列出它的 agents，缩进一级。
- 状态与 herdr 一致，颜色与图标都取她的定义：
  - working 黄 ●
  - blocked 红 ●
  - done 青 ●
  - idle 绿 ○
  - unknown 灰 ·
- 底部沿用 herdr 侧栏的两个入口：
  - 左 `new`：新建 space
  - 右 `menu`：**暂缓**。先保持 herdr 原样，不在 GPUI 侧栏实现；后续再决定是复用 herdr 的全局菜单还是接 hicodeer 的 PopoverMenu
- 交互：点击切焦点、右键重命名/关闭、拖拽排序可选。
- 事件驱动更新，不轮询；连接断开显示"未连接"并退避重连。

## 5.1 中央 Agent 标签

- 中央 pane 的第一个标签固定为 `Agent`，内容是一个 terminal，里面跑 herdr TUI。
- 名称固定为 `Agent`，不可关闭，始终排第一；打开的文件排在它后面。
- 创建路径复用 `NewCenterTerminal`：`TerminalView::deploy` → `TerminalPanel::add_center_terminal` → `Workspace::add_item_to_active_pane`。
- 与普通 terminal item 的差异：`tab_content_text` 返回固定名称；关闭动作被拒绝；插入位置固定在首位。
- Agent panel 里的点击行为：选中某个 agent 后，聚焦中央 `Agent` 标签并让 herdr TUI 切到对应 pane。

## 5.2 i18n

- 所有新增 UI 字符串一律走 `localization::localized_str!` / `format_message`，禁止硬编码中文或英文。
- 新增词条同时补 `assets/locales/*.json`，至少中英文；已有词条优先复用。
- herdr 自带的 TUI 文案不纳入 hicodeer 的 i18n，保持 herdr 原样。

## 6. herdr client

- hicodeerd 增加隐藏子命令 `herdr-client`，内部调用 `client::run_client()`。
- 由 hicodeer 的 terminal 面板启动，启动时注入 socket 路径等环境变量。
- 配置侧栏默认隐藏：`sidebar_start_collapsed = true`、`sidebar_collapsed_mode = "hidden"`（宽度 0，不渲染，也没有展开箭头）。
- 侧栏隐藏后，底部的 `new` 入口由 Agent panel 接管；`menu` 暂缓，保持 herdr 原样。
- TUI 内部的 tab bar / pane 分屏保持原样，不接管。

## 7. OHOS 适配点

- **平台分支**：`aarch64-unknown-linux-ohos` 的 `target_os` 是 `linux`，herdr 的 linux 分支会命中，包括 zbus（D-Bus）、logind 等。需要给这些路径加 `not(target_env = "ohos")`，走 fallback。
- **PTY**：herdr 用 vendored `portable-pty`，其 openpty 路径要在 OHOS 上验证；必要时参照 `patches/rustix-openpty` 的做法补丁。
- **信号**：herdr 的 SIGHUP/SIGTERM/SIGWINCH 处理与 hicodeerd 现有处理合并，避免互相覆盖。
- **进程组**：pane 子进程必须以独立进程组启动，退出时整组回收。
- **路径**：socket、session、日志全部显式指定，不依赖 `$HOME` 与 `/tmp`。
- **编译**：`[patch.crates-io] portable-pty` 需要复制到 hicodeerd 的 workspace root，否则 path 依赖下补丁不生效。

## 8. 分阶段（已实施）

阶段 1：herdr 库化 + hicodeerd 内嵌 server + TUI client 子命令，先让 TUI 在 hicodeer terminal 里跑起来。

- 已落地：`herdr` 增加 `src/lib.rs`（`run_cli` / `run_client_entry` / `build_embedded` / `api_socket_path`），`src/main.rs` 退化为薄壳；`server/headless/embedded.rs` 提供同步 `build` 与异步 `run`；hicodeerd 在自己的 runtime 里 `select!` 驱动，并新增隐藏子命令 `herdr-client`。
- 已落地：`herdr-config.toml` 随 HNP 的 `conf/` 发布，hicodeerd 通过 `HERDR_CONFIG_PATH` 指向它，因此 herdr 自带侧栏默认收起且不画展开箭头。

阶段 2：控制通道 + GPUI 侧栏（spaces / agents 的读写与事件）。

- 已落地：hicodeerd 新增 4024 loopback 桥（`src/herdr_control.rs`），首行 token 校验后原样转发到 herdr API socket；端点与 token 经 4023 管理握手（`SshInfo.herdr`）下发，cmd-client 以 `herdr_control()` 暴露给引擎。
- 已落地：`crates/agent_panel` 按 herdr 的 `workspace.list` / `agent.list` 每 1s 刷新一次；space 名下缩进列出 agent，状态色与 herdr 一致。事件订阅（`events.subscribe`）暂未接入，属下一轮优化。

阶段 3：编辑器标签页整合、侧栏折叠与细节打磨。

- 已落地：`Item::is_closable`（默认 true）让固定标签页既不出关闭按钮也不被关闭动作摘掉；`TerminalView::deploy_agent_terminal` 以 `hicodeerd herdr-client` 建终端，标题固定为 "Agent"、插在首位。
- 已落地：布局预设把 Agent panel 纳入，智能体模式 Agent 停左、经典模式停右；新增文案全部走 `localized_str!`。

## 9. 风险

- OHOS 上 zbus / logind / portable-pty 的兼容性是最大不确定项，阶段 1 先做最小验证。
- 单进程下 herdr 崩溃会带走 hicodeerd，需要保证 herdr 的 panic 不扩散（catch_unwind 或隔离线程）。
- 控制通道是新增攻击面，token 与 loopback 绑定必须成立。
- 体积与编译时间增加，需在阶段 1 结束时量一次。

## 10. 验证

- 阶段 1：`hicodeerd` 启动后两个 SSH 端口与 herdr API/client socket 同时可用；terminal 里能跑起 TUI。
- 阶段 2：GPUI 点击 space，TUI 切到对应 workspace；agent 状态变化在两处同时出现。
- 收尾：hicodeerd 退出后无残留 pane 进程、无残留 socket。
