# OHOS 命令程序解析：把 which 查询转发到 hicodeerd（which crate fork + resolver hook）

## 问题描述 (Problem Description)

OHOS 上 HiCodeer 运行在受限的应用沙箱里（SELinux 域 `debug_hap`），**看不到** hicodeerd
守护进程所在环境（`hishell_hap`）下的目录（`/storage/Users/currentUser/.harmonybrew/...`、`~/.rustup/...`）。
而 HiCodeer 启动的命令（语言服务器、`node`、`git`、`rustup`、`rust-analyzer` 等）**实际都在
hicodeerd 的环境里执行**，因此程序解析也必须用那个环境的 PATH。

默认的 `which` crate 只查**本进程**的 PATH，于是所有只存在于 hicodeerd 环境的程序都解析不到，
相关功能退化成"下载托管版本"或直接失败。修复目标是：**每一次 `which` 查询都转发到 hicodeerd，
由它用自己（`hishell_hap`）的环境解析**。

## 问题表现 (Symptoms)

- LSP 启动失败，界面/日志报：

```
Language server rust-analyzer: error reading latest release: error decoding response
body: request or response body error: operation timed out
```

  这是掉进"下载 rust-analyzer"分支后出网超时的表象，**根因并不在网络**。

- hicodeerd 日志（`/storage/Users/currentUser/HiCodeer/logs/hicodeerd.log`）里看不到
  `which rust-analyzer` 这类查询，说明查询根本没发到 daemon。
- app 沙箱的 PATH 不包含 `.harmonybrew`；同一个目录 `ls` 能看到，app 却解析不到里面的程序。
- **概率性**：有时 daemon 日志又**有** `which rustup` 记录，时有时无。
- 手工从项目面板点开同一个文件**正常** → 排除权限与文件缺失。

## 问题原因 (Root Cause)

### 第一层：`which` 只查进程自己的 PATH

app 在 `debug_hap` 域够不到 hicodeerd 环境，进程 PATH 里没有 `.harmonybrew`。
`which` crate 的全部公有入口（`which_in` → `which_in_all`）都收敛到 `Finder::find`，
而它查找的是**本进程**的 PATH，因此必然 miss。

### 第二层：`Finder::find` 的早退跳过了 hook（更隐蔽）

在 `Finder::find` 里，`paths` 来自调用方传入的 `shell_path`：

```rust
let paths = paths.ok_or(Error::CannotGetCurrentDirAndPathListEmpty)?;
let paths = self.sys.env_split_paths(paths.as_ref());
if paths.is_empty() {
    return Err(Error::CannotGetCurrentDirAndPathListEmpty);
}
```

这段**提前 return**，位于 hook 兜底代码**之前**。而 `shell_path` 来自
`LspAdapterDelegate::shell_env()`，它在 **app 启动早期可能为空**（环境捕获
`<shell> -l -c '<zed> --printenv'` 尚未完成）。于是：

- `shell_env()` 非空时 → 本地扫描 miss → 走到 hook → 转发成功（日志里有 `which`）
- `shell_env()` 为空时 → **早退** → 永远到不了 hook（日志里没有 `which`）

这正是"时有时无"的来源：窗口只有启动初期一小段，恰好落在窗口里的查询就丢。

### 注意：LSP 症状有两个独立根因

本次排查中 LSP 的 `rust-analyzer --help` exit=1 → 掉进下载分支，**还有第二个根因**：
`rust-toolchain.toml` 被写成了本机未安装的版本号（`1.98.1`），rustup shim 因此直接失败。
那与本方案无关，已另行修复（该文件只决定 LSP 的 toolchain，不决定编译版本——`script/bundle-ohos`
硬编码 `RUSTUP_TOOLCHAIN=1.97.1` 会压过它）。

## 解决方案 (Solution)

### 关键洞察

所有 `which` 入口都收敛到 `Finder::find` 一个函数，所以**只需在这一处挂 hook**，
不必改动 14 个调用点（`node_runtime.rs`、`lsp_store.rs`、`dap_store.rs`、`git_store.rs`、
`shell.rs`、`environment.rs`、`askpass.rs`、`auto_update.rs`、`extension_builder.rs`、
`remote/transport*.rs`、`gpui_util/src/lib.rs` 等）。

### 实现

1. **fork `which` crate** 到 `patches/which`（基于 which 8.0.5），在 `Cargo.toml` 的
   `[patch.crates-io]` 注册 `which = { path = "patches/which" }`。

2. **`patches/which/src/hook.rs`（新增）**：
   - `pub type Resolver = dyn Fn(&OsStr) -> Option<Vec<PathBuf>> + Send + Sync + 'static;`
   - `OnceLock<Box<Resolver>>` + `set_resolver` / `intercept`
   - **thread-local `DEPTH` 递归守卫**：resolver 的实现自己也会解析程序（它把查询交给
     hicodeerd 环境），这样的嵌套查询不能再进 resolver，否则无限递归。嵌套查询直接保留
     本地结果。
   - `DepthGuard` 用 `Drop` 恢复计数，unwind 时也正确。

3. **`patches/which/src/finder.rs`**：在 `Finder::find` 末尾加 OHOS hook 块
   （`finder.rs:122-134`）。**本地扫描保持权威**——只有本地结果为**空**时才问 resolver：

```rust
#[cfg(target_env = "ohos")]
{
    let mut found: Vec<PathBuf> = ret.collect();
    if found.is_empty() {
        if let Some(from_resolver) = crate::hook::intercept(hook_name.as_os_str()) {
            found = from_resolver;
        }
    }
    return Ok(found.into_iter());
}
#[cfg(not(target_env = "ohos"))]
return Ok(ret);
```

4. **早退兜底**（`finder.rs:95-114`）：OHOS 下 `paths` 缺失或拆开后为空时**不再 `return Err`**，
   改为拿一个空列表继续往下走，从而自然流到上面的 hook 块：

```rust
#[cfg(target_env = "ohos")]
let paths = match paths {
    Some(paths) => self.sys.env_split_paths(paths.as_ref()),
    None => Vec::new(),
};
#[cfg(not(target_env = "ohos"))]
let paths = {
    let paths = paths.ok_or(Error::CannotGetCurrentDirAndPathListEmpty)?;
    let paths = self.sys.env_split_paths(paths.as_ref());
    if paths.is_empty() {
        return Err(Error::CannotGetCurrentDirAndPathListEmpty);
    }
    paths
};
```

5. **`crates/util/src/command/ohos.rs`**：安装并实现 resolver。
   - `init`（`:210`）里加一行 `install_remote_which_resolver();`（`:217`）
   - `install_remote_which_resolver`（`:240`）：`which::set_resolver(Box::new(|name| resolve_in_daemon(name)))`
   - `resolve_in_daemon`（`:251`）：`executor().resolve_program(&name)` → hicodeerd，
     返回该路径；**任何失败都读作"那里没有"**，不向上抛错，调用方保留本地结果
   - `report_resolver_failure`（`:273`）：按故障节流，一次故障只 warn 一次（启动会连续探测很多程序）

6. **`resolve_local_program` 去掉 `which::which` 回退**：原来先试进程 PATH、再试 HNP 私有目录；
   现在只认 HNP 私有目录。理由：PATH 里能解析的名字是**给 daemon 用的**，本地 `exec`
   不应该走它，否则会把"该转发"的名字误判成可本地执行。

7. **全部门控**：fork 的每一处差异都包在 `#[cfg(target_env = "ohos")]` 里
   （`[patch.crates-io]` 是全局的、没有 per-target 门控，所以行为等价只能靠逐处 cfg 保证）。
   其他平台拿到的仍是上游原样。

8. **日志纪律**：正常路径**不加日志**。新增的日志只有两处 warn，都在异常路径
   （resolver 重复安装、daemon 查询失败），且按故障节流。

### 被否决的备选：一次性快照

早期实现是"启动时枚举沙箱工具、拍一张名字→路径的快照"，后来**全部回退**。原因：
- 实测在启动竞争下赶不上 `which("node")`（快照还没生成，查询已经发生）
- 与"**每一次**查询都真的发到 hicodeerd"的要求不符 —— 快照是死数据，daemon 环境变化后不会跟

## 走过的弯路（重要，避免重复）

1. **"本地 PATH 扫描应该能命中 rustup bin"** → 错。app 沙箱根本看不到 `.harmonybrew`，
   相关目录只在 hicodeerd 环境里可见。
2. **"daemon 侧一定有 which 查询"** → 不能想当然。必须**读 daemon 日志**确认查询是否真的
   到达；日志里没有就是没到达，而不是"daemon 没记"。
3. **只修"转发通路"就以为完事** → 还有第二层的 `Finder::find` 早退。表现为"时有时无"，
   只有把日志按时序对齐才能看出是 `shell_env()` 为空那一瞬间漏掉的。
4. **把 LSP 的 `operation timed out` 当成网络问题** → 它是"下载分支"的表象，
   真正卡点是本地 toolchain 版本号写错，属于另一个 bug。

## 验证 (Verification)

hicodeerd 日志 `2026-09-28 17:44:14` 一轮全部为成功（`exit=0`）：

```
17:44:14 which -- 'rustup' || true                          exit=0 done
17:44:14 rustup which rust-analyzer                         exit=0 done
17:44:14 .../1.97.1-aarch64-unknown-linux-ohos/bin/rust-analyzer --help   exit=0 done
17:44:14 .../rust-analyzer                                   （无 exit 行 = LSP 进程持续运行）
```

`which rustup` / `which rust-analyzer` 出现在 daemon 日志里即是转发生效的直接证据：
app 沙箱看不到 `.harmonybrew`，这两个名字只能由 hicodeerd 用自己环境解析。

**如实说明**：早退兜底（第二层）覆盖的是"`shell_env()` 为空"的瞬间，在日志里与
"本地扫描失败后走 hook"记录的是**同一条** daemon 记录，无法单独区分哪一次走的是兜底。
两条路径现在都通向 hicodeerd，需求已满足。

## 修改文件 (Modified Files)

- `Cargo.toml` — `[patch.crates-io]` 注册 `which = { path = "patches/which" }`，附注释说明
  hook 只对 OHOS 生效。注意该文件同时还有 zed 升级带来的其它差异，本次只涉及 which 那几行。
- `Cargo.lock` — 跟随 patch 更新锁文件。
- `patches/which/` — **新增（git 未跟踪）**，which 8.0.5 的 fork；除下列文件外均与上游一致：
  - `patches/which/src/hook.rs` — **新增**：`Resolver` 类型、`OnceLock`、`set_resolver`/`intercept`、
    thread-local `DEPTH` 递归守卫与 `DepthGuard`。
  - `patches/which/src/finder.rs` — `Finder::find` 末尾加 OHOS hook 块（本地扫描权威优先）；
    早退兜底：OHOS 下 `paths` 缺失/为空不再 `return Err`，改走空列表以到达 hook。
  - `patches/which/src/lib.rs` — 加 `#[cfg(target_env = "ohos")] mod hook;` 与
    `pub use crate::hook::{Resolver, set_resolver};`。
- `crates/util/src/command/ohos.rs` — 加 `install_remote_which_resolver` / `resolve_in_daemon` /
  `report_resolver_failure` 与 `RESOLVER_FAILURES` 计数；`init` 中调用安装；
  `resolve_local_program` 去掉 `which::which` 回退，只认 HNP 私有目录。
- `crates/gpui_ohos/depend/launch-zed/src/launch_app.rs` — **未改动**（`util::command::init("")`
  调用点在 HEAD 中已存在，安装动作收在 `init` 内部完成）。

[[ohos-debug-lessons]]
