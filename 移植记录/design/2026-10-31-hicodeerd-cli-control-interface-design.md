# hicodeerd 命令行控制接口：让插件与 agent 驱动当前会话

日期：2026-10-31
状态：已实现（提交 `c79eeb8`），实机验证由主人完成
范围：`hicodeerd` 入口（`crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs`）、内嵌 agent server（`herdr/src/lib.rs`）、插件运行链路

## 1. 背景

HiCodeer 的 agent 能力由 `hicodeerd` 内嵌的 agent server 提供。这个 server 只有两种对外控制面：API socket 与命令行。pane 里的 agent 与插件都是命令行这一面的使用者：

- SKILL 让 agent 用 `hicodeerd pane list`、`hicodeerd agent --help` 这类命令检查与操作当前会话。
- 插件 manifest 里直接写 `"$HERDR_BIN_PATH" pane list`，`HERDR_BIN_PATH` 由 server 注入为**当前进程的可执行文件**（`herdr/src/app/api/plugins/runtime.rs:49`，取的是 `crate::platform::launch_executable()`）。

在 HiCodeer 里，这个「当前进程」就是 `hicodeerd` 自己。而 `hicodeerd` 原本只有两个身份：daemon，以及 `agent-tui` 的 TUI 客户端。它没有「执行一条会话命令」的身份，启动参数也只服务于 daemon 自己（`--socket`、`--log`）。

如果插件或 agent 直接用上游 `herdr` 二进制，会踩两件事：

- 交付物里没有 `herdr`：包里只有 `hicodeerd`（HNP 清单 `hap/entry/src/main/module.json5:169`）。
- 另起一个实例会撞单实例：server 的 socket 是固定路径，启动时被占用就直接退出（`herdr/src/server/headless/bootstrap.rs`），而 daemon 已经持有它。

结论：**必须在 `hicodeerd` 这一个可执行文件里补上 CLI 客户端身份，把命令转发给同进程内嵌的 server，而不是启动第二个 server。**

## 2. 目标与非目标

目标：

- `hicodeerd <命令组> ...` 在 pane 内可用，行为与上游 `herdr <命令组> ...` 一致。
- 插件通过 `"$HERDR_BIN_PATH"` 调命令时，落到 daemon 已经托管的那一个会话。
- `hicodeerd` 原有的 daemon 启动参数、`agent-tui`、`--help` 行为不变。

非目标：

- 不把上游 `herdr` 二进制打进包。（`herdr/Cargo.toml` 的 `[[bin]] name = "herdr"` 保留在仓库里，但不进 HAP。）
- 不支持 `server` 命令组：daemon 的生命周期由 HNP 框架与包管理器管理，不允许从会话里启停。
- 不为「跨 uid 的另一个会话」做发现：CLI 只面向当前会话。

## 3. 设计

### 3.1 一个可执行，两副面孔

`main()` 在真正进入 daemon 之前做一次判定，顺序是固定的（`hicodeerd/src/main.rs:186` 起）：

1. `agent-tui`：TUI 客户端分支，最先处理。
2. `server`：直接拒绝，退出码 2。
3. 其余命令组与顶层选项：转发给内嵌 server。
4. `--help`：打印 daemon 自己的帮助。
5. 兜底：按 daemon 启动。

顺序不能调换。`agent-tui` 与 `server` 都在最前，是因为它们和「会话命令」共用同一个命名空间，必须优先截获；`--help` 放在转发之后，是因为 `hicodeerd --help` 应该介绍这个二进制能当 daemon 用，而 `hicodeerd pane --help` 才该介绍 pane 命令。

### 3.2 判定：白名单 + 带值选项预扫描

`is_agent_cli`（`hicodeerd/src/main.rs:135`）只认两类首 token：

- 18 个命令组（`AGENT_CLI_SUBCOMMANDS`，`main.rs:105`）：`api`、`status`、`completion`、`completions`、`config`、`channel`、`machine`、`workspace`、`worktree`、`tab`、`notification`、`agent`、`terminal`、`pane`、`plugin`、`integration`、`session`、`update`。
- 4 个顶层选项（`AGENT_CLI_OPTIONS`，`main.rs:129`）：`--skill`、`--version`、`-V`、`--default-config`。

用白名单而不是「不认识就转发」，是因为 daemon 自己也有参数（`--socket`、`--log`、`--help`）。若把所有未知参数都转发，daemon 将无法启动。

预扫描要做一件事：跳过带值的全局选项。`--session`、`--machine`、`--remote` 各带一个值，`hicodeerd --session work pane list` 里的 `work` 是会话名而不是命令组。`is_agent_cli`（`main.rs:135` 的函数体）遇到这三个 token 就把下标 `+2`，再判断首个非值 token。漏掉这一步的后果是：daemon 把这条命令当成自己的启动参数，去 bind 端口，而不是转发。

### 3.3 转发：补 argv[0] 后调用内嵌 server

`run_agent_cli`（`hicodeerd/src/main.rs:150`）：

- 在参数前补 `argv[0] = "hicodeerd"`，让帮助文本、错误前缀和 `--version` 输出都自报 `hicodeerd`。
- 调 `agent_server::run_cli_with_args(argv)`；成功退出 0，失败打印 `hicodeerd: <err>` 并退出 1。

被调用侧是新增的公开入口（`herdr/src/lib.rs:545`）：

- `run_cli()`（`lib.rs:528`）现在只负责从 `std::env::args` 取参数，然后转调 `run_cli_with_args`。
- `run_cli_with_args` 内部完整保留上游 CLI 行为：`--machine` 预判、会话配置解析、`--remote` 校验、`cli::maybe_run` 分组分发、update 分支、`--skill`、`--default-config`、未知参数拒绝。
- 参数由宿主传入而不是直接读环境，是为了让「嵌入了这个 crate 的可执行文件」能用自己的 argv 调同一套 CLI。

### 3.4 `server` 为什么被拒绝

`SERVER_SUBCOMMAND`（`main.rs:133`）在转发之前被拦截，输出：

```
hicodeerd server: refused; this process is the daemon, which manages its own lifetime
```

理由有两层：

- `server stop/reload-config` 这类命令会停掉或重配 server，而当前进程就是那个 server，等于让会话命令杀掉承载它的 daemon。
- daemon 是 HNP 拉起的，退出后由框架决定是否重启；从 pane 里停它会把所有 pane 一起带走。

### 3.5 CLI 不受 nested 检查影响

上游的 nested 检查（`should_block_nested_for_env`，`herdr/src/lib.rs:473`）在 `run_cli_with_args` 的末尾，只有走到「启动 TUI」那一步才会触发。所有 CLI 分支在它之前就返回了。这样 pane 内（`HERDR_ENV=1`、默认 `allow_nested = false`）调 `hicodeerd pane list` 不会被当成「嵌套启动」拦下——这正是接口存在的场景。

## 4. 命令面

命令组的定义在 `herdr/src/cli/spec.rs`（顶层 `subcommand(...)` 一列，约 32-49 行），分发在 `herdr/src/cli.rs:101` 的 `maybe_run`。与本接口关系最大的几组：

- `plugin`（`herdr/src/cli/spec.rs:790`）：`install` / `uninstall` / `update` / `link` / `unlink` / `enable` / `disable` / `list` / `config-dir` / `action list|invoke` / `log list|show` / `pane open|focus|close`。这是「插件能不能在 HiCodeer 里跑」的直接入口。
- `integration`：`install` / `uninstall` / `status`，负责把 hook 脚本装进各 agent 的配置目录。
- `pane` / `agent` / `workspace` / `tab` / `terminal`：会话操作面。SKILL 教 agent 用的就是这些。

`--skill` 输出编进二进制的 `SKILL.md`（`herdr/src/lib.rs:467`），`--default-config` 输出默认配置；两者都走同一条转发路径，所以 `hicodeerd --skill` 与 `hicodeerd pane --help` 的程序名一致。

## 5. 运行时链路

pane 内执行一条命令时：

```
agent / 插件脚本
  → hicodeerd <group> ...
  → is_agent_cli 命中白名单
  → run_agent_cli：argv[0]="hicodeerd"
  → agent_server::run_cli_with_args
  → cli::maybe_run 分组分发
  → API client 连 HICODEER_AGENT_SOCKET_PATH 指向的 socket
  → daemon 内嵌 agent server
```

socket 寻址分两种情况（`herdr/src/session.rs:175` 的 `active_api_socket_path`）：

- pane 内：pane 环境由 `herdr/src/integration/env.rs` 的 `apply_pane_base_env` 注入 `HICODEER_AGENT_SOCKET_PATH`，CLI 直接连到当前会话。
- 裸 shell：没有这个变量时退化为 `<config_dir>/hicodeer.sock`。daemon 的 socket 走另一条推导（`hicodeerd/src/main.rs:544` 的 `agent_socket_path`，落在 `<files>/.hicodeer/agent/agent.sock`，常量见 `main.rs:72`、`:74`、`:80`），系统 shell 也不继承 `run()` 里写入的 `HICODEER_AGENT_SOCKET_PATH`（`main.rs:381`），所以裸跑连不上。这是有意的边界：控制面只对当前会话开放。

插件链路：

```
plugin action invoke / pane 事件
  → App::start_plugin_command（herdr/src/app/api/plugins/runtime.rs:16）
  → 注入 HERDR_BIN_PATH = launch_executable()（当前 hicodeerd）
  → 插件子进程执行 "hicodeerd" pane list
  → 命中 §3.2 的转发分支
```

这条链是「同一份可执行既当 daemon 又当客户端」的直接收益：插件不需要知道 `hicodeerd` 装在哪，也不需要另找 `herdr`。

## 6. 与去商标的关系

本接口是去 herdr 商标的配套，不是独立功能：

- CLI 自报的名字是 `hicodeerd`（`main.rs:100`），帮助文本、错误前缀、`--version` 全部一致。
- 但插件协议面（`HERDR_ENV`、`HERDR_PLUGIN_*`、`HERDR_BIN_PATH`）**故意保留 herdr**。插件 manifest 是上游生态的既有契约，改了这些变量，现成插件一条命令都跑不起来——详见《去 herdr 商标》§4。
- 因此会看到一个刻意的不对称：**命令行对用户叫 hicodeerd，对插件叫 HERDR_BIN_PATH。** 前者是给人看的，后者是给插件作者写的。

## 7. 验证

实机验证由主人完成（2026-10-31），口径如下：

- `agent-tests/e2e/test_plugin.py`：造一个最小 `herdr-plugin.toml` → `plugin link` → `action list` → `action invoke` 真跑并检查日志 `exit_code=0` → `disable`/`enable` → `unlink` 后消失。
- `agent-tests/e2e/test_skill.py`：逐条执行 `SKILL.md` 里的 bash 块验证帮助与参数，确认组清单完整，`--skill` 输出与仓库文件逐字节一致。
- `agent-tests/e2e/test_hook_lifecycle.py`：`integration uninstall` 后 hook 文件消失、配置干净；`integration install` 后文件回来、版本一致、脚本里的来源为 `hicodeer:<agent>`。
- 运行方式与前置环境见 `agent-tests/e2e/README.md`；`test_plugin.py` 需要显式设置 `HICODEER_AGENT_SOCKET_PATH`。

## 8. 边界与维护规则

- **新增命令组必须同步白名单**：`herdr/src/cli/spec.rs` 里加了顶层组，`AGENT_CLI_SUBCOMMANDS`（`main.rs:105`）必须同批加。漏加的后果不是报错，而是 `hicodeerd <新组>` 被当成 daemon 启动参数——最坏情况是再起一个 daemon 去抢端口。
- **`server` 永久拒绝**：不要为了「方便」把它加进白名单。
- **`--help` 归 daemon**：不要把它并入 `AGENT_CLI_OPTIONS`，否则 `hicodeerd --help` 会变成 CLI 帮助，daemon 的启动参数就没人介绍了。
- **带值全局选项必须继续预扫描**：以后若新增带值的顶层选项（形如 `--xxx <value>`），`is_agent_cli` 的下标跳过列表要同步，否则 `hicodeerd --新选项 v pane list` 会误判。
- **程序名只有一个来源**：`CLI_PROGRAM_NAME`。帮助、错误前缀、argv[0] 都从它来，不要在各分支里另写字面量。
