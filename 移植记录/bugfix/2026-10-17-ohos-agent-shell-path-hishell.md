# agent 的 shell 命令落在 app 沙箱：$SHELL 与 ACP terminal 都没走到 hishell

> 相关定位手段见 [[ohos-debug-lessons]]（/proc 读进程 uid、SELinux 域、父进程；hilog 按 tag 过滤；用 daemon 权限对照同一个路径的可见性）。

## 问题描述

agent panel 里的 codebuddy 通过 ACP `terminal/create` 执行命令时，命令落在 HiCodeer 应用进程的沙箱里（uid `20020233`、SELinux 域 `normal_hap`），看不到 `/storage/Users/currentUser/.harmonybrew` 下的工具；而 hicodeerd（uid `20020117`）能看见。目标是把 agent 的执行链切到 daemon 侧，同时保证 hishell 能安全承载 ACP/MCP 的 `-c` 调用。

同一批改动里还有一处相关修复：调用方把 PATH 作为 `PATH=...` 赋值写进命令字符串后，daemon 直接用 `sh -c` 执行，调用方的 PATH 会整体覆盖 daemon 自己的 PATH，系统目录（`/usr/bin`、`/system/bin` 等）随之丢失，daemon 侧解析不到系统程序。

## 问题表现

- agent 命令里 `ls /storage/Users/currentUser/.harmonybrew/bin` 报 `Operation not permitted`；`which brew` 找不到。
- 同一个命令经 `/data/app/bin/hishell -c` 执行时能列出 `.harmonybrew/bin`，说明权限差异来自执行者，不是文件本身。
- agent 命令的进程：`uid=20020233`、域 `o:r:normal_hap`、父进程是 `com.hicodeer.edit`（app 主进程），`cmdline` 是 `/bin/sh -i -c exec </dev/null...`。
- 把 `$SHELL` 改成 hishell 后，hishell 的三处行为会破坏 ACP：
  - `-c` 模式把子进程 stdin 置空，ACP 的请求（走 stdin）进不去；
  - `exec </dev/null\n<命令>` 的前缀被当成「终端启动 bridge」的交互约定，命令不执行；
  - daemon 不可达时的降级提示直接 `eprintln!`，在 pty 下与 stdout 合并，污染 agent 读到的输出且无法用 `2>/dev/null` 屏蔽。
- 命令字符串里带 `PATH=...` 时，daemon 侧只看到调用方的 PATH，`/usr/bin` 等系统目录丢失，`which`/程序解析失败。

## 问题原因

命令链与两处硬编码：

1. `launch_app.rs` 把进程 `$SHELL` 钉成 `"/bin/sh"`，所有 `Shell::System` 调用者（ACP agent 启动、MCP、git、task）都继承它。
2. `util::shell::get_default_system_shell_preferring_bash()` 在非 Windows 上**直接返回 `"/bin/sh"`**，不看 `$SHELL`；ACP 的 `create_terminal_entity` 用它选 shell（`crates/acp_thread/src/terminal.rs:962`），所以即使 `$SHELL` 改了，agent 的工具命令仍是 `/bin/sh`。
3. `/bin/sh` 由 app 进程内的 alacritty pty fork（`TerminalBuilder` → `open_pty` → `tty::new`），天然是 app uid/域，于是 PATH 与文件访问都受沙箱限制。
4. hishell 原本只服务「交互终端」和「机器解析输出的一次性命令」，对 ACP 这种「双向 stdio 的 `-c` 调用」不适配（见上）。
5. PATH 的归属被弄混了：调用方（app）知道应用目录，daemon 知道系统目录。命令以 `PATH=...` 前缀交给 daemon 时，shell 的赋值会整体替换环境，daemon 的系统目录被覆盖掉。

## 排查中的死路

- ❌「`saving history... completed.` 出现两次是 ACP 链路有两个 zsh」——实测单条命令只有一个 zsh（`ps -o PID,PPID,COMM -p $$`：父进程直接是 hicodeerd），只打印一次；两次来自我在测试命令里又嵌了一个 `zsh -i -c`，外层与内层各触发一次 EXIT trap。据此改调用点（`non_interactive`）的改动已撤销。
- ❌「在 hishell 里过滤 `-i`」——能消掉提示，但篡改了调用方参数；调用方真要交互语义时会被破坏。已撤销，改为尊重传入参数。
- ❌「在 `acp_thread/src/terminal.rs` 给 shell 选择加 OHOS 门控」——两处调用点要各加一次；改为在 `util/src/shell.rs` 一处实现里收敛。
- ❌「`crates/zed/Cargo.toml` 的 lib/bin 共用 `main.rs` 警告是本次引入的 bug」——是 OHOS 移植层加的 `[lib] path = "src/main.rs"`（供 `launch-zed` 的 NAPI cdylib）与上游 `[[bin]]` 共用同一文件导致的 cargo 诊断，构建实际只用 `--lib`，不影响产物。
- ❌「release 编译卡死是代码问题」——是两个 `bundle-ohos`（一个是被中断后残留的）加另一个 `cargo test --offline` 同时抢 target 目录/jobserver，`rustc` 卡在 `FUTEX`。清理进程后即可，非代码缺陷。

## 解决方案

四处改动，分层解决：

1. **进程 `$SHELL` 切到 hishell**（`launch_app.rs`）：`SYSTEM_SHELL_PATH` 改为 `/data/app/bin/hishell`，让所有 `Shell::System` 调用者经 daemon。
2. **默认 shell 不再硬编码**（`util/src/shell.rs`）：`get_default_system_shell_preferring_bash()` 在 OHOS 上返回 `get_system_shell()`（即 `$SHELL`）；Windows/其它 Unix 分支原样保留，三个 `#[cfg]` 互斥块保证每个平台只有一个尾表达式。
3. **hishell 适配 `-c` 调用**（`hishell/src/main.rs`）：
   - 交互判定收紧为必须 `exec -a <argv0> <program>`，普通 `exec ...`（含 `exec </dev/null` 前缀）按命令执行；
   - `stdin_mode` 从 `Null` 改为 `Piped`，并用独立线程转发调用方 stdin（调用方可能长期保持 stdin 打开，不能进主等待路径）；
   - `-c` 模式静默：`finish(result, quiet)` 只在交互模式打印失败；daemon 不可达时只写 `log::error!`，照常 fallback；`SshCommandExecutor::new` 失败保持原行为（退出 1）。
4. **不篡改调用方参数**：hishell 对 `-i`/`-c` 原样转发；是否交互由调用方决定。
5. **daemon 侧合并 PATH**（`hicodeerd/src/exec.rs`）：`spawn_command` 在把命令交给 `sh -c` 之前，扫描命令位置上的 `PATH=` 赋值，把 daemon 自己的 PATH 追加到调用方值之后（caller first，daemon after）；调用方传空值时只留 daemon 的 PATH，避免空条目指向当前目录；命令里没有 `PATH=` 时不改动，直接继承 daemon 环境。只处理 program 之前的赋值，参数里的 `PATH=` 不碰。

验证判据：

- 重装后 agent 命令的 `uid=20020117`，`which brew` → `/storage/Users/currentUser/.harmonybrew/bin/brew`；
- 进程树里 `bun`（codebuddy）的父进程是 `hicodeerd`；
- `zsh -i -c 'echo B'` 手动执行仍会打印 `saving history`（系统 rc 未被改）。

## 修改文件

- `crates/gpui_ohos/depend/launch-zed/src/launch_app.rs` — `SYSTEM_SHELL_PATH` 由 `"/bin/sh"` 改为 `"/data/app/bin/hishell"`，两处注释同步说明「所有 `Shell::System` 调用者经 daemon 执行」。
- `crates/util/src/shell.rs` — `get_default_system_shell_preferring_bash()` 增加 OHOS 分支返回 `get_system_shell()`；非 Windows 分支条件收窄为 `not(any(windows, target_env = "ohos"))`。
- `crates/gpui_ohos/depend/cmd-agent/hishell/src/main.rs` — `exec` 识别收紧为 `exec -a`；`-c` 模式 stdin 改为 Piped 并新增转发线程；`finish` 增加 `quiet` 参数、`-c` 模式不再 `eprintln` 降级提示。
- `crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/exec.rs` — `spawn_command` 在 `sh -c` 之前调用新增的 `merge_path_assignments`，把 daemon 的 PATH 追加到调用方的每个 `PATH=` 赋值之后；配套 `skip_single_quoted` / `skip_double_quoted` / `skip_escaped` / `is_assignment_word` / `is_empty_shell_value` 与 `sh_quote`。

## 遗留

- agent 进程里的 `$SHELL` 仍是 `/bin/sh`：它是从 hicodeerd 继承的，`$SHELL=hishell` 只作用于 app 进程；判断链路是否生效要看命令的 uid/父进程，不能看 agent 里的 `echo $SHELL`。
- 调用方保留 `-i`，因此每条 agent 命令仍会输出一次 `saving history... / completed.`（`/etc/zshrc` 的 `preexit` EXIT trap，同时写一次 `~/.zsh_history`）。要消除只有两条路：改系统 rc（影响所有终端）或改调用方不传 `-i`（已明确否决），当前接受这一次输出。
- `const DAEMON_PATH: &str = "/daemon/bin:/usr/bin"` 未定位：全仓、设备 `/data/service/hnp`、其它 workspace 均无此字符串；hicodeerd 现有实现是把 daemon 的 PATH 追加到调用方 PATH 之后（`hicodeerd/src/exec.rs:288-374`），不是硬编码。
