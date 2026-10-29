# OHOS 上 agent.prompt 永远报「agent 不在 pane 前台进程」

## 问题描述 (Problem Description)

在 HarmonyOS 设备上，任何"由程序给 pane 里的 agent 下达任务"的调用（`agent.prompt`）都会失败，返回
`agent_not_ready: agent <pane> is no longer the pane foreground process`。但那个 pane 里的 agent 明明活得好好的，
屏幕上正常显示 TUI、`agent.get` 也报 `interactive_ready: true`。

用户手动在面板里打字不受影响——受影响的只有走 API 的自动化路径：编排层派活、角色之间传任务、脚本驱动 agent
全部不可用，多 agent 协同只能靠人盯着点。

同一轮排查里还发现 `pane.process_info` 在 OHOS 上是残的：只返回 `{"shell_pid": N}`，
`foreground_process_group_id` 和 `foreground_processes` 永远为空。

## 问题表现 (Symptoms)

- `agent.prompt` 100% 失败，报 `no longer the pane foreground process`；同一个 pane 的 `agent.get` 显示
  `interactive_ready: true`、`agent_status: idle`，说明 agent 进程确实在。
- `pane.process_info` 只有 `shell_pid`，缺 `foreground_process_group_id` 与 `foreground_processes`。
- 受影响方法可正常工作的有：`agent.start` / `agent.rename` / `pane.send_text` / `pane.send_input` /
  `pane.read` / `agent.wait` / `pane.close`，所以"模拟打字"能绕过，但会与输入框草稿、权限弹窗、用户手打输入打架。
- 复现：在 pane 里启动任意 node/bun 包装的 agent（如 `codebuddy`，实际进程是 `node .../codebuddy`）→
  调 `agent.prompt` → 必现。

## 问题原因 (Root Cause)

三段链，前两段是平台事实，最后一段是真 bug。

### 1. OHOS 的 procfs 不报前台进程组

`/proc/<pid>/stat` 里 `tty_nr` 与 `tpgid` 对所有进程都是 `0`（实测 pane shell：`tty_nr=0 tpgid=0`）。
所以 `platform::foreground_job(pid)` 恒为 `None`——它依赖 `foreground_process_group_id(pid)` 的
`(tpgid > 0).then_some(tpgid)`。

于是 `app/agents.rs` 的判断只能走到第二分支：

```rust
crate::detect::foreground_job(child_pid)                    // OHOS 上必 None
    .or_else(|| {
        crate::detect::foreground_group_leader_job(runtime.pty_foreground_process_group_id()?)
    })
```

PTY 这条路是好的（见下节），所以问题不在"拿不到进程组"。

### 2. PTY 的 tcgetpgrp 其实可用（关键反证）

用 `pidfd_open(434)` + `pidfd_getfd(438)` 复制 `hicodeerd` 持有的 pty master fd 后实测：

```
fd 23 → tcgetpgrp = 25686   （临时 pane 的 shell）
fd 29 → tcgetpgrp = 29756   （pC 里 codebuddy 的进程组）
```

而 `ps` 侧确认 `codebuddy`（comm `node-MainThread`）的 `pgrp == 29756 == 自身 pid`，就是组长。所以
`pty_foreground_process_group_id()` 能拿到正确的 pgid，`foreground_group_leader_job(29756)` 也返回了 job。

### 3. 真 bug：leader job 的 argv0 是空的

`platform/linux.rs::foreground_group_leader_job` 构造组长进程时写的是：

```rust
let mut processes = vec![ForegroundProcess {
    pid: process_group_id,
    name,          // comm = "node-MainThread"
    argv0: None,   // ← 这里
    ...
}];
```

而 `detect::normalized_process_name` 的取值是：

```rust
let effective = process.argv0.as_deref().unwrap_or(&process.name);
```

`argv0` 为空 → 退化到 comm → `"node-MainThread"`（内核线程名）→ `identify_agent` 匹配不上任何 agent。
下游 `runtime_hosts_agent` 返回 false → `agent.prompt` 被拒，报"不在前台"。

对照：`macos.rs` 构造同样的结构时用的是 `process_argv0_name(pid)`，有值。Linux 这条分支漏了——
在正常 Linux 上因为 `tpgid` 可用、走的是另一条 `foreground_job` 路径，这个缺失永远不暴露。
OHOS 被迫走 leader 路径，才踩上。

`pane.process_info` 是另一半：它只调 `detect::foreground_job(shell_pid)`（不经过 leader 分支），
所以 OHOS 上直接拿到 `None`，字段全空。

## 解决方案 (Solution)

### 修正一：给 leader job 补 argv0

`platform/linux.rs::foreground_group_leader_job` 里把 `argv0` 填成 `argv[0]`：

```rust
// 修改前
argv0: None,

// 修改后
argv0: argv.as_ref().and_then(|parts| parts.first().cloned()),
```

前提已验证：读 argv 依赖 `process_allows_remote_memory_read(state, comm, wsl)`，判据只是 `state` 不是
`D/Z/X`（WSL 才额外看 comm）；node 进程 state 是 `S`，`/proc/<pid>/cmdline` 也实测可读。

### 修正二：process_info 复用同一条链

`app/api/panes.rs` 的 `pane.process_info` 与 `agents.rs` 对齐，在 `foreground_job` 失败后回退到 PTY 的前台组：

```rust
let foreground_job = shell_pid
    .and_then(crate::detect::foreground_job)
    .or_else(|| {
        runtime
            .pty_foreground_process_group_id()
            .and_then(crate::detect::foreground_group_leader_job)
    });
```

### 验证结果

- `agent.prompt` 返回 `{"type":"agent_prompted", ...}`，目标 agent 实际收到并回复（屏幕可见 `VERIFY-OK`）。
- `pane.process_info` 恢复为 `shell_pid=19915 fg_pgid=19953`，进程列表含
  `node-MainThread argv0=node`、`gpui-mcp-server argv0=/storage/.../gpui-mcp-server`。

### 走过的死路（重要，别再重复）

1. **误判为 `platform/fallback.rs`**。以为 OHOS 落到了 fallback stub。实际 `rustc --print cfg --target
   aarch64-unknown-linux-ohos` 给出 `target_os="linux"`，走的是 `linux.rs`，fallback 与 OHOS 无关。
2. **在 app 层加 `#[cfg(target_env = "ohos")]` 洞**。曾给 `app/agents.rs`、`app/api/panes.rs` 各塞一段
   descendant 兜底。能跑，但把平台事实泄漏到两个上层调用点，以后再添调用方就得再挖一个；已全部回退
   （`app/agents.rs` 净 diff 归零）。
3. **想用"扫 /proc 按 pgrp 匹配"替换 descendant 扩展**。被否定：agent 可能是孙进程——例如通过 `cbb`
   启动 `codebuddy` 时，agent 自建进程组，按 `pgrp` 枚举反而找不到它，只能沿后代树找。
4. **想直接改 `platform::foreground_job` 加兜底**。会污染 `available_pane_shell`（shell 是否空闲，
   供 `agent.start` 判断目标 pane 是否可启动），把"后代子树非空"误判成"shell 忙"。所以不能动它。

### 约束：不要删 descendant 扩展

`foreground_group_leader_job` 里的

```rust
#[cfg(target_env = "ohos")]
processes.extend(ohos_descendant_processes(process_group_id));
```

是必需的，不是临时补丁——它就是为"agent 是孙进程"这一情况准备的。删掉会让包装器启动的 agent 重新不可见。

## 修改文件 (Modified Files)

- `herdr/src/platform/linux.rs` — `foreground_group_leader_job` 补 `argv0`（`None` → `argv[0]`），并写明原因：
  空 argv0 会退化到 comm（`node-MainThread`）导致 agent 识别失败，OHOS 因 `tpgid=0` 只能走这条路径。
- `herdr/src/app/api/panes.rs` — `pane.process_info` 在 `foreground_job` 失败后回退到
  `pty_foreground_process_group_id()` + `foreground_group_leader_job`，与 `app/agents.rs` 的链保持一致。

相关经验见 [[ohos-debug-lessons]]。
