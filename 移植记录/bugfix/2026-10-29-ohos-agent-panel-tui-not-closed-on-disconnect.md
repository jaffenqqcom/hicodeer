# 断连后 Agent TUI 不关闭

## 问题描述 (Problem Description)

HiCodeer 的 Agent panel 每秒轮询 daemon（hicodeerd 内嵌的 herdr server）取 Agent 状态。当 daemon 侧与面板之间的通信断掉时，面板已经显示"运行 hicodeerd 解锁功能"的断开提示，但 workspace 里那个 Agent TUI 终端 tab 一直留着。典型现场是 TUI 里已经打出 `herdr: server shut down: server is shutting down` 后进程退出，tab 变成一具"死壳"，面板却没有任何关闭动作。期望行为：面板一旦读不到信息就关掉 TUI，daemon 恢复后自动重开。

## 问题表现 (Symptoms)

- Agent panel 显示断开提示（"Run "hicodeerd" in your system terminal…"），但 Agent TUI tab 仍在，内容是 `herdr: server shut down: server is shutting down` 或已停止的进程画面。
- 该状态可以一直持续（不是刷新间隙），面板继续每秒轮询、继续报失败，tab 始终不关。
- 只有 daemon 换代（重启后 token 变化）时 tab 才会被清掉；单纯断连不会。
- 复现：打开一个 Agent TUI → 让 hicodeerd 退出或被停掉 → 观察面板与 tab。

## 问题原因 (Root Cause)

关闭动作本身只有一条实现，但"什么时候算断开"被拆成了三套互不一致的判断，且其中两套都走不到关闭：

1. **显示层**：`apply()` 收到 `Err(NotReady)` 或 `Err(Failed)` 都设置 `unavailable`，渲染时只要 `unavailable.is_some()` 就显示断开提示。所以界面很早就"知道"断了。
2. **关闭触发层（第一套）**：`Failed` 分支只做 `consecutive_failures += 1`，注释还明确写着"A failed request is not a disconnection"，从不置 `wanted = false`。只有 `NotReady` 会走 `note_disconnect()`。
3. **关闭触发层（第二套）**：`note_disconnect()` 用 `consecutive_failures == DISCONNECT_CLOSE_THRESHOLD`（精确等于 3）判断，NotReady 与 Failed 交替计数时会直接跨过 3，永远不触发。
4. **关闭执行层**：`sync_agent_terminal()` 开头 `let Some(control) = cmd_client::agent_control() else { return; }`。断连时 bootstrap 已把 control 清空，于是即使 `wanted` 已经为 false，函数也在第一行直接返回，**永远走不到关闭分支**。

本次现场正是第 2、4 条的组合：herdr server 关闭后 daemon 端点还在、bridge 还在监听，面板连得上但请求失败，拿到的是 `ClientError::Failed`；`Failed` 不置 `wanted=false`，即便置了，`agent_control()` 仍可能为 `Some`，但旧代码里若 control 为 `None` 就根本进不来。

ASCII 对照（旧）：

```
refresh 失败 ──┬─ NotReady ──► note_disconnect ──(==3)──► wanted=false ──► sync: control=None ──► return  ← 断在这
               └─ Failed   ──► 只加计数 ────────────────────────────────────────────────────────────► 永不关
```

TUI 侧的行为（记录，不作为修复点）：`herdr/src/client/mod.rs:396` 在 `ConnectionLost` / 非 detached 的 `ServerShutdown` 时 `exit(1)`。这是 herdr TUI 客户端的通用收尾，三个入口共用（herdr 自身 `client` 子命令、`hicodeerd agent-tui`、autodetect 后 attach），不是 Agent TUI 专用；它做的是"进程退出"，不是"关闭 tab"。删掉它会让通用客户端断连后挂死，因此不动。

## 解决方案 (Solution)

最终形态：**一套判定**——刷新失败即视为断开，成功即视为可用；关闭与开启都放在 `sync_agent_terminal` 这一个地方，并且都先检查 tab 是否存在（幂等）。

`apply()` 失败分支（`crates/agent_panel/src/agent_panel.rs:525-561`）：

```rust
Err(error) => {
    match &error {
        ClientError::NotReady => { /* 略 */ self.unavailable = Some(Unavailable::WaitingForServer); }
        ClientError::Failed(message) => {
            if self.consecutive_failures == 0 { log::warn!("agent panel: refresh failed: {message}"); }
            self.unavailable = Some(Unavailable::ServerUnavailable);
        }
    }
    self.consecutive_failures += 1;   // 只用于"一次故障一行日志"
    self.wanted = false;              // 显示断开 = 关 TUI
    self.needs_sync = true;
    self.clear_history();
}
```

`sync_agent_terminal()` 的统一判定（`:786-864`）：

```rust
let control = cmd_client::agent_control();
let present = workspace_has_agent_terminal(workspace.read(cx), cx);
let owned_by_current_daemon = matches!((&control, &self.deployed_agent_token),
    (Some(c), Some(t)) if c.token == *t);
let replaced_daemon = control.is_some() && !owned_by_current_daemon;

if present && (!self.wanted || replaced_daemon) {
    self.deployed_agent_token = None;
    close_agent_terminals(workspace, window, cx);   // 幂等：按实际存在的 tab 遍历
    if self.wanted { self.needs_sync = true; }
    return;
}
if !self.wanted || present { return; }              // 已开着就不再开
```

删除的代码：

- `note_disconnect()` 整个函数（"连续 3 次才算断开"这套条件）。
- `DISCONNECT_CLOSE_THRESHOLD` 常量。
- `Failed` 分支里单独累加计数、不参与断连判定的路径。
- `sync_agent_terminal` 开头"拿不到 control 就 return"的提前返回。

代价（已与使用者确认并接受）：任意一次刷新失败（最长 3 秒连接/读超时）就会关 TUI。设备卡顿或 daemon 忙时可能闪断后自动重开一次。

曾考虑并放弃的方案：

- 删掉 herdr client 的 `ConnectionLost → exit(1)`：那是三处共用的通用收尾，删了会让 herdr 独立客户端断连后挂住；且它退出后 tab 仍需有人关。
- 按错误文本区分"连接失败"与"读超时"：判据脆（bridge 代理可能接受连接后才 EOF），且与新口径"显示断开即关"重复。
- 在 `terminal` / `terminal_view` 里加连接检测：这两个 crate 只负责 pty 与视图，`is_agent_terminal()` 仅看 `pinned_title`，与 herdr 无任何连接状态，不应引入第二处判定。

另有一处发现未改（使用者决定不区分）：`Unavailable::{WaitingForServer, ServerUnavailable}` 目前只被 `is_some()` 使用，UI 不区分两者文案。

## 修改文件 (Modified Files)

- `crates/agent_panel/src/agent_panel.rs` — `apply()` 失败分支合并为一处并直接置 `wanted=false`；删除 `note_disconnect` 与 `DISCONNECT_CLOSE_THRESHOLD`；`sync_agent_terminal()` 合并为"present 才关、present 才不重开"的单一判定；同步更新 `wanted`、`consecutive_failures`、`render_disconnected` 的注释。
- `crates/agent_panel/src/client.rs` — 历史列表超时（`SESSIONS_READ_TIMEOUT`）的注释改为"失败即会被当作断开并关闭终端"，与新的判定一致。

验证：

- `bash ./script/bundle-ohos` 全量通过：Rust `Finished dev profile`（8m02s）、hicodeerd/hishell HNP 打包成功、`BUILD SUCCESSFUL`、签名与验签通过。
- `./install-local.sh` 装机并启动成功（`install bundle successfully` / `start ability successfully`）。
- 未做设备侧人工复现（停 daemon 观察 tab 关闭、重启 daemon 观察自动重开），待现场确认。

参见 [[ohos-debug-lessons]]。
