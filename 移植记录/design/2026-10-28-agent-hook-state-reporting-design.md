# Agent 状态上报与 Agents TUI 生命周期：设计

日期：2026-10-28
状态：已实施并通过单测 / 端到端验证
适用范围：`herdr/src/integration/**`（daemon 侧集成与插件）、`crates/agent_panel/src/agent_panel.rs`（面板侧的终端与通知）、`agent-tests/e2e/test_hooks.py`（验证）

## 1. 背景与目标

Agent 面板（`crates/agent_panel`）要显示每个 pane 里 AI agent CLI 的实时状态：在跑、空闲完成、等用户批准。这三个状态由 agent 自己上报最准，屏幕识别只能兜底。

目标：

1. 每个能在鸿蒙上跑起来的 agent CLI，都用它自己提供的扩展点（hook 脚本或进程内插件）把状态报到 pane 的 UDS 上。
2. 面板据此维护 agent 状态、完成标记与通知，并把「Agents TUI」这个客户端标签的生命周期管住——daemon 换代后必须重建，不能留一个连不上任何东西的空壳。

本轮补的两处缺口：

- **DevEco Code（deveco）在 TUI 下完全不上报**。它有两套插件宿主，此前只装了 server 那一套。
- **daemon 重启后 Agents TUI 不重连**。客户端收到 `server is shutting down` 会退出，但标签壳子还在，面板只看标签在不在，于是永远不重建。

## 2. 状态模型

三个对外状态，由服务端 `AgentState` 收敛：

- `working`：这一轮在跑（含工具执行、流式输出）。
- `blocked`：等用户批准或回答，agent 自己停住。
- `idle` / `done`：这一轮结束。

`done` 与 `idle` 的区别不在状态本身，而在服务端打的完成标记 `completion_seq`：`herdr/src/app/actions.rs` 里对 `Working → Idle` 的转换调用 `is_completion_transition`，只有非「用户 Esc 打断」的转换才置 `completion_seq`。面板的完成通知以此为准（`agent_panel.rs` 的 `notify_agent_transitions`：`Finished` 事件在 `completion_seq` 为空时直接跳过）。**状态是 agent 报的，完成标记是服务端判的**，两者不能混。

## 3. 三层结构

```
agent CLI 进程
  ├─ 命令式 hook：agent spawn 我们的脚本，脚本读 stdin JSON 后连 UDS
  └─ 进程内插件：JS/TS 插件直接连 UDS（deveco、opencode 这类）
        ↓  UDS（agent.sock）
herdr server（跑在 hicodeerd 里）
  ├─ 校验 HERDR_ENV / pane_id / seq，维护状态机与 completion_seq
  └─ 通过 agent.list / pane.get 暴露
        ↓  RPC
agent panel（app 侧）
  ├─ 行状态、行标记、提示音、应用内 toast
  └─ Agents TUI 标签的部署 / 重建
```

上报协议两个方法，参数都带 `pane_id` / `source` / `agent` / `seq`：

- `pane.report_agent`：`state` 三态之一，用于状态变化。
- `pane.report_agent_session`：`agent_session_id`，用于让 pane 记住它服务的是哪条会话（历史恢复要用）。

门槛三件事必须同时成立，否则插件/脚本直接返回不做事：`HERDR_ENV=1`、`HICODEER_AGENT_SOCKET_PATH`、`HERDR_PANE_ID`。

## 4. 各 agent 的接入方式

- `zcode`：命令式 hook，配置在 `~/.zcode/cli/setting.json` 的 `hooks.events`；blocked 来自 `PermissionRequest` 与 `PreToolUse`（matcher 为 `AskUserQuestion`）。
- `codewhale`：命令式 hook，配置在 `~/.codewhale/config.toml` 的 `[[hooks.hooks]]`；无 blocked 来源。
- `atomcode`：命令式 hook，配置在 `~/.atomcode/hooks.json` 的命名条目；无 blocked 来源。
- `deveco`：server 插件 + TUI 插件，配置在 `deveco.jsonc` 与 `tui.json`；blocked 来自 permission / question 列表非空。

事件命名的坑：zcode 用 PascalCase（`SessionStart` / `PreToolUse` / `PermissionRequest`），codewhale 与 atomcode 用 snake_case（`session_start` / `tool_call_before`）。**写错大小写不会报错，只是永远不触发**，所以每次接入都要用 `atomcode hooks list` 这类自检命令或探针确认真的执行了。

codewhale 与 atomcode 报不出 blocked，不是我们的映射没写，而是它们的事件集里没有审批语义：

- codewhale 二进制内置 `EVENTS = { SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, SubagentStart, SubagentComplete }`。
- atomcode 的 7 个事件（`SessionStart` / `UserPromptSubmit` / `PreToolUse` / `PostToolUse` / `PostToolUseFailure` / `Stop` / `SessionEnd`）同样没有审批类。

它们的审批框出现时状态只能停在 `working`，这是工具能力边界，不打算用屏幕识别去补。

## 5. deveco：双宿主与 TUI 插件

### 5.1 为什么需要两个插件

deveco 的插件加载按宿主分流：

- `~/.config/deveco/deveco.jsonc` 的 `plugin` 数组 → 只在 headless server（`deveco run` / `serve`）加载。
- `~/.config/deveco/tui.json` 的 `plugin` 数组 → 只在 TUI 加载。

面板里跑的是 TUI（`deveco` 无参数），所以 server 插件不会生效。这是「集成显示已安装、面板却一直 idle」的根因。

另有一个目录级陷阱：**`~/.config/deveco/plugins/` 下的每个文件都会被当成 server 插件加载**。往那里放一个只导出 `tui` 的插件会被拒绝：

- 只导出 `tui` → `must default export an object with server()`
- 同时导出两者 → `must default export either server() or tui(), not both`

所以 TUI 插件放在配置目录根：`~/.config/deveco/hicodeer-tui-session.js`。

### 5.2 插件形态

```js
export default {
  id: "herdr.deveco.session-state",
  setup,      // V1 入口，deveco 不走；保留以对齐 opencode 的形状
  tui,        // 实际入口，返回 dispose 函数
};
```

### 5.3 为什么是轮询

deveco 的 TUI API 提供了 `api.route.current`（当前路由）与 `api.state.session.status/permission/question`（按会话查状态），但没有可用的服务端事件通道：`api.event.on` 是 `(name, handler)` 两参，实测订阅 `session.*` 收不到任何事件。因此改为每 250ms 读一次这两个值，与 opencode TUI 插件 100ms 轮询 route 的做法同源。

代价是状态变化最多延迟 250ms，可接受；收益是不依赖未公开的事件名。

### 5.4 状态判定

- **会话归属取根会话**：TUI 允许从主会话打开子会话（`task` 工具）。pane 的身份应该始终是根会话，否则面板的「历史恢复」会被子会话 id 覆盖。`rootSessionID()` 沿 `api.state.session.get(id).parentID` 上溯，带环检测。
- **状态读整条链**：选中子会话时，工作发生在子会话上。`stateOf(selected, root)` 先查 selected 与 root 的 permission/question（任一非空即 `blocked`），再按 selected → root 的顺序取第一个已知状态。这样「子会话在跑、根会话 idle」不会被误判成空闲。
- **状态映射与 server 插件一致**：`active` / `busy` / `pending` / `retry` / `running` / `streaming` / `working` → `working`，`idle` → `idle`。两处读的是同一套 `session.status` 值，映射表不能分叉。

### 5.5 投递可靠性与生命周期

- **失败必须重发**：`requestOnce` 返回是否投递成功。失败时把 `lastSessionID` / `lastState` 清空，下一次 tick 重发。否则一次超时（daemon 刚起、socket 暂不可达）就会让某个状态永久丢失——`working → idle` 的 idle 那次丢了，面板会一直停在 working。
- **dispose 后在途请求丢弃**：请求在 `chain` 里排队，无法取消，所以在真正发送前再检查一次 `disposed`。否则 TUI 退出后迟到的上报会把 pane 写回 `working`。
- **API 缺失时明确报错**：`route` 或 `state.session.status` 不存在时打一次 `console.error` 并返回，不进入轮询。否则表现与「集成没装」完全一样，无从排查。

## 6. 安装、卸载与版本

### 6.1 两套落点

`install_deveco`（`herdr/src/integration/targets.rs`）写三处：

1. `plugins/hicodeer-agent-state.js`（server 插件），注册到 `deveco.jsonc` 的 `plugin`。
2. `hicodeer-tui-session.js`（TUI 插件，配置目录根），注册到 `tui.json` 的 `plugin`。

卸载对称：从两个配置文件移除条目，删两个文件。`tui.json` 由安装过程创建，卸载时保留文件本身（用户可能在里面还有别的设置）。

### 6.2 半装判定

`registry.rs` 的 `deveco_tui_integration_is_valid` 在 server 插件版本够新之后，再检查 TUI 插件文件与 `tui.json` 里的注册。任一缺失就把整体判成 `outdated`，让面板提示更新。

判据只认 `tui.json`，不认 `tui.jsonc`：install 只写 `tui.json`，uninstall 只改 `tui.json`，如果判定同时接受 `tui.jsonc`，用户在那个文件里留一条旧注册就会让「面板没状态」被误报成「已安装」。

### 6.3 版本

`DEVECO_INTEGRATION_VERSION = 2`，两个资产头同步写 `HERDR_INTEGRATION_VERSION=2`。TUI 插件是本轮新增的能力，老安装只有 server 插件，所以必须让它们被判定为过期——否则发布后老用户拿不到新插件。

## 7. Agents TUI 的生命周期

### 7.1 问题

`AgentPanel` 启动时会部署一个固定的「Agents TUI」标签，跑 `hicodeerd agent-tui --socket <agent.sock>`。这个客户端只连它启动时那个 daemon：

- daemon 关闭时给所有客户端发 `server is shutting down`，客户端按设计退出而不是重连。
- 标签是 pinned 的，进程退出后壳子还在。
- 面板原来用 `workspace_has_agent_terminal`（只判断「有没有 pinned 的 agent 标签」）判断是否已经部署，于是 `wanted == present` 恒成立，永不重建。

### 7.2 判据：daemon token

`cmd_client::AgentControl` 里有 `token`，注释写明是「this run generated」——**每次 daemon 启动都换一个**。面板记住部署时用的 token（`deployed_agent_token`），与当前 `agent_control()` 的 token 比较：

- 相同：标签属于当前这一代 daemon，正常。
- 不同：标签是上一代 daemon 的遗留，先关闭再重建。

选 token 而不是「读终端进程是否还活着」，是因为后者要改 `crates/terminal_view` 暴露接口；而 token 判据完全在面板内，且判的是「是不是同一个 daemon」这个真正的问题，不是「进程还在不在」这个代理指标。

### 7.3 两个时序细节

- **部署窗口不重复部署**：`deploy_agent_terminal` 是异步的（要 `create_terminal_task` 再 add 到 pane）。如果标签落地耗时超过重试间隔，refresh 会再发起一次部署，最终留下两个 Agents TUI 标签，且两者都会连上 daemon、都报状态。为此加了 `DEPLOY_SETTLE_TIMEOUT`（15 秒）：已经为这一代 daemon 发起过部署、且还在窗口内时，只置 `needs_sync` 等下一次刷新，不再部署。
- **`wanted == false` 时也要关**：早先的实现把「token 不匹配」折算成 `present = false`，于是 `wanted(false) == present(false)` 立刻返回，死标签没被关闭。现在改为独立分支：只要标签不属于当前 daemon，无论 `wanted` 指向哪边都先关掉，`wanted` 为真时再安排下一帧重建。

### 7.4 与断连阈值的关系

`note_disconnect` 在连续 3 次刷新失败后把 `wanted` 置 false 并关掉标签，用于「daemon 真的没了」的长时断连。它与 token 判据互补：短时重启走 token 分支（几秒内重建），长时断连走阈值分支（先收起终端）。两者都合法，不会互相打架——token 分支只在 `agent_control()` 有值（即 daemon 已应答）时才会走到。

## 8. 测试策略

### 8.1 插件单测（bun）

`herdr/src/integration/assets/deveco/hicodeer-tui-session.test.ts`，`bun test` 直接跑，mock `node:net` 与 TUI API，覆盖 8 项：无会话不发请求、选中会话后上报会话与 working 转换、子会话活动归到根会话、`streaming` 映射成 working、permission 出现时 blocked、dispose 后不再上报、投递失败会重发、缺 API 时不安装并报错一次。

它是纯 JS，不受 Rust 构建影响，也不需要设备。

### 8.2 集成单测（Rust）

`herdr/src/integration/tests.rs` 里 deveco 相关 8 个：安装写两个文件并注册、幂等、卸载对称、`tui.json` 里用户条目保留、TUI 插件不进 `plugins/`、缺 TUI 插件时判 `outdated`、配置目录缺失报错、目录解析遵循 XDG 与 OHOS 沙箱例外。

### 8.3 端到端（`agent-tests/e2e/test_hooks.py`）

四个 agent 各跑一轮真实 TUI：`integration.list` 断言可用与 `current` → `agent.start` → 处理启动提示 → 发一轮带工具调用的提示 → 断言 `working` 出现过且收尾回到 `idle`/`done` → 断言 `completion_seq` 非空 → 断言 `agent_session.source` 是 `herdr:<agent>`。

deveco 与 zcode 额外跑一轮 blocked：deveco 临时把 bash 权限改成 `ask`（文本级插入，退出按原字节还原），zcode 用一条它会自己拦下的命令；断言 `blocked` 且 `agent_session.source` 正确。

当前结果：42 项检查、0 失败。

### 8.4 测试里必须处理的环境事实

- **TUI 会吞掉启动后的第一个回车**：文字进了输入框但没提交，状态停在 idle。做法是送完文字等 1 秒再回车，之后 12 秒内状态没变化就再按一次（最多 3 次）。
- **deveco 每次启动都问目录信任**（`Safety Check:`，只有 session 级选项），几秒后还可能弹一次 `Update Available`。两者都挡住输入框，所以「发消息前」要再扫一遍启动提示，不能只在启动时扫一次。
- **`agent.start` 会撞上 `agent_pane_busy`**：新建 space 的 root pane 还没交出可用 shell。重试即可，不是缺陷。
- **zcode 需要 `TMPDIR` 指到 `/data`**：它要在 `os.tmpdir()` 下 bind broker socket，`/storage`（hmdfs）上 `listen` 报 `EPERM`，进程直接退出。托管块必须同时覆盖正常轮与 blocked 轮。
- **codewhale 会弹运行中审批**：不答就一直 working。脚本自动按 `1` 授权一次。

## 9. 已知限制

- **通知的表现层没有自动化覆盖**：侧栏行标记、提示音、应用内 toast 都在 app 侧（`attention_agents` + `toast_is_silent`），`agent.list` / `pane.get` 不暴露 attention 字段。端到端只能锁到「通知的输入条件」——三态与 `completion_seq`。toast 是否真的画出来需要人工看。
- **codewhale / atomcode 无法上报 blocked**：见第 4 节，事件集没有审批语义。要补需要等上游新增事件。
- **deveco 的 `setup` 是空实现**：deveco 实测只走 `tui(api)`。如果将来某个版本改走 V1 的 `setup` 入口，插件不会启动，只能靠「缺 API 时 console.error」这条线索发现。
- **轮询的 250ms 延迟**：状态变化最多晚 250ms 到达面板。对通知与行标记无影响，对「实时进度条」这类用途不够。
- **deveco blocked 的偶发性**：曾出现一次端到端停在 `idle`（疑为升级提示在提交窗口内弹出）。加上「重发前重扫提示」后连续多轮通过，但没有证据表明 100% 不复现。

## 10. 实施切分与验证记录

1. 探测 deveco TUI API 形状（临时探针插件写文件）。验证：看到 `TUI CALLED`，且 `api.route.current` / `api.state.session.status` 可用。
2. 用探针复现「上报能改变 pane 状态」。验证：`idle → working → idle` 三态在 `pane.get` 上可见。
3. 落成资产 + 安装 / 卸载 / 半装判定。验证：Rust 单测 8 项全过。
4. 面板 token 判据与关闭重建。验证：重启 daemon 后 `agent-tui` 的 pid 由 49970 变为 51126，父进程仍是应用。
5. 版本升到 2。验证：设备上应用启动后自动重装，`integration.list` 从 `outdated` 转 `current`。
6. 端到端。验证：42 项检查、连续多轮全过。

## 11. 关键位置索引

- TUI 插件：`herdr/src/integration/assets/deveco/hicodeer-tui-session.js`
- TUI 插件单测：同目录 `hicodeer-tui-session.test.ts`
- 安装卸载：`herdr/src/integration/targets.rs` 的 `install_deveco` / `uninstall_deveco`
- 半装判定：`herdr/src/integration/registry.rs` 的 `deveco_tui_integration_is_valid`
- 版本常量：`herdr/src/integration/mod.rs` 的 `DEVECO_INTEGRATION_VERSION`
- 面板重连：`crates/agent_panel/src/agent_panel.rs` 的 `sync_agent_terminal` / `close_agent_terminals` / `DEPLOY_SETTLE_TIMEOUT`
- 完成标记：`herdr/src/app/actions.rs` 的 `is_completion_transition` 与 `last_agent_completion_seq`
- 端到端：`agent-tests/e2e/test_hooks.py`，说明在同目录 `README.md`
