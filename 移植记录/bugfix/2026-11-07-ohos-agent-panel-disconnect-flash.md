# Agent panel 启动时闪现「先启动 hicodeerd」提示

## 问题描述 (Problem Description)

先启动 `hicodeerd`、再启动 HiCodeer 时，Agent panel 会先正常显示出 daemon 里已经跑着的 agent，随即整棵树被「Run "hicodeerd" in your system terminal to unlock all HiCodeer features.」提示替换，约一秒后又自动恢复成 agent 列表。daemon 明明在运行，提示却要求用户去终端启动它。该闪现可稳定复现，且每次启动都会发生。

## 问题表现 (Symptoms)

- 面板出现三段式闪烁：agent 列表 → 断开提示 → agent 列表，全程约 2~4 秒。
- 提示文案与事实矛盾：daemon 已在运行，用户不需要执行任何命令。
- 同一次失败还会把 `wanted` 置 false，Agent 终端 tab 跟着被关闭再重建（终端内容闪断）。
- 触发条件：daemon 冷启动后的第一次历史扫描（约 3 秒）恰好与面板每秒一次的状态轮询重叠。
- 复现步骤：重启 `hicodeerd` → 保持 HiCodeer 运行或重新启动它 → 观察 Agent panel。

## 问题原因 (Root Cause)

根因不是「daemon 掉线」，而是**daemon 的主循环被一次磁盘扫描占住**，面板把由此产生的单次读超时误判成了断开。

完整链路：

1. 面板每秒刷新一次（`crates/agent_panel/src/agent_panel.rs:60` 的 `REFRESH_INTERVAL`，循环在 `:463`）。
2. 第一次成功刷新后立刻补拉历史会话（`agent_panel.rs:555`：`history.is_none() && !history_loading` 就调 `refresh_history`）。
3. daemon 的 `agent.sessions` 在 App 主循环里**同步**执行：`herdr/src/app/agents.rs:99-114` 直接调 `sessions::collect_sessions`，调用点 `herdr/src/app/api.rs:1116` 是 `return self.handle_agent_sessions(request.id)`，没有 spawn。冷启动后首次扫描要真解析全部会话库，实测 2.97~3.37 秒。
4. 面板普通 RPC 的读超时是 3 秒（`crates/agent_panel/src/client.rs:36` 的 `READ_TIMEOUT`）。扫描期间排队的 `workspace.list`/`agent.list` 正好撞上超时，`apply` 收到 `ClientError::Failed`。
5. 旧口径是「一次失败即断开」（2026-10-29 为修「断连后 TUI 不关闭」引入的代价，见 `2026-10-29-ohos-agent-panel-tui-not-closed-on-disconnect.md`）：失败分支直接置 `unavailable`、`wanted = false` 并 `clear_history()`。
6. 渲染只看 `unavailable.is_some()`（`agent_panel.rs:3238-3240`），整屏换成 `render_disconnected` 的提示；扫描结束后下一次轮询成功又把列表画回来。

排查中排除的假设（都不成立）：

- **不是 daemon 重启 / token 变化**：daemon 进程启动时间早于 app，动态 token 未变；bootstrap 的管理连接一直持有，`set_agent_control(None)` 只在管理通道 `Connection` 失败时才执行。
- **不是 agent server 没起来**：`agent_control::start` 与 `start_agent_server` 都在 daemon 启动时执行一次，不随客户端变化。
- **不是历史请求自己超时**：历史请求有 20 秒的 `SESSIONS_READ_TIMEOUT`，冷扫 3 秒远未到；超时的是每秒状态轮询的 3 秒预算。
- **不是面板初始状态抖动**：`unavailable` 初值为 `Some(WaitingForServer)`，能先显示列表说明首次刷新已经成功，问题发生在成功之后。

由代码路径可直接判定阻塞点：`handle_agent_sessions` 在 API 派发线程上同步返回，扫描期间同一循环无法处理任何其它请求。

## 解决方案 (Solution)

两件事一起做：**daemon 侧解除主循环阻塞（治根）**，**面板侧单次失败不降级（容错）**。

### 1. `agent.sessions` 移出主循环

新增 deferred 处理器（`herdr/src/app/api/agents.rs:64`），主循环只做两件事：快照 `workspace_roots`、把请求交给后台线程；扫描完成后经 `respond_to` 回写响应。派发处（`herdr/src/server/headless.rs:3049`）在 `AgentPrompt` 分支旁拦截：

```rust
if matches!(&msg.request.method, api::schema::Method::AgentSessions(_)) {
    let deferred_changed = self
        .app
        .handle_deferred_agent_sessions_request(msg.request, msg.respond_to);
    return changed | deferred_changed;
}
```

### 2. 并发请求单飞（review 阶段补的第二个缺陷）

只把扫描挪到线程还不够：`sessions::fad` 的缓存是**进程级** `static CACHE: Mutex<Option<HashMap<..>>>`（`herdr/src/sessions/fad.rs:59`），`collect()` 开头 `CACHE.lock().take()`（`:64`）把整张表取走，扫完再写回。两个并发扫描会互相把缓存掏空，双双退化为全量解析，最后写回的 `next` 还覆盖另一方的结果；面板在 20 秒超时后重试，还会不断叠加线程（每个 8 MiB 栈）。

因此加了等待队列（`herdr/src/app/mod.rs:142` 的 `pending_session_scan_requests`）：扫描在飞时，后来的请求把 `(id, respond_to)` 压进队列直接返回；扫描线程结束后一次性给所有等待者回写同一份结果，各自带自己的 `id`。`None` 表示当前没有扫描。

### 3. 栈大小与 panic 兜底

扫描线程用 8 MiB 栈（`AGENT_SESSION_SCAN_STACK_SIZE`，`agents.rs:23`）：FAD 连接器经 `fsqlite` 读 SQLite，pager 在调用线程栈上建状态，debug 构建单帧约 86 KiB，默认 2 MiB 会在嵌套打开路径上溢出。扫描体用 `catch_unwind` 包住，panic 时把等待队列取走并逐条回 `agent_sessions_failed`，避免槽位被永久占死、后续请求全部石沉大海。

### 4. 面板：单次失败不降级

`crates/agent_panel/src/agent_panel.rs:68` 新增 `REFRESH_FAILURES_BEFORE_DISCONNECT = 3`；失败分支（`:573`）改为：

```rust
let disconnected = match &error {
    ClientError::NotReady => true,                    // 端点真的没了，立即降级
    ClientError::Failed(message) => {
        if self.consecutive_failures == 0 { log::warn!("agent panel: refresh failed: {message}"); }
        self.consecutive_failures + 1 >= REFRESH_FAILURES_BEFORE_DISCONNECT
    }
};
self.consecutive_failures += 1;
#[cfg(target_env = "ohos")]
{ self.status_recorded = false; }                    // 任何失败都重建通知基线
if disconnected { /* unavailable / wanted=false / clear_history */ }
```

未降级期间保留上一份快照、不关 Agent 终端、不清历史；三次失败按每次 3 秒读超时算至少 9 秒，覆盖冷扫的 3 秒。`NotReady` 仍立即降级。睡眠抑制守卫改为 `unavailable.is_none() && consecutive_failures == 0`（`:630`），失败期间释放，不拿旧快照硬撑。

### 取舍与放弃的方案

- **不回退成「成功才恢复」的另一极端**：阈值 + 治根，既不会因一次抖动误判，也不会像原口径那样每次失败都关终端。
- **不用「信号量 + busy 错误」**：扫描期间面板会拿到 `sessions_failed`、历史区空置并每秒重试；等待队列复用同一结果更平滑。
- **不改 `fad` 的 take-then-put 缓存**：会牵动 `delete_session` 等其它调用方的语义，用单飞隔离并发更小。
- **不删 `AgentPrompt` 分支的 `changed | deferred_changed`**：那是既有约定，本次按同构写法接入，单独改会造成两个 deferred 分支不一致。

## 修改文件 (Modified Files)

- `crates/agent_panel/src/agent_panel.rs` — 新增 `REFRESH_FAILURES_BEFORE_DISCONNECT`；`apply()` 失败分支区分 `NotReady`/`Failed`，后者连续 3 次才降级并关闭终端；`status_recorded` 任何失败都清零；睡眠抑制判定加入失败计数。
- `herdr/src/app/api/agents.rs` — 新增 `handle_deferred_agent_sessions_request`（后台扫描 + 等待队列 + `catch_unwind` + 8 MiB 栈）、`take_pending_session_scan` 辅助函数，以及两个回归测试（`the_session_scan_does_not_block_the_app_loop`、`a_scan_thread_that_cannot_start_answers_with_an_error`）。
- `herdr/src/app/mod.rs` — 新增 `PendingSessionScanRequest` 类型别名、`pending_session_scan_requests` 字段与初始化，以及 `#[cfg(test)]` 的 `session_scan_test_gate`。
- `herdr/src/server/headless.rs` — 派发处拦截 `agent.sessions` 走 deferred 路径。

说明：`AGENT_SESSION_SCAN_STACK_SIZE` 与 `thread_spawn::spawn_named_with_stack_size` 由并行改动补充（非本次直接编写），但它是该扫描线程能稳定跑 fsqlite 的前提，一并记录。

## 验证

- `./script/bundle-ohos`：`EXIT=0`、`=== HAP BUILD SUCCESSFUL ===`。中间一次在 `sign-app` 阶段偶发 `ERROR: 11010001 ... file length 0 is too smaller`（signed.hap 变 0 字节），原样重跑通过，与代码无关。
- 单元测试（`agent-tests/run-unit-tests.sh`）：
  - `the_session_scan_does_not_block_the_app_loop` — ok（5.99s），断言扫描在飞时 `agent.list` 200ms 内返回。
  - `a_scan_thread_that_cannot_start_answers_with_an_error` — ok（0.04s），断言 spawn 失败回 `agent_sessions_failed` 且槽位释放。
- 设备实测（装包 + 重启 `hicodeerd` 后，直接打 daemon 的 agent API）：
  - `agent.sessions` 冷扫 **3.370s**（269 条），热扫 0.065~0.084s。
  - 冷扫期间 `agent.list` 最大 **0.154s**（空闲基线 0.003s）。
  - 扫描期间 daemon 线程列表出现 `herdr-agent-ses`（comm 截断 15 字节）与 `herdr-api-conn` 并存。
  - 应用侧日志未出现 `agent panel: refresh failed`。
  - 用户目视：面板不再闪烁。
- 尚未生效项：单飞、`status_recorded` 修正与两个测试只编译、未装包；`herdr` 代码编进 `hicodeerd`，HNP 服务不随覆盖装包换新映像，需装包后**再重启 daemon** 才生效（标「未验证」）。

参见 [[ohos-debug-lessons]]。
