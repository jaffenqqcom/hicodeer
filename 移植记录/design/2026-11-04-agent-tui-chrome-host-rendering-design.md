# Agent TUI 的 Tab 栏、右键菜单、滚动条改由 HiCodeer 绘制

- 日期：2026-11-04
- 状态：**已实现，含第二至六轮实机反馈与检视修复**（2026-11-04 开发完成并经主人首肯）。`cargo check --lib`（herdr）、`cargo check -p agent_panel -p terminal_view`、`bash ./script/bundle-ohos`（`EXIT=0` + `HAP BUILD SUCCESSFUL`）三项编译判据均通过；改动尚未提交，为工作区改动（共 36 个文件：35 个代码/配置/资源文件 + 本文，见 §11）。`./install-local.sh` 装包已执行，主人随后重启了 hicodeerd daemon（实测新映像已生效，见 §11 P11）；daemon 侧新 API 已实测可用。第二轮修复见 §11 P12（装包后仍需再次重启 daemon），第三轮滚动条几何见 §11 P13，第四轮拖动/拇指长度/gutter 见 §11 P14，第五轮拖动跟手见 §11 P15，第六轮按 claude 只读检视结论全量修复并补 13 语言 i18n 见 §11 P16。第三至六轮的宿主侧改动重启应用即生效；P16 另改了 herdr 侧（`mouse.rs` / `panes.rs` / `client_views.rs`），需再重启一次 daemon 才生效。实机目视验收（§8 的 8 条）**未验证**，需主人在设备上完成。开发后核对结论见 §9，实现与本文的偏差见 §11。
- 范围：
  - `herdr/`（vendored 上游 agent server 与 TUI 客户端）
  - `crates/gpui_ohos/depend/cmd-agent/hicodeerd/conf/agent-config.toml`
  - `crates/agent_panel/src/`（`snapshot.rs`、`client.rs`、`agent_panel.rs`）
  - `crates/terminal_view/src/terminal_view.rs`
- 结论：**让 herdr TUI 客户端停止绘制 tab 栏、右键菜单、pane 滚动条，把这三样的状态与动作改走 herdr 已有的 JSON API，由 HiCodeer 侧用 GPUI 绘制**。pane 内容仍由 herdr 渲染，不在两个进程之间传像素。

## 1. 背景：现在是谁在画

Agent 面板中间的显示区不是 GPUI 视图，而是一个独立进程往终端里写字符格：

- `crates/agent_panel/src/agent_panel.rs:942` 调 `TerminalView::deploy_agent_terminal()`，在中央固定标签页里起一个 pty。
- `crates/terminal_view/src/terminal_view.rs:296` 的 `deploy_agent_terminal` 把命令拼成 `hicodeerd agent-tui --socket <path>`，并设 `pinned_title`；`is_agent_terminal()`（`terminal_view.rs:365`）以 pinned title 判定。
- `crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs:244` 的 `agent-tui` 分支调 `agent_server::run_client_entry()`，即 herdr 的 TUI 客户端。
- 客户端用 crossterm 画完整字符界面，HiCodeer 只负责把它的画面显示出来。三处绘制点：
  - tab 栏：`herdr/src/client/shell/tabs.rs:7` 的 `render_tab_bar`，由 `herdr/src/client/shell/render.rs:313` 在 `render_shell` 里调用。
  - 右键菜单：`herdr/src/client/shell/context_menu.rs:4` 的 `items()` 生成清单、`:145` 的 `open_pane_context_menu` 开菜单、`:358` 的 `activate_pane_context_action` 执行。
  - pane 滚动条：`herdr/src/ui/scrollbar.rs`，受配置 `herdr/src/config/model.rs:978` 的 `pane_scrollbars` 控制。
- 下发配置 `crates/gpui_ohos/depend/cmd-agent/hicodeerd/conf/agent-config.toml` 已把 server 自带侧栏彻底隐藏（`sidebar_start_collapsed = true`、`sidebar_collapsed_mode = "hidden"`），但第 16-18 行明确写着 "The tab row stays"，tab 栏仍由 herdr 画。

宿主侧的现存链路：`crates/agent_panel/src/client.rs:474` 的 `call()` 每次开一条短连接，经 daemon 的 token 守护 loopback 桥（127.0.0.1:40240）转发到 herdr 的 API UDS；面板 `REFRESH_INTERVAL = 1000ms`（`crates/agent_panel/src/agent_panel.rs:60`）轮询 `workspace.list` + `agent.list`。

## 2. 需求与最终口径（多轮讨论定稿）

三项需求：

1. **Tab 栏**：TUI 里不再显示 tab 栏；tab 的展示与操作移到 Agent 面板。
2. **右键菜单**：主显示区右键的 6 个选项（Rename pane 等），显示什么、点击响应都由 herdr 决定；显示改由 HiCodeer 按终端右键菜单风格绘制。
3. **滚动条**：右侧滚动条由 HiCodeer 按终端滚动条风格显示。

逐条定稿口径（与最初描述有差异处已按最后确认更正）：

- **面板条目规则**：space 下的条目是**平级的两类**，不引入 tab 层、也不引入 pane 层：
  - 每个**正在运行的 agent** 一条（跨 tab、跨 pane 平铺），显示 agent 名与状态点，与现在的 `render_agent_row` 完全一致；
  - 每个**没有任何 agent 的 tab** 一条，显示 `Terminal-<tab 名>`（tab 名默认为数字 `1`、`2`，改过名就是自定义名）。一个 tab 不管分了几个 pane，只要没 agent 就只出一条。
- 一个 tab 只要任意 pane 有 agent，就只出该 tab 下的 agent 条目，不再出空 tab 条目；分屏里的空 pane 不单独列。
- **agent 行的 Rename / Close 保持现状不改**：Close 走 `close_agent_pane`（关 pane，不连带关整个 tab），主人明确确认当前逻辑符合要求。
- **空 tab 行的 Rename / Close** 走 `tab.rename` / `tab.close`（关整个 tab）。
- **New Tab** 入口放在 space 行（右键菜单一项 + hover 的 `+`），走 `tab.create`。
- **动态刷新**：靠现有 1 秒轮询。终端里手动起了 agent，下一轮该 tab 的 `Terminal-N` 条目变成 agent 条目；agent 全部退出，agent 条目收敛回一条 `Terminal-N`。不新增事件通道。
- **滚动条**：只做显示，鼠标悬在滑块上把光标换成箭头（与 HiCodeer 终端滑块一致），不接管拖动/点击跳转。
- **右键菜单**：菜单项清单由 herdr 提供（新增只读 API），HiCodeer 用 `ContextMenu` 渲染，点击后调 herdr API 执行；`Rename pane` 用 HiCodeer 风格的行内输入框，确认后调 `pane.rename`。

## 3. 架构：把 herdr client 降为 pane 渲染器

跨进程链路（三进程、两跳）：

```
HiCodeer (GPUI, app uid)
  │  token 守护的 loopback TCP 40240 → daemon 内嵌 herdr server 的 API UDS
  ▼
hicodeerd (daemon uid) ── 内嵌 herdr server（状态与 API 的权威）
  │  client protocol UDS ↔ pty
  ▼
hicodeerd agent-tui (TUI 客户端，只画 pane 内容)
```

三条数据流：

- **tab / agent 数据**：`agent_panel` 轮询 `tab.list` + `agent.list` + `workspace.list`，在宿主侧 join 出条目。这是纯读，不需要客户端参与。
- **右键**：宿主 GPUI 收到右键 → 换算成终端行列 → `pane.context_menu` 问清单 → 渲染 → 点击调 `pane.context_menu.invoke` → server 执行。**客户端不参与菜单**。
- **滚动条**：宿主从 `pane.list` 的 `scroll` 字段（`herdr/src/api/schema/panes.rs:487` 的 `PaneScrollInfo`）拿位置，随面板轮询刷新。

为什么不走别的路：

- **不在两个进程之间传像素/状态流**：herdr 已有完整 JSON API，让 server 做权威、宿主做视图，比自建上行通道简单且不会与 server 状态分叉。
- **不让客户端回传菜单动作**（已排除方案，见 §10）：回传需要新增 client↔server 的下行指令通道；而菜单动作除 Rename 外本来就是 server API 调用序列，server 侧直接执行即可。
- **pane 内容仍由 herdr 渲染**：`pane.surface` 的字符网格、选择、copy mode、kitty graphics 都在客户端，整体重写进 GPUI 不在本设计范围内。

## 4. 实现分解

### 4.1 herdr：新增 chrome 开关（`src/config/model.rs` 的 UiConfig）

三个字段全部带 serde default，保证非 OHOS 与旧配置行为不变：

- `show_tab_bar: bool`（默认 `true`）：`false` 时布局里 tab_bar 高度为 0，且跳过绘制，pane 内容顶满整个终端。
- `context_menu_owner: enum { Herdr, Host }`（默认 `Herdr`）：`Host` 时右键既不弹客户端菜单，也不把右键转成 pane 鼠标事件下发。
- `pane_scrollbars` 已存在（`model.rs:978`），配置里直接置 `false`，无需改代码。

配套：`Default` 实现（`model.rs:1221` 附近）、配置解析单测（`model.rs:1531`、`1546` 附近的断言）同步补齐。

### 4.2 herdr：渲染与输入接入点

- `src/client/shell/render.rs:253` 的 `render_shell`：`show_tab_bar == false` 时不进 `:313` 的 `render_tab_bar` 分支；布局计算处把 tab_bar 的 Rect 高度置 0（不是只跳过绘制，否则顶部会留一行空白）。
- `src/client/shell/mouse.rs:1785` 的右键分支：`context_menu_owner == Host` 时提前返回，不生成 `ClientShellOverlay::ContextMenu`，也不 `push_pane_mouse_event`。
- `src/ui/scrollbar.rs` 与 app 侧滚动条绘制路径遵守 `pane_scrollbars = false`（**实现时核对**：确认 server 下发的 `paneScrollbarRect` 也随该配置关闭，否则客户端会在 surface 里留一列空白）。

### 4.3 herdr：新增菜单清单与执行 API

把 `src/client/shell/context_menu.rs:4` 的 `items()` 规则搬到 server 侧共用（抽到 `src/app/api/` 或一个共享模块），保证「显示什么仍由 herdr 决定」只有一份实现。搬移可行，因为 `open_pane_context_menu`（`:145`）依赖的三个状态 server 侧都有：

- `source_pane_id` = 当前 `focused_pane_id` 且不等于目标 pane；
- `has_manual_label` = `pane.label.is_some()`；
- `right_click_passthrough` = pane 上的字段。

两个新方法加进 `src/api/schema.rs:47` 的 `Method`、`ResponseResult` 与 `src/api/schema/panes.rs`；同步补 schema round-trip 单测（`src/api/schema/tests.rs` 的既有做法）。

- `pane.context_menu`：入参 `{ tab_id, row, col }`，server 用 `pane.layout`（`src/app/api/panes.rs:509`，返回 `PaneLayoutRect`）按行列命中 pane，返回该 pane 的菜单项清单。
- `pane.context_menu.invoke`：入参 `{ pane_id, action_id }`，server 执行等价于 `activate_pane_context_action`（`context_menu.rs:358`）的动作序列：`ClearPaneName` → `pane.rename(label: null)`、`SwapWithFocusedPane` → `pane.swap` + `pane.focus`、`SplitRight` / `SplitDown` → `pane.split`、`Zoom` → `pane.zoom`、`ToggleRightClickPassthrough` → `pane.input.set`、`ClosePane` → pane 关闭。`RenamePane` 不在此执行，返回 `{ kind: "rename", pane_id, current_label }`，由宿主弹行内输入框。

多 pane 命中与坐标换算见 §6。

### 4.4 配置下发

`crates/gpui_ohos/depend/cmd-agent/hicodeerd/conf/agent-config.toml` 的 `[ui]` 段：

```
show_tab_bar = false
pane_scrollbars = false
context_menu_owner = "host"
```

同时改掉第 16-18 行 "The tab row stays" 那段注释，说明这三样已由 HiCodeer 绘制。配置随 HNP 的 `conf/agent-config.toml` 发布，由 `HERDR_CONFIG_PATH` 指向（`hicodeerd/src/main.rs` 的 `agent-tui` 分支会把它拷到可写副本）；**实现时核对**：客户端读的是这份拷贝而不是包内原件。

### 4.5 `crates/agent_panel`：数据模型与条目

- `snapshot.rs:103` 的 `Snapshot::from_api` 改为接收三份 payload（`workspaces` / `agents` / `tabs`），产出平级条目枚举：
  - `Entry::Agent`（复用现有 `Agent` 结构与 `render_agent_row`，`agent_panel.rs:1439`）；
  - `Entry::Terminal { tab_id, label, status }`（新）。
  - join 规则：按 `tab_id` 把 agent 挂到 tab 下；tab 的 agent 列表非空 → 逐条 `Entry::Agent`；为空 → 一条 `Entry::Terminal`，label 取自 `tab.list` 的 `TabInfo.label`（`herdr/src/api/schema/tabs.rs`）。状态点用 `TabInfo.agent_status` 聚合。
- `client.rs:53` 的 `fetch()` 增加一次 `tab.list` 调用；新增 `tab.create` / `tab.focus` / `tab.rename` / `tab.close` 四个薄封装（照 `focus_space` 等现有写法）。
- `agent_panel.rs:1287` 的 `render_space` 遍历新条目；空 tab 行复用 `row_container`，显示 `Terminal-<label>`，行内 Rename / Close 按钮走 tab API；space 行增加 `+` 按钮与右键菜单项 `New Tab`。
- **不改**：agent 行的 close 仍调 `close_agent_pane`（`client.rs:110`），rename 仍走 `rename_agent`；历史会话区不动。

### 4.6 `crates/terminal_view`：右键菜单

`terminal_view.rs:715` 已有 `deploy_context_menu`，样式基准就是它。改动**全部加 `#[cfg(target_env = "ohos")]` 门控，且仅 `is_agent_terminal()`（`terminal_view.rs:365`）生效**：

- 现有右键处理（`terminal_view.rs:1845` 的 `on_mouse_down`）在 `terminal.read(cx).mouse_mode(...)` 为真时不弹菜单；agent terminal 的 herdr 客户端开着鼠标捕获，所以现在弹的是 herdr 的字符菜单。对 agent terminal 要绕过该判断，改走 herdr 清单。
- 右键时把 GPUI 坐标换算成终端行列，调 `pane.context_menu`，用返回的 `[{ id, label }]` 构造 `ContextMenu`（`menu.action` 一段一项，风格、位置、dismiss 行为复用 `deploy_context_menu`）。
- 点击菜单项调 `pane.context_menu.invoke`；`kind: "rename"` 时用 HiCodeer 风格行内输入框（参考 `terminal_view.rs:700` 附近的 rename editor 做法），确认后调 `pane.rename`。
- 右键事件不再转成 ANSI 发给 pty，避免 herdr 客户端同时收到一份。
- **未验证**：坐标换算所需的单元格尺寸取自终端字体度量；实现后须在分屏与不同字号下实测命中。

### 4.7 `crates/terminal_view`：滚动条

同样 cfg 门控、仅 agent terminal 生效：

- `terminal_view.rs:1897` 的 `custom_scrollbars` 会滚动 HiCodeer 自己终端的 scrollback，对 agent terminal 是错的（会把 herdr 的 TUI 画面滚走），对该场景禁用。
- 改画一个由 herdr `PaneScrollInfo` 驱动的滑块：`thumb_top` / `thumb_len` 按 `herdr/src/ui/scrollbar.rs` 的 `scrollbar_thumb` 同一公式计算，颜色与尺寸取 `Scrollbars::for_settings::<TerminalScrollbarSettingsWrapper>()` 那套主题值。
- 滑块 hover 时把光标设为箭头（与 HiCodeer 终端滑块一致）；**不接管拖动与点击跳转**。
- 位置数据来自 `pane.list` 的 `scroll` 字段，随面板 1 秒轮询刷新。**已知局限（未验证）**：滚动时滑块最多滞后 1 秒；若实测不可接受，再补 `pane.scroll_changed`（`herdr/src/api/schema/events.rs:81`）订阅，列为第二阶段，本次不做。

## 5. 新增 API 的报文

`pane.context_menu`（设计稿为 `tab_id` 必填；实现见 §11 P1：`tab_id` 省略由 server 取当前聚焦 tab，`row`/`col` 由宿主按 `terminal_bounds` 换算后传入，省略时取聚焦 pane）：

```json
{"id":"pane.context_menu","method":"pane.context_menu",
 "params":{"row":12,"col":80}}
```

```json
{"result":{"type":"pane_context_menu","pane_id":"w_1:p2","items":[
  {"id":"rename_pane","label":"Rename pane"},
  {"id":"split_right","label":"Split right"},
  {"id":"split_down","label":"Split down"},
  {"id":"zoom","label":"Zoom"},
  {"id":"toggle_right_click","label":"Send right-clicks to pane"},
  {"id":"close_pane","label":"Close pane"}
]}}
```

`pane.context_menu.invoke`：

```json
{"id":"invoke","method":"pane.context_menu.invoke",
 "params":{"pane_id":"w_1:p2","action_id":"split_right"}}
```

`rename_pane` 的响应为 `{"result":{"type":"pane_context_menu_rename","pane_id":"w_1:p2","current_label":null}}`（不是设计稿写的 `kind: "rename"`，见 §11 P2）。

**实现时核对**：`Method` / `ResponseResult` 枚举、`schema.rs` 的 `method_name` 映射（`herdr/src/api/server.rs:622` 附近）、JSON schema 生成与 round-trip 单测四处都要同步，缺一处会编译或运行时报错。核对结论：前三处已同步，round-trip 单测未补（§11 P9）。

## 6. 多 pane 命中与坐标换算

宿主右键拿到的是 GPUI 像素坐标，herdr 的命中判定用终端字符行列：

1. 宿主按终端字体度量把 `event.position` 换算成 `(row, col)`，随请求传给 `pane.context_menu`。
2. server 用 `pane.layout`（`src/app/api/panes.rs:509`）的 `PaneLayoutRect` 判断落在哪个 pane，再据该 pane 的状态生成菜单清单。
3. 单 pane 时该判定退化为「当前 tab 的 root pane」，无需额外逻辑。

这样分屏命中完全由 herdr 决定，宿主不复制布局算法。**未验证**：终端 padding、HiDPI 缩放、herdr 的 mode bar 占行是否影响 row 换算，实现后实测。

## 7. 门控与上游同步约束

- `crates/terminal_view/` 是上游 Zed 代码：新增分支必须 `#[cfg(target_env = "ohos")]` 门控，非 OHOS 桌面构建行为不变；不能门控的整段改动用 `[OHOS PORT BEGIN/END]` 注释对。
- `herdr/` 是仓库内 vendored 的上游代码：本轮新增的是**通用配置项与通用 API**（不是 OHOS 专属补丁），默认值保证其它平台行为不变；改动处留注释说明用途，后续合上游 herdr 快照时按清点评估文档 §7 的手法库同步，勿在合并时丢失。
- `移植记录/` 下已有文档只读，本设计只新增这一篇，不改既有文档。
- 协议面（`HERDR_ENV`、`HERDR_PLUGIN_*`、`herdr-plugin.toml`、`[ui.toast.herdr]`）按去品牌化既定边界保留，不动。

## 8. 验证

改完按既定授权跑 `./script/bundle-ohos`（判据 `EXIT=0` + `HAP BUILD SUCCESSFUL`）与 `./install-local.sh` 装包；daemon 不随重装换新，若改到 agent.sessions 一类 daemon 内代码才需要重启 daemon（本设计改的是 client 与 API，装包后重启应用即可，**实现时核对**）。

实机验收清单：

1. Agent 终端顶部不再有 herdr 的 tab 字符栏，pane 内容顶满。
2. 面板 space 下：跑着 agent 的显示 agent 名 + 状态点；没 agent 的 tab 显示 `Terminal-N`；同一 tab 多 pane 只影响 agent 条目数，不增加空 tab 条目。
3. 在终端里手动起 agent / 退出 agent，1 秒内条目在 `Terminal-N` 与 agent 名之间切换。
4. 空 tab 行 Rename / Close 生效（关整个 tab）；agent 行 Rename / Close 仍是关 pane。
5. space 行的 `+` 与 `New Tab` 能新建 tab，面板随即出现新条目。
6. 主显示区右键弹 HiCodeer 风格菜单，6 项动作逐一生效；Rename pane 用行内输入框。
7. 右侧滚动条按终端风格显示，鼠标悬在滑块上光标变箭头；滚动时滑块位置跟随（允许最多 1 秒滞后）。
8. 非 OHOS 桌面构建不受影响。

## 9. 开发完成后核对清单（主人要求）

开发完成后回到本文逐条核对，发现偏差就改本文或在对应条目标「已偏离 + 原因」，并把开头状态改为「已实现」。以下为 2026-11-04 的核对结论：

- [x] §2 的条目规则、`Terminal-<tab 名>` 文案、agent 行 Close 保持关 pane 三项：**一致**。`Space.terminals` + `render_terminal_row` 显示 `Terminal-<label>`；agent 行未改，Close 仍走 `close_agent_pane`。
- [x] §4.1 的三个配置字段在 `Default`、解析单测、`agent-config.toml` 三处：**齐全**。`show_tab_bar` / `context_menu_owner` 新增并补了 `pane_appearance_defaults_and_parse` 断言；`pane_scrollbars` 为既有字段，配置文件下发三项。
- [x] §4.2「tab_bar 高度置 0」：**做到**（`herdr/src/client/shell/config.rs` 的 `layout()` 里 `show_tab_bar && ...`，高度为 0，`render_shell` 自然跳过）。**「右键不下发 pty」已偏离**，见 §11 P7。
- [x] §4.3 菜单规则只有一份实现：**一致**。`herdr/src/api/schema/context_menu.rs` 的 `pane_context_menu_specs` 是唯一来源，客户端 `items()` 与 server handler 都调它。
- [ ] §4.4 配置走 `HERDR_CONFIG_PATH` 可写副本：**未验证**（`main.rs` 的 `writable_agent_config` 链路存在，未实机确认）。
- [x] §4.5 未引入 `pane.list`：**一致**（面板只用三份 list）；但宿主侧另加了 `pane.current` / `pane.context_menu` 调用，见 §11 P6。
- [x] §4.6 / §4.7 cfg 门控且仅 `is_agent_terminal()` 生效：**一致**（`terminal_view::agent_chrome` 模块、`agent_chrome_active`、`render_agent_chrome` 均带 cfg，非 OHOS 有空实现）。**非 OHOS 桌面构建未验证**（本机 target 即 ohos）。
- [x] §5 四处同步：`Method` / `ResponseResult` / `api_method_name` 三处已同步；静态 schema 文件 `herdr/docs/next/api/herdr-api.schema.json` 已用 `HERDR_UPDATE_API_SCHEMA=1 ./agent-tests/run-unit-tests.sh 'generated_protocol_schema_artifact_is_current'` 重新生成并通过一致性测试（含两个新方法），且已随 2026-11-04 14:07 的 `bundle-ohos` + `install-local.sh` 编进 daemon 并装包，先前「已安装 HAP 副本仍是旧的」的滞后已消除。**单独新增的 round-trip 单测未补**，见 §11 P9。
- [ ] §6 坐标换算在分屏、HiDPI、mode bar 出现时的命中：**未验证**，需实机。
- [ ] §8 的 8 条实机验收：**未验证**，待主人在设备上逐条确认；不得当作通过。
- [x] 文件头状态已更新为「已实现」并写明编译判据；**提交号无**（改动未提交，为工作区改动）；实机验证人待主人补。

## 10. 已排除的方案（决策留痕）

- **tab 栏保留在 TUI 顶部、由 HiCodeer 覆盖绘制**：可行，但要在字符网格第一行盖 GPUI，得自己算单元格宽高、内容原点、tab 溢出滚动，并在字号/DPI 变化时重算；成本高于直接放进面板。主人最终选「TUI 里不显示，tab 展示进面板」。
- **面板里显式画 tab 层 / pane 层**（三层树、tab 行 + pane 子行、徽标 + hover 浮层、行内状态点）：主人明确「不想有多个层级」，最终定为平级两类条目。
- **右键动作回传 herdr 客户端执行**：最严格地「由 herdr 决定」，但需新增 client↔server 下行指令通道（`EndpointControl` 新 kind + client 侧分发），而除 Rename 外的动作本就是 server API 序列；主人选「HiCodeer 调 herdr API 执行」。
- **`pane.scroll_changed` 订阅**：能让滚动条跟手，但宿主侧要新写长连接订阅客户端。本次只做显示 + hover 光标，滞后 1 秒可接受再不上订阅；实测不可接受才做第二阶段。

## 11. 实现记录与偏差（2026-11-04 核对）

本轮改动 20 个代码/配置/资源文件（19 处修改 + 新增 `herdr/src/api/schema/context_menu.rs`），另加本文，共 21 个文件。分布以 `git status --short` 为准：

- `herdr/`：`config/model.rs`、`config.rs`、`lib.rs`（配置开关与默认模板）、`client/shell/state.rs`、`client/shell/config.rs`、`client/shell/mouse.rs`、`client/shell/context_menu.rs`（关闭三处 chrome、菜单规则改用共享 specs）、`api/schema.rs`、`api/schema/response.rs`、`api/schema/context_menu.rs`（新）、`api/server.rs`、`api/mod.rs`、`app/api.rs`、`app/api/panes.rs`（两个新 API）、`docs/next/api/herdr-api.schema.json`（静态 schema 重新生成，见 P9）。
- `crates/agent_panel/src/`：`snapshot.rs`、`client.rs`、`agent_panel.rs`。
- `crates/terminal_view/src/terminal_view.rs`（全部 cfg 门控）。
- `crates/gpui_ohos/depend/cmd-agent/hicodeerd/conf/agent-config.toml`。

编译与安装判据：

- `cd herdr && cargo check --lib`：通过（仅既有 warning）。
- `cargo check -p agent_panel -p terminal_view`：通过（仅既有 warning）。
- `bash ./script/bundle-ohos`：`EXIT=0` + `=== HAP BUILD SUCCESSFUL ===`（12:48，HAP 186259628 字节）。
- `sh ./install-local.sh`：force-stop、安装、启动均成功。脚本提示 daemon 不随覆盖装 HAP 换新。
- 装包后实测：磁盘 `/data/service/hnp/bin/hicodeerd` 指向的二进制时间戳为 20:48:37（本次构建），但 daemon 进程（pid 15077）启动于 20:44:43，仍跑旧映像。

与本文设计的差异（编号供后续核对引用）：

- **P1** `pane.context_menu` 的 `tab_id` 改为 `Option<String>`（host 省略，server 取当前 active workspace/tab），`row`/`col` 也改为 `Option<u16>`（省略时取聚焦 pane）。原因：host 不跟踪 herdr 的 tab_id；host 用 `last_content().terminal_bounds` 的 `cell_width`/`line_height` 换算行列传入。§5 报文已按实现更正。
- **P2** `rename_pane` 的响应是 `ResponseResult::PaneContextMenuRename`（`type: pane_context_menu_rename`，字段 `pane_id` / `current_label`），不是设计稿的 `{ kind: "rename", ... }`。
- **P3** New Tab 入口只做了 space 行 hover 的 `+` 按钮，未做 space 行右键菜单项。原因：主人最后定稿「加一个新增的入口就可以了」，取最小实现。
- **P4** 滚动条状态来源改为 TerminalView 自己 500ms 轮询 `pane.current` 的 `scroll`，而非随面板 1 秒轮询；滑块滞后 ≤500ms。设计稿 §3 / §4.7 的「随面板 1 秒轮询」已偏离。
- **P5** 宿主与 server 之间新增 `terminal_view::agent_chrome::AgentChromeHooks` 全局函数指针（由 `agent_panel::init` 注册），避免 terminal_view 依赖 agent client；右键清单读取与 invoke 均为同步 loopback 调用，不是异步任务。设计稿未提此实现细节。
- **P6** `Space` 用 `terminals: Vec<TerminalEntry>` 字段而非 `Entry` 枚举；`Agent.tab_id` 去掉 `#[cfg(target_env = "ohos")]` 门控（所有平台可读，便于 join）。面板未引入 `pane.list`，但宿主侧新增了 `pane.current` 调用。
- **P7** host 模式下，pane 显式开启 `right_click_passthrough`（或配置修饰键）时仍透传右键给 pane，保留 herdr 原有语义；GPUI 层也未阻止右键 ANSI 发往 pty，而是由 herdr 客户端在 host 模式忽略它。设计稿 §4.2 写的「既不弹菜单也不下发」只在普通右键下成立。
- **P8** Rename pane 由 host 用 `Editor::single_line` 浮层实现（符合设计稿「HiCodeer 风格行内输入框」）。
- **P9** `src/api/schema/tests.rs` 里没有为本轮两个新方法单独新增 round-trip 单测；只补了 `config/model.rs` 的配置解析断言。静态 `herdr/docs/next/api/herdr-api.schema.json` 由既有的 `generated_protocol_schema_artifact_is_current` 测试在 `HERDR_UPDATE_API_SCHEMA=1` 下重新生成（1 passed），已含 `pane.context_menu` / `pane.context_menu.invoke`；新副本已随 2026-11-04 14:07 的 `bundle-ohos` + `install-local.sh` 编进 daemon 并装包，先前「`hicodeerd api schema --json` 查不到新方法」的滞后已消除。该副本不影响实际请求解析（以 Rust 枚举为准，见 P11 实测）。
- **P10** 非 OHOS 桌面构建、§6 坐标换算命中、§8 实机 8 条验收、§4.4 可写副本链路均**未验证**，需主人在设备上完成；本文不把它们记作通过。
- **P11（重启 daemon 后的实测，2026-11-04 20:52）**
  - 主人重启后 daemon 为 pid 31238（`/proc/31238` 时间 20:52:08，晚于构建 20:48:37），`/proc/31238/maps` 中 `hicodeerd` 的 inode 为 1551005，与磁盘 `/data/service/hnp/hicodeerd.org/hicodeerd_0.1.0/bin/hicodeerd` 一致 → 确为新映像；`agent-tui` 客户端 pid 31724（20:52:17）亦为新进程。
  - HNP 发布的 `conf/agent-config.toml` 含 `show_tab_bar = false` / `context_menu_owner = "host"` / `pane_scrollbars = false`。
  - 直连 daemon 的 API UDS 实测：`pane.context_menu {}` 返回 `pane_context_menu`，`items` 为 6 项（Rename pane / Split right / Split down / Zoom / Send right-clicks to pane / Close pane）；`pane.context_menu.invoke { action_id: "rename_pane" }` 返回 `pane_context_menu_rename`（`current_label: null`）。
  - `tab.list` 返回 3 个 tab（w2:t1 / w2:t3 / w3:t1），`agent.list` 返回 2 个 agent（挂在 w2:t1 与 w3:t1）；按 §2 的 join 规则，w2:t3 应显示 `Terminal-3`，另两个 tab 显示 agent 名。数据侧符合预期，**面板渲染未目视验证**。
  - `pane.current` 在无滚动历史时 `scroll` 为 null：host 侧 `focused_pane_scroll()` 会返回 Err，轮询里静默忽略 → 不画滑块，与「无滚动历史不显示滚动条」的预期一致；有滚动历史时是否返回 `scroll` 对象未实测。
  - 应用侧 cmd-client 的 40240 token 是否随 daemon 重启重新握手**未验证**；若面板显示断连，重启应用即可重新握手。

- **P12（第二轮：主人实机反馈后的修复，2026-11-04）**
  主人实机验收反馈四项：① TUI 的 tab 栏没消失；② herdr 自己画的右键菜单没消失；③ 滚动条看不见了；④ space / agent / 历史 / 归档行的操作按钮间距不一致，右对齐点也不一致。
  - ①②根因：`hicodeerd/src/main.rs` 的 `writable_agent_config()` 在可写副本已存在时只补「缺失的表」（`add_missing_config_tables`），不补「新增的键」。app 沙箱里那份旧副本没有 `show_tab_bar` / `context_menu_owner`，`#[serde(default)]` 于是回落到上游默认值，客户端继续画 tab 栏与右键菜单。server 侧读的是 HNP 原件（含新键），所以 `pane_scrollbars=false` 生效，正好解释了「①②还在、③ herdr 的滚动条却没了」这一组合现象。
  - ①②修复：`herdr/src/config/model.rs` 的 `Default for UiConfig` 用 `cfg!(target_env = "ohos")` 把 `show_tab_bar` / `pane_scrollbars` 默认改为 `false`、`context_menu_owner` 默认改为 `Host`，与同文件既有的 `prompt_new_tab_name: !cfg!(...)` 手法一致；副本缺键时也生效。配置解析单测改为 cfg 感知并通过（`pane_appearance_defaults_and_parse`）。`agent-config.toml` 的显式三项保留。
  - ③修复：host 原先调 `pane.current` 取滚动度量，实测该响应的 `scroll` 为 null；改用 `pane.list` 取 `focused` pane 的 `scroll`（实测 w2:p1 为 `offset=0, max=4314, viewport=32`），无 scroll 时返回 `(0,0,0)` 而不是 Err。改动在 `crates/agent_panel/src/client.rs::focused_pane_scroll`。
  - ④修复：新增 `ROW_ACTION_GAP = px(2.)` 与 `ROW_ACTION_SIDE_PADDING = px(4.)`，space / agent / 空 terminal tab / 历史 / 归档五种行的 hover 操作按钮统一 `.gap(ROW_ACTION_GAP).px(ROW_ACTION_SIDE_PADDING)` 且都用 `.right_0()`，右对齐点一致。
  - 第二轮 `cargo check --lib`（herdr）、`cargo check -p agent_panel -p terminal_view`、配置单测均通过；重新构建 HAP 与装包后**仍需再次重启 daemon**（herdr 二进制变了），四项实机表现仍需主人目视确认。

- **P13（第三轮：滚动条几何对齐终端，2026-11-04）**
  主人反馈：host 滑块贴最右边缘（`.right_0()`），鼠标放上去被系统识别成「改变窗口大小」；终端自身滚动条不贴边且更粗。
  - 参照 `crates/ui/src/components/scrollbar.rs`：`ScrollbarStyle::Regular`（terminal 默认）在 OHOS 上拇指宽 9px、`SCROLLBAR_PADDING = 4px`、轨道宽 `9 + 2*4 = 17px`，拇指距右缘 4px；`with_stable_track_along` 用 `editor_background` 画轨道。
  - 修复：`crates/terminal_view/src/terminal_view.rs` 新增 `AGENT_SCROLLBAR_THUMB_WIDTH = 12px`（比 Regular 略粗，按主人反馈）、`AGENT_SCROLLBAR_RIGHT_INSET = 6px`（比 Regular 的 4px 多 2px，确保离开窗口 resize 命中区）、`AGENT_SCROLLBAR_TRACK_WIDTH = 24px`；`render_agent_chrome` 先画一层轨道（`.right_0()`、宽 24px、`editor_background`），再画拇指（`.right(6px)`、宽 12px、`.rounded_md()`、`scrollbar_thumb_background`、`.cursor(CursorStyle::Arrow)`）。
  - 此改动只在应用侧，装包重启应用即生效，**不需要重启 daemon**。`cargo check -p terminal_view` 通过。

- **P14（第四轮：滚动条拖动、拇指长度、不盖文字，2026-11-04）**
  主人反馈：① 滚动条不能通过鼠标拖动（确认当时按定稿只做显示）；② 样式与终端不一致，「现在只是一个点」，终端滚动条长度按缓冲自己计算；③ 滚动条盖住最右边的文字。主人定夺：**只补拇指拖动**，不做轨道点击跳转。
  - 长度：照 `ui::components::scrollbar::thumb_ranges` 的算法，`thumb_size = MINIMUM_THUMB_SIZE(25px).max(track_h * viewport/(viewport+max))`；之前是 `.max(8px)` 且比例极小，长缓冲下退化成一个点。新增 `AGENT_SCROLLBAR_MIN_THUMB = px(25.)`。
  - 拖动：`focused_pane_scroll()` 现返回 `(pane_id, offset, max, viewport)`；新增 `client::set_pane_scroll` 调既有 `pane.scroll { pane_id, offset_from_bottom }`；`AgentChromeHooks` 加 `set_scroll`。拇指 `on_mouse_down` 记录 `AgentScrollDrag { pane_id, start_y, start_offset, max, thumb_h, track_h }`；根视图 div 上挂 `on_mouse_move` / `on_mouse_up(MouseButton::Left)` 处理拖动（不用 `window.on_mouse_event`：它只能在 paint 阶段注册，放 `render()` 会触发 `debug_assert_paint` 断言），位移换算 `offset = start_offset - dy/max_top * max`，本地乐观更新滑块并写回 server；拖动时拇指用 `scrollbar_thumb_active_background`、hover 用 `scrollbar_thumb_hover_background`。
  - 不盖文字：`render()` 给 `#terminal-view-container` 加 `.pr(AGENT_SCROLLBAR_TRACK_WIDTH)` 预留 gutter，滚动条画进这块留白；终端列数相应减少（与 herdr 原先 `pane_scrollbars=true` 时自己留一列等价），不再覆盖最右字符。
  - 此轮只动应用侧，`cargo check -p agent_panel -p terminal_view` 通过；装包重启应用即生效，**不需要重启 daemon**。

- **P15（第五轮：拖动不跟手，2026-11-04）**
  主人反馈：拖动滚动条时界面滚动不跟手。
  - 根因两处：① `drag_agent_scroll` 每次 `MouseMoveEvent` 都**同步**调 `pane.scroll`（一次 loopback TCP 往返），阻塞 UI 线程；② 500ms 轮询用服务端旧 offset 覆盖本地乐观位置，滑块每半秒跳回一次。
  - 修复：新增 `agent_scroll_last_request: Option<Instant>`，拖动期间写请求节流到 40ms；`send_agent_scroll` 用 `cx.background_spawn` 在后台线程发，不占 UI 线程；轮询在 `agent_scroll_drag.is_some()` 时只更新 `pane_id/max/viewport`、保留本地 offset；松手时 `flush_agent_scroll` **同步**补发最终 offset，保证最后一次写入压过仍在途的后台请求（避免乱序）。
  - 仍存在的架构性延迟：服务端 `pane.scroll` 后要生成新的 pane surface → 经 pty 发给 TUI 客户端 → 客户端重绘，画面内容必然比本地滑块慢一帧；本次只消除 UI 阻塞与滑块跳回，不消除这条链路。
  - 应用侧改动，`cargo check -p agent_panel -p terminal_view` 通过；装包重启应用即生效，**不需要重启 daemon**。2026-11-04 14:07 已执行 `bundle-ohos`（`HAP BUILD SUCCESSFUL`）与 `install-local.sh`（`install bundle successfully` / `start ability successfully`），P15 修复已装到设备；拖动是否跟手仍待主人目视确认。

- **P16（第六轮：按 claude 检视结论全量修复 + i18n 多语种，2026-11-04）**
  主人要求「检视出来确定是问题的都修改」，并给本轮新增界面文字补 i18n 多语种。claude 在独立 pane（`w2:t5` / `w2:pG`，cwd=hicodeer，`argv:["claude"]`，未改其 `~/.claude`、env、wrapper、签名）只读检视本轮 21 个文件，耗时约 10m49s，给出 3 高 + 6 中 + 若干低/规范项；除「滚动条纵向基准」外全部落地。

  高严重度（已修）：
  1. **拖拽卡死**：`on_mouse_up` 只在 `hitbox.is_hovered(window)` 时回调（`crates/gpui/src/elements/div.rs:219`），指针离开视图后松手时 `agent_scroll_drag` 永久保持。新增 `on_mouse_up_out`，并在 `on_mouse_move` 检查 `event.pressed_button != Some(MouseButton::Left)` 时 `end_agent_scroll_drag`。
  2. **写乱序**：原 40ms 节流 + 每次 `background_spawn` 各开一条独立连接，无法保证「最后一次写胜出」。改为 `AgentScrollWriter`：单一常驻 worker 串行发送，`pending` 只存最新 offset；松手把最终 offset 压进同一通道，因此最后一次写必定最后到达。
  3. **轮询泄漏**：原 `start_agent_scroll_poll` 用 `cx.spawn(...).detach()` 且 `let _ = view.update(...)` 不退出。改为 Task 存 `agent_scroll_poll_task` 字段（随视图 drop 取消），`view.update(...).is_ok()` 为假时 `break`。

  中严重度（已修）：
  4. 三处同步 loopback（右键清单 / invoke / Rename 确认）改异步：`window.spawn` + `background_spawn`，结果回主线程再 `ContextMenu::build` 或开行内编辑器。
  5. `mouse.rs` 的 Host 判断从 pane 命中块外移进块内，不再连带禁掉 workspace / 标签栏右键菜单。
  6. `tab.list` 加 OHOS 门控，OHOS 侧失败降级为空 tabs（不拖垮整个快照）；非 OHOS 不拉，`Terminal-*` 行自然为空。
  7. `focused_pane_scroll` 去掉 `panes.first()` 回退，找不到 focused pane 返回 Err（轮询据此隐藏滑块）。
  8. 静默吞错全部补日志：scroll 写入失败 `warn`、轮询失败 `warn`、菜单 / invoke / rename 失败 `error`；`panes.rs` 的 swap 失败改为解析响应并返回错误，不再继续 focus。
  9. flush 显式用 drag 的 pane_id；拖拽期间轮询发现 pane_id 变化则结束拖拽，避免把旧 offset 写到新 pane。

  低严重度 / 规范（除纵向基准外已修）：
  - `AGENT_SCROLL_WRITE_INTERVAL` / `AGENT_SCROLL_POLL_INTERVAL` 替代 40/500 魔鬼数字。
  - 拖拽开始 / 结束、菜单打开、rename 打开 / 确认补 info 状态日志。
  - 四元组改 `PaneScrollMetrics`、菜单项改 `PaneContextMenuEntry`（定义在 `terminal_view` 顶层、不带 cfg，避免跨 crate 门控连锁）。
  - `client.rs` 的 `call` 注释移回 `call` 上方。
  - `panes.rs` / `context_menu.rs` / `terminal_view.rs` 的 rustfmt 格式修正。
  - Agent 与空 tab 行按 `tab_index` 合并排序，与 tab 顺序一致。
  - `PaneContextMenuSpec` 去掉 `Deserialize`（`&'static str` 反序列化不可用）。
  - `client_views.rs` 的 `shell_endpoint_claims_geometry` 与 `shell_locations_may_need_reconcile` 补 `PaneContextMenuInvoke`。
  - **未改**：滚动条纵向基准（`terminal_view.rs` 用 `bounds.size.height` 算 track，而轨道 / 滑块按根视图 `top_0`/`bottom_0` 定位）。claude 自标「需实机验证」，盲改风险大于收益，留待实机确认后处理。

  i18n（13 语言）：
  - 源码：`Terminal-{}` 改 `localization::format_message`；`New Tab` 原已接 `localized_str!`；右键菜单标签在 `show_agent_context_menu` 用 `localization::lookup(&entry.label)`，保留 herdr 英文原文作 key，查不到回退英文。
  - 词典：13 个语言包补 9 个静态 key（`New Tab` / `Rename pane` / `Clear pane name` / `Swap with focused pane` / `Use HiCodeer right-click menu` / `Send right-clicks to pane` / `Split right` / `Split down` / `Close pane`）与 `Terminal-{}` 的 `formats`；`index.json` 补 `Terminal-{}` 的 `source_formats`；`catalog_sha256` 未改。未走 extract / generate 全量重生成（主人选「手工补 13 语言译文」），diff 可控。

  编译与安装：`cargo check -p agent_panel -p terminal_view`、`cargo check --manifest-path herdr/Cargo.toml --lib`、`bash ./script/bundle-ohos`（`HAP BUILD SUCCESSFUL`）、`sh ./install-local.sh` 均通过。**daemon 重启**：本轮改了 herdr 侧（`mouse.rs` / `panes.rs` / `client_views.rs`），装包后需再重启一次 daemon 才生效；宿主侧（terminal_view / agent_panel / 词典）重启应用即生效。

**daemon 重启**：第一轮由主人于 20:52 完成，实测证据见 P11；第二轮改了 herdr 的 `Default`，装包后仍需按同一手法再重启一次 daemon 才能让 ①② 生效。后续若再改 herdr 侧 daemon 内代码，同样按 `project_hicodeerd_restart` 的实测手法重启 daemon：

```zsh
killall hicodeerd
setsid /data/service/hnp/bin/hicodeerd --log </dev/null >/dev/null 2>&1 &
```

`killall` 会一并杀掉 `hicodeerd agent-tui` 客户端与 daemon 下的交互 shell，面板里的 agent pane 会断、需重开，且会中断当前正在运行的 agent 会话，执行前需主人确认。
