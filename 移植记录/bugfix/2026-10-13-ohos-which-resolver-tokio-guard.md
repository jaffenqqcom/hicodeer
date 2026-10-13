# HiCodeer 找不到系统 node：which resolver 在 tokio 线程上被拒

> 相关定位手段见 [[ohos-debug-lessons]]（hilog 按 tag 过滤、/proc 读进程环境、版本号不可推断二进制同源）。

## 问题描述

设备上已经装了可用的 node（harmonybrew 的 `/storage/Users/currentUser/.harmonybrew/bin/node`，v26.10.0，musl + 已 OHOS 签名），但 HiCodeer 每次启动仍然去下载自己的 managed node，且下载下来的那份在设备上根本执行不了。应用设置里没有配置任何 node 路径（`config/settings.json` 无 `node` 段），代码里的选择顺序也明确是「先找系统 node，找不到才下载」（`crates/node_runtime/src/node_runtime.rs:115-161`），所以问题不在配置，而在「找系统 node」这一步被判成了失败。

## 问题表现

- HiCodeer 应用数据目录下持续出现下载产物：`/storage/Users/currentUser/HiCodeer/node/node-v24.11.0-linux-arm64/`（glibc 构建、无 `.codesign`、解释器指向 `/lib/ld-linux-aarch64.so.1`）。
- hilog 中出现两条关键记录（重启后必然复现）：
  ```
  W A00001/com.hicodeer.edit/Zed: command: which resolver query failed: cannot resolve a program synchronously from a tokio runtime
  W A00001/com.hicodeer.edit/Zed: Zed managed Node.js binary at /storage/Users/currentUser/HiCodeer/node/node-v24.11.0-linux-arm64/bin/node
       failed check with output: Output { status: ExitStatus(unix_wait_status(32256)), stdout: "", stderr: "sh: .../bin/node: Permission denied\n" }
  ```
- 结果：node 既用不上系统的，下载的也用不了；依赖 node 的功能（如 arkts language server）受影响。
- 触发条件：**每次**冷启动都会走到下载分支（不是偶发）。

## 问题原因

应用里 `which::which("node")` 走 OHOS 专用的 resolver 转发给 hicodeerd 查询，而这次查询被 `resolve_program` 自己的一道护栏挡下，调用方把「查询没发出去」误当成「系统没有 node」。

失败链条（`A ⇒ B` 表示导致）：

1. `SystemNodeRuntime::detect()`（`crates/node_runtime/src/node_runtime.rs:924-928`）先 `which::which("node")` 取路径，拿到路径才谈得上使用。
2. app 沙箱内本地 PATH 扫描必然失败：app 的 PATH 看不到 harmonybrew；即使看到，app uid `20020233` 对 `-rwxrwx---`（owner 20001006 / group file_manager）的文件落在 other 位 = 0，`which` 的 `is_valid_executable` 也判否。
3. 于是走 `patches/which` 的 resolver（`patches/which/src/finder.rs:128-133`：本地扫描为空时才 `intercept`，且 resolver 返回值**不再做可执行性校验**）。resolver 实现为 `resolve_in_daemon`（`crates/util/src/command/ohos.rs:253`）→ `SshCommandExecutor::resolve_program`。
4. **断点**：`resolve_program` 开头的护栏（`crates/gpui_ohos/depend/cmd-agent/cmd-client/src/executor.rs:221` 旧版）：
   ```rust
   if tokio::runtime::Handle::try_current().is_ok() {
       return Err(std::io::Error::new(
           std::io::ErrorKind::WouldBlock,
           "cannot resolve a program synchronously from a tokio runtime",
       ));
   }
   ```
   查询体内部要 `self.pool.runtime().block_on(...)` 等一次 SSH 往返，而 tokio 不允许在 runtime 自己的线程上再 `block_on`（thread-local 检查，命中即 panic「Cannot start a runtime from within a runtime」）。作者用这道护栏把 panic 换成 Err。
5. 事实是**调用方确实在 tokio worker 线程上**：日志线程号 20364/20365，线程名 `tokio-rt-worker`（`/proc/<pid>/task/<tid>/comm` 实读）。护栏注释里「runs on the GPUI thread or a plain background thread」的假设不成立。
6. 护栏返回 `Err(WouldBlock)` ⇒ resolver 返回 `None`（`ohos.rs:267-271` 只 warn 一次就返回 None）⇒ `which::which` 报 `CannotFindBinaryPath` ⇒ `DetectError::NotInPath` ⇒ 落到 managed 下载（`node_runtime.rs:153-169`）。
7. 附带第二层：下载源的平台标记由 `consts::OS` 拼出，OHOS target 报 `linux`，于是下到 glibc 的 `node-v24.11.0-linux-arm64`；该二进制无 `.codesign`，内核拒绝执行，`node --version` 报 `Permission denied`（`node_runtime.rs:670`），所以即便下载了也用不了。

**本质**：resolver 的契约只有「这个路径」和「那里没有」两种结果，`resolve_in_daemon` 把「查询因环境被否决」也并进了「没有」。故障被伪装成正常结果，且伪装得毫无痕迹（只有一条 warn）。

### 排查中排除的错误假设（死路）

- ❌「settings 里指定了 node 路径」——`HiCodeer/config/settings.json` 无 `node` 段；`settings.json:59` 里那行 harmonybrew node 是 claude-acp 的 agent_server 命令，与 node runtime 无关。
- ❌「hicodeerd 起不来 / 解析不到」——用 daemon 的 uid（20020117）实测 `/bin/sh -c "which -- 'node' || true"` 返回 `/storage/Users/currentUser/.harmonybrew/bin/node`，`node --version` = v26.10.0。
- ❌「node 命中本地 HNP 名单所以走本地执行」——解包 `git.hnp` / `hishell.hnp` / `hicodeerd.hnp`，三者均不含 node/npm。
- ❌「走了 QEMU guest 后端」——`script/bundle-ohos:49` 的 `ENABLE_QEMU_AGENT=false`，实走 `register_ohos_backend`（daemon）。
- ❌「app 无权限执行沙箱外的系统 node」——权限事实成立（app 不在 file_manager 组），但命令按设计交 daemon 执行，不是直接原因。
- ❌「换成 harmonybrew bottle 下载源就好」——该方案先前的落地已被回退（即使下载了也运行不了）。

## 解决方案

**关键认识**：护栏本身没错（在 runtime 线程上 `block_on` 必 panic），错在「就地执行」是唯一实现。解法是把查询**挪到一个不带 tokio 上下文的线程**去执行——那个线程 `try_current()` 为 Err，`pool.runtime().block_on(...)` 完全合法（从 runtime 外部驱动它）。

在 `crates/gpui_ohos/depend/cmd-agent/cmd-client/src/executor.rs` 引入 `ResolveQueryWorker`：

- 一个常驻线程（`OnceLock` 惰性启动，线程名 `cmd-client-resolve`），持有 `Arc<Pool>`，循环接收 `(name, reply)` 请求。
- `resolve_program` 按上下文分流：

```rust
fn resolve_program(&self, name: &str) -> std::io::Result<Option<PathBuf>> {
    // 非 tokio 上下文：原地执行（与改动前完全一致）
    if tokio::runtime::Handle::try_current().is_err() {
        return resolve_program_query(&self.pool, name);
    }
    // tokio 上下文：交给无 tokio 上下文的专用线程，而不是拒绝
    let worker = self.resolve_worker
        .get_or_init(|| ResolveQueryWorker::start(self.pool.clone()));
    worker.resolve(name)
}
```

- 原查询体抽成自由函数 `resolve_program_query(pool: &Pool, name: &str)`，逻辑不变。

**code review 后追加的两道防线**：

- `recv_timeout(RESOLVE_REPLY_TIMEOUT = 8s)`：8s 高于「allocate 预算 3s + SSH 超时 2s」的最坏值，用于在 worker 卡死时释放调用方，避免把一个 tokio worker 永久挂住。
- 单次查询 `catch_unwind(AssertUnwindSafe(...))`：一次 panic 不再打掉 worker 线程（否则请求通道断开，进程存活期间所有 which 查询都会失败，等于又回到「找不到 ⇒ 下载」）。

**为什么不选其它方案**：

- `tokio::task::block_in_place`：只在「当前 runtime 是 multi-thread 且当前线程是其 worker」时成立，项目里并存 current-thread runtime（`cmd-client/src/bootstrap.rs:58`），落到那儿会 panic——等于把「返回 Err」升级成「崩溃」。
- 调用侧用 `spawn_blocking` 包住 `which` 调用：能解 node 这一处，但改的是上游 `crates/node_runtime`（需 cfg 门控），且其它在 tokio 上下文里发起的 which 查询仍会静默失效。

**验证**（`./install-local.sh` 覆盖安装 + 重启，14:10）：

```
I com.hicodeer.edit/Zed: using Node.js found on PATH: SystemNodeRuntime {
    node: "/storage/Users/currentUser/.harmonybrew/bin/node",
    npm:  "/storage/Users/currentUser/.harmonybrew/bin/npm",
    scratch_dir: "/storage/Users/currentUser/HiCodeer/node" }
I com.hicodeer.edit/Zed: starting language server process.
    binary path: "/storage/Users/currentUser/.harmonybrew/bin/node",
    args: [".../zed-ets-language-server/index.js"]
```

- `which resolver query failed ... tokio runtime` 与 `using Zed managed Node.js ...` 均消失；下载 node 的 `Permission denied` 不再出现。
- `HiCodeer/node/` 下只剩 `cache`（SystemNodeRuntime 的 scratch），`node-v24.11.0-linux-arm64` 已不存在。
- 编译判据：`bash ./script/bundle-ohos` → `EXIT=0` + `HAP BUILD SUCCESSFUL`。

**尚未处理（规模小，暂接受）**：worker 为单线程串行且 `pool.allocate()` 不在 SSH 超时约束内，并发查询会排队（队头最坏 3s）；worker 无 shutdown 路径（持有 `Arc<Pool>`）；which 的递归保护 `DEPTH` 是 thread_local，查询换线程后对新流程失效（当前 SSH 路径不含 which 调用，不触发）。

## 修改文件

- `crates/gpui_ohos/depend/cmd-agent/cmd-client/src/executor.rs` — 新增 `ResolveQueryWorker` + `ResolveRequest` + `resolve_program_query`；`SshCommandExecutor` 增加 `resolve_worker: OnceLock<..>` 字段；`resolve_program` 改为按 tokio 上下文分流（非 tokio 原地、tokio 走 worker）；新增常量 `RESOLVE_REPLY_TIMEOUT`；`use std::sync` 增加 `OnceLock`。
