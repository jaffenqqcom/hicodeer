# agent-tests/e2e

端到端脚本：直接对 `hicodeerd` 内嵌的 herdr agent API 发 JSON-RPC，校验历史会话的扫描、面板状态、恢复三条链路。

只用 python3 标准库（`socket` / `json` / `subprocess` / `argparse`），没有第三方依赖，也不编译、不装包、不重启 daemon。

**验证边界（重要）**：本机没有 gpui-mcp 集成，`mcp__gpui-inspector__get_app_state` 返回 `No running GPUI app found`，所以**面板的像素级点击不在本套脚本范围内**。脚本能验的三层是——RPC 行为、`history.json` 数据状态、源文件删除；凡是「界面上看起来对不对」的判断都不在这里，别把这些脚本当成 UI 测试。

## 前置条件

- 设备上有 `hicodeerd` 在跑（它是 HNP 服务，父进程不是应用）。
- 应用至少启动过一次，`agent.sock` 已创建。
- 要测的 agent 已在本机装好，并且真的跑过至少一次——扫描器只读它们自己的会话存储。

**设备侧的 PATH 与包装器**（不满足时 `agent.start` 会成功、但 pane 里立刻 `command not found`，看起来像恢复失败）：

- pane 继承的 PATH 必须包含 `~/npm/bin`。缺它的话 deveco / zcode / mcode（minimax）/ openclaw / opendesk 全部起不来。当前已在 `~/.zshrc` 里加过，换机器或换用户要重加。
- deveco 需要一个缓存入口 `@deveco/deveco-code/bin/.deveco`（软链到 openharmony 平台二进制）。缺它会报 `failed to install the right version … deveco-code-linux-arm64`。

检查 socket 在不在：

```bash
ls -l /data/storage/el2/base/haps/entry/files/.hicodeer/agent/agent.sock
```

socket 路径由 `agent_socket_path()` 决定（`crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs:441-449`）：`$XDG_CONFIG_HOME`（缺省 `/data/storage/el2/base/haps/entry/files`）下的 `.hicodeer/agent/agent.sock`。daemon 会把最终值导出成 `HICODEER_AGENT_SOCKET_PATH`，本套脚本优先读它，其次读这个缺省路径；每个脚本都能用 `--socket` 覆盖。

## 排障：恢复失败先看 `pane.read`

`test_resume.py` 只说“恢复没成”，原因在 pane 里。拿到失败时返回的 `pane_id` 后先读屏（`pane.read` 的参数是 `pane_id` + 必填 `source`，取值 `visible` / `recent` / `recent_unwrapped` / `detection`，见 `schema/panes.rs:347-359`），两种高频原因都不是 resume 逻辑的问题：

```bash
cd agent-tests/e2e   # 下面这条要在脚本目录里跑，才能 import agent_rpc
python3 -c "import agent_rpc,sys,json;r=agent_rpc.AgentRpc();print(json.dumps(r.call('pane.read',{'pane_id':sys.argv[1],'source':'recent','lines':60}),ensure_ascii=False))" w7:p3
```

- 屏上是 `command not found`（或 `sh: xxx: not found`）→ **PATH 问题**，看上面「设备侧的 PATH」。
- 屏上是 `failed to install the right version … deveco-code-linux-arm64` → **deveco 缺缓存入口**，看上面第二条。
- 屏上是 agent 自己的恢复报错（例如 openclaw 找不到该 query）→ 才是 CLI 层能力问题。

## 脚本

### `agent_rpc.py` — 公共库，不单独运行

- `AgentRpc(socket_path=None, timeout=15.0)`：一次调用一条连接（面板也是这么做的，`crates/agent_panel/src/client.rs:6`）。
- `call(method, params)` → 返回 `result` 对象；错误体抛 `RpcError`，`unknown_method` 抛 `EnvironmentNotMet`。
- 封装：`ping` / `sessions` / `agents` / `panes` / `workspaces` / `rename` / `start_agent` / `create_workspace` / `close_workspace` / `delete_session` / `resume`。
- `HistoryState`：面板自己的 `history.json`（pin / archive / 历史标题），没有对应 RPC，见下面「方法清单」。
- `Harness`：收集断言、打印「期望 vs 实际」、给出退出码。

三个脚本共享两个参数：`--socket`（覆盖 socket 路径）与 `--timeout`（单次 RPC 读超时，默认 15 秒；`agent.start` 会按启动超时自动放宽）。

### `test_scan.py` — 扫描校验（只读）

```bash
python3 test_scan.py
python3 test_scan.py --agent codebuddy --agent claude
python3 test_scan.py --agents codebuddy,pi --min-sessions 2
```

参数：

- `--agent SLUG` 可重复 / `--agents A,B,C`：期望有会话的 agent 名单，给定时替换默认 17 个。
- `--min-sessions N`：每个 agent 至少几条会话，默认 1。

断言：`agent_sessions` 结果类型正确；期望名单里每个 agent 都有 ≥N 条；每条会话的 `agent` / `session_id` / `source_path` 非空；`(agent, session_id)` 唯一。缺 `workspace_root` 只告警：这类行仍然出现在结果里，面板把它们收进「Unknown folder」组（`crates/agent_panel/src/session_service.rs` 的 `group_for`），恢复时省略 `cwd`。

### `test_panel_ops.py` — 面板状态校验（数据层，可回滚）

名字叫 panel_ops，但验的是面板**所依赖的数据层**：`history.json` 的写入语义 + 服务端原始行不被改动，不是点界面。

```bash
python3 test_panel_ops.py --agent codebuddy --session <id>
python3 test_panel_ops.py --agent claude --session <id> --pane w1:p2
python3 test_panel_ops.py --agent codebuddy --session <id> \
    --allow-delete --create-cmd "codebuddy -p hello"
```

参数：

- `--agent`（必填）、`--session`（省略时取该 agent 最近更新的一条）。
- `--data-dir`：应用数据根，默认从 `/data/storage/el2/base/haps/entry/files/custom_data_dir` 读（`launch_app.rs:83` 写入的那个值）。
- `--pane PANE_ID`：额外校验 `agent.rename` RPC（需要一个正在跑的 agent）。
- `--allow-delete` + `--create-cmd`：跑删除校验，二者缺一就跳过。
- `--create-timeout`：等新会话出现在扫描结果里的秒数，默认 90。
- `--keep`：不回滚，仅调试用。

步骤（每步都读回断言，且 `finally` 里回滚）：

1. 历史标题 rename（写 `history.json` 的 `title_override`）
2. pin（`pinned=true`）
3. archive（`archived=true`，且 pin 标记仍在）
4. 取消 pin 后 archive 仍保留
5. **RPC 复核**：上述写入前后各取一次 `agent.sessions`，断言该会话原始行的 `title` / `workspace_root` / `source_path` 等字段完全未变——面板状态是覆盖层，不得改动 daemon 扫出来的原始数据
6. 可选：`agent.rename` 改 pane 上的 agent 名再改回
7. 可选：`agent.session.delete` —— 先跑 `--create-cmd` 造一条新会话，只删这一条

删除的三重护栏：没有 `--allow-delete` 不删；没有 `--create-cmd` 不删；删之前同时核对「这条 session 不在本轮基线里」和「它的 `source_path` 不属于任何既有会话」，任一不满足就拒绝。

### `test_resume.py` — 恢复校验

```bash
python3 test_resume.py --all                      # 全量：名单里每个有会话的 agent 各恢复一次
python3 test_resume.py --agent codebuddy --session <id>
python3 test_resume.py --agent deveco --session <id> --require-ready
python3 test_resume.py --agent codebuddy --session <id> --cleanup
python3 test_resume.py --agent openclaw --session <id> --expect-known-failure
```

`--all` 是「能不能点开」的总闸：它遍历 `test_scan.DEFAULT_AGENTS`（17 个 agent），对每个取最新一条会话真跑一次恢复，并强制 `--require-ready`——**没到 `interactive_ready` 就是失败**，没有跳过。会打印 `恢复覆盖：N/17 个 agent`，没有会话的 agent 会列出来，避免「以为全跑了其实只跑了几个」。

跑之前它会**关闭 daemon 里所有 space**（`--keep-spaces` 可关掉这个行为）。原因：pane 和它的 agent 进程活在 hicodeerd 里，杀面板应用没用（应用只是客户端，实测杀完 daemon 里仍是 21 个 agent / 12 个 space）；而残留的 pane 会继续占着它恢复过的会话，下一轮就报 `session … is open in another window`。关掉 space 才会真正回收 pane 与进程组（实测 12 个 space 关完，agent 归 0、进程全清）。

个别 agent 需要 shell 启动文件才能生效的变量，这类由脚本**在该条检查前后临时写入并删除**，不留在用户环境里，且排在最后跑：

- `zcode` → `TMPDIR=/data/storage/el2/base/haps/entry/files/tmp`。它要在 `os.tmpdir()` 下 bind 一个 broker socket，`/storage` 是 hmdfs，`listen` 直接 `EPERM`；而 `~/.zshrc` 默认把 TMPDIR 指到 `/storage/Users/currentUser/tmp`。
- 实现：写一个 `# >>> herdr-test-env` 托管块到 `~/.zshrc`，`finally` 里删掉；`body` 外层还有一层 `finally`，异常退出也会清理。pane 是 daemon 起的 shell，只读启动文件，所以这是唯一能让它看到变量的方式——脚本自己 `export` 对 pane 无效。

两个判定上的硬约束：

- **`resume_unsupported` 一律失败**。每个被扫描的 agent 都必须在服务端有恢复命令（单测 `sessions/fad.rs::every_scanned_agent_has_a_resume_plan` 守着这条），所以服务端拒绝说明扫描集合与恢复表不一致，属缺陷，不再当环境限制。
- **argv 表缺项一律失败**。`expected_resume_argv` 返回 `None` 时不再打 WARN 跳过——那会让任何 argv 都算通过。

参数：

- `--agent`（与 `--session` 同时给；`--all` 时忽略）。
- `--isolate-workspace`：新建独立 space 而不是复用同目录的 space。`--all` 自动打开：同一 space 里的两个 agent 会抢同一个 root pane，且跑完留下的 pane 会继续占着会话（Codewhale 报 `open in another Codewhale window`、Hermes 报 `This chat is open in another Hermes window`，之后该会话再也恢复不了）。
- `--workspace-root`：省略时取该会话的 `workspace_root`。会话本身没有 `workspace_root` 时（hermes 这类连接器猜不出目录），**省略 `cwd` 字段**，由服务端按当前聚焦 space 推导（`herdr/src/app/api/workspaces.rs:95-112`）。注意 `reuse_root` 只在带 `cwd` 时生效（`workspaces.rs:48-49`），所以这条路径一定会新建 space，脚本据此与运行前的 workspace 列表比对；`workspace.create` 报错时退出码 2，并原样打印服务端错误码。
- `--start-timeout-ms`：默认 30000，必须 >3000 且 ≤300000。
- `--require-ready` / `--wait-seconds`：额外硬断言会话在 N 秒内到达 `interactive_ready`。
- `--cleanup`：结束后关闭本轮新建的 workspace（本轮之前就存在的 space 一律不关）。
- `--method-args JSON`：额外合并进 `agent.start` 参数的 JSON 对象（`{session}` 占位替换）。**不能带 `args`**——`resume_session` 与 `args` 互斥，同传会被服务端判 `invalid_agent_argument`（`herdr/src/app/agents.rs:300-310`）。
- `--require-agent-session` / `--no-require-agent-session`：是否断言该 pane 的 `agent_session`（**默认开启**）。默认会校验 `value` == 会话 id、`agent` == 目标 agent、`source` == `herdr:<label>`（`agy` 例外，是 `herdr:antigravity_cli`）。`openclaw` 自动跳过这一项（它起不来时 session 会被清掉），也可手动用 `--no-require-agent-session` 关掉。
- `--expect-known-failure`：把该 agent 的失败记为「已知失败」，退出码仍为 0。用于 `openclaw`：服务端会正确下发 `openclaw resume <id>`，但 CLI 的 `resume <query>` 只匹配 session key / display name / label（[官方文档](https://docs.openclaw.ai/cli/resume)），扫描出的 id 是 UUID，TUI 大概率 exit 1。

断言：`agent.start` 返回成功；返回的 `argv`（完整形式，含 argv[0]）等于服务端恢复命令表（见下）；`agent.list` 出现新 pane；pane 归属新建的 workspace；`agent_session` 的 `value` / `agent` / `source` 正确（默认开启）；`agent` 字段等于目标 agent。

无 cwd 的会话（如 `hermes`）走另一条分支：脚本不发 `cwd`，由服务端按当前聚焦 space 推导，并打一行 INFO 说明；不猜目录、也不断言面板最终用了哪个目录（那是 UI 侧决策，API 看不到）。

**`agent` 字段的时机**：`launch_pending` 期间该字段一定是 `null`（服务端还没识别出进程，`effective_known_agent()` 为空），所以这一条只在启动落定后判定——加 `--require-ready` 时等到 `interactive_ready` 再断言；没加且仍在 `launch_pending` 时降级为 WARN（文案「启动中，跳过 agent 字段断言」），不算失败。`pane_id` / `argv` / `agent_session` 都更早可用，收到即断言。

服务端恢复命令表（`herdr/src/agent_resume.rs:204-366` + `herdr/src/app/agents.rs:568-590`，脚本里是 `expected_resume_argv` 直接断言）：

- `claude` / `devin` / `droid` / `gemini` / `hermes` / `qodercli` / `codebuddy` / `qwen` / `grok` / `zcode` / `codewhale` / `atomcode` / `opendesk` → `<cmd> --resume <id>`
- `kimi` / `pi` / `opencode` / `kilo` / `deveco` → `<cmd> --session <id>`
- `minimax` → `mcode --session <id>`
- `cursor` → `cursor-agent --resume <id>`
- `agy` → `agy --conversation <id>`
- `letta` → `letta --conversation <id>`（id 形如 `default:<agent>` 时展开成 `--conversation default --agent <agent>`）
- `copilot` / `omp` → `<cmd> --resume=<id>`（等号形式，一个 argv）
- `codex` → `codex resume <id>`（子命令，无横线）
- `openclaw` → `openclaw resume <id>`（有 argv，但 CLI 起不来，见上）
- `deepseek` → `dsh tui --resume <id>`（launcher 先要 profile 名 `tui`，之后的参数原样交给 app）
- `mastracode` → `mastracode --thread <id>`

`opendesk` 现在也能走正常恢复：`sessions/opendesk.rs::workspace_of` 会从 task 的系统提示里读出工作目录（` - 工作目录：<path>`），所以 `workspace_root` 不再恒为 None。脚本没有为它保留任何特判，遇到个别旧会话仍缺 `workspace_root` 时按环境不满足处理（退出码 2）。

### `deveco` TUI 插件单测（bun test）

```bash
cd herdr/src/integration/assets/deveco && bun test ./hicodeer-tui-session.test.ts
```

覆盖 8 项：无会话时不发请求、选中会话后上报会话与 working 转换、子会话活动归到根会话、`streaming` 映射成 working、permission 出现时 blocked、dispose 后不再上报、投递失败会重发、缺 TUI API 时不安装并报错一次。

### `test_hooks.py` — hook 集成校验（安装 + 上报）

```bash
python3 test_hooks.py --all                  # 四个有 hook 的 agent 全跑：安装自检 + 真发一轮提示
python3 test_hooks.py --agents deveco        # 只跑一个（deveco 默认附带 blocked 校验）
python3 test_hooks.py --all --no-install     # 只校验，未安装就判失败
python3 test_hooks.py --all --no-blocked     # 跳过「等审批」校验
python3 test_hooks.py --all --keep           # 跑完不关本轮新建的 space
```

覆盖 `HOOK_AGENTS`：`deveco` / `atomcode` / `codewhale` / `zcode`。

每个 agent 的检查链：

1. `integration.list` 找到该 target，断言 `available`；除非 `--no-install`，否则 `integration.install` 后断言 `state == current`。
2. 新建独立 space，`agent.start` 起 TUI，断言返回了 `argv`。
3. 处理启动提示（见下），等 pane 的 `agent` 字段等于目标 agent。
4. `pane.send_text` + `pane.send_keys(["enter"])` 发一轮提示，轮询 `pane.get`：
   - 必须至少出现过一次 `agent_status == "working"`；
   - working 之后必须回到 `idle`（`done` 也算收尾）。
5. 断言 `agent.list` 里该 pane 的 `completion_seq` 非空——这是服务端对「这次空闲是真完成、而不是被 Esc 打断」的判定（`herdr/src/app/actions.rs` 的 `is_completion_transition` + `suppress_completion`），属 herdr 内部事实。
6. 断言 `agent_session.source == "herdr:<agent>"`、`agent_session.agent == <agent>`——即状态确实来自我们装的 hook，而不是屏幕识别兜底。

覆盖范围只到 herdr 服务端：脚本读的都是 `integration.list` / `pane.get` / `agent.list`，断言服务端记录的状态、会话与完成标记。面板侧的表现（侧栏行标记、提示音、应用内 toast，即 `agent_panel.rs` 的 `attention_agents` 与 `toast_is_silent`）**不在本脚本范围内**，也不为了它往协议里加字段。`completion_seq` 之所以要断言，是因为它是服务端对「这次空闲是真完成而不是被打断」的判定，属于 herdr 内部事实。

`BLOCKED_AGENTS`（`deveco` / `zcode`）额外跑一轮「等审批」校验，断言 `agent_status == "blocked"`，走的是 agent 自己的审批链路：

- `deveco`：把 `deveco.jsonc` 临时插一条 `"permission": {"bash": "ask"}`（文本级插入，退出时按原字节还原），再让它执行 bash 命令；插件收到 permission 事件后上报 blocked。
- `zcode`：给它一条它自己会拦下来确认的命令（`BLOCKED_PROMPTS`，默认 `rm -rf <不存在路径>`），触发 `PermissionRequest` 事件。

`codewhale` 不在 `BLOCKED_AGENTS`：它的 hook 事件集只有 `SessionStart` / `UserPromptSubmit` / `PreToolUse` / `PostToolUse` / `Stop` / `SubagentStart` / `SubagentComplete`，**没有审批类事件**，审批框期间状态只能停在 `working`。脚本在 `PROMPT_APPROVALS` 里登记了它的审批框（按 `1` 授权一次），否则那一轮会一直等到超时。`atomcode` 同理：hooks.json 里虽然有 blocked 分支，但 7 个事件都没有审批语义，实际报不出来。

参数：

- `--agents`：逗号分隔名单；`--all` 为四个全跑。两者都不给直接判失败。
- `--no-install`：不装，只校验。
- `--prompt`：默认「用 bash 执行 echo hi，然后只回复两个字：成功」——带一次工具调用，能覆盖 `working` 到 `idle` 的完整往返。
- `--wait-seconds`（默认 180）/ `--start-timeout-ms`（默认 60000）/ `--startup-prompt-seconds`（默认 30）。
- `--no-blocked` / `--blocked-prompt`：关掉或改写「等审批」那一轮。

blocked 那一轮会先确认权限注入成功：配置不存在、读不了、或者已经带 `permission` 键时直接判失败，不会在「根本没有 ask」的前提下跑完再说通过。断言除 `agent_status == "blocked"` 之外还校验 `agent_session.source == "herdr:<agent>"`，防止被其它来源的 blocked 蒙过。
- `--cwd`：workspace 目录，默认沿用 daemon 当前目录。
- `--keep`：保留本轮 space。

环境上的硬事实（按踩坑顺序）：

- **TUI 会把启动后的第一个回车吞掉**。提示文字已经进了输入框，但窗口还在排版，回车被丢弃，状态停在 `idle`。脚本的做法是：送完文字先等 `TUI_READY_DELAY_SECONDS`，再按回车；之后 `PROMPT_CONFIRM_SECONDS` 内状态没离开起始值就再按一次（最多 `PROMPT_SUBMIT_ATTEMPTS` 次）。atomcode 每次启动都会撞上这个。
- **deveco 每次 TUI 启动都问一次目录信任**（`Safety Check:` 提示，只有 session 级选项，没有持久化开关）。脚本在 `STARTUP_PROMPTS` 里登记了 `down` + `enter` 自动应答，只对 deveco 生效。不处理的话会一直停在提示上，`agent.start` 之后的断言全部拿不到数据。
- **deveco 还有第二个迟到的提示**：启动几秒后会弹「Update Available」，同样挡住输入框。所以脚本在开始输入之前会再扫一次启动提示，而不是只在启动时扫一次。
- **deveco 有两套插件、两个落点**：`deveco.jsonc` 的 `plugin` 只在 headless server 生效，`tui.json` 的 `plugin` 只在 TUI 生效；而 `plugins/` 目录下的每个文件都会被当成 server 插件加载，所以 TUI 插件放在配置目录根下（`hicodeer-tui-session.js`），不能放进 `plugins/`。
- **zcode 需要 `TMPDIR` 指到 `/data`**（见 `test_resume.py` 同款说明），脚本用同一套 `ShellEnvironment` 托管块在跑 zcode 前后临时写入并删除。这个托管块必须覆盖**两轮**（正常轮 + blocked 轮）：少了它 zcode 会在 `listen EPERM` 上直接崩掉，pane 退回 shell、`agent` 变成 `None`。

`openclaw` 不在覆盖范围：`agent --local` 下单进程直跑，两套 hook 都挂在 Gateway 生命周期上，实测都不触发（2026-10-28）。

## 退出码

- `0` 全通过
- `1` 断言失败（`Harness` 打印「期望 / 实际」，RPC 错误还打原始回包）
- `2` 环境不满足：socket 不存在 / 连不上 / 超时 / `unknown_method` / agent 不支持恢复 / 会话缺 `workspace_root`

## 方法清单（实测自代码，不是猜的）

全部方法名与参数以 `herdr/src/api/schema.rs` 的 `#[serde(rename = "...")]` 为准，服务端处理在 `herdr/src/app/api.rs:1115-1225`。

- `agent.sessions` — 参数 `{}`，返回 `{"type":"agent_sessions","sessions":[...]}`；条目字段 `agent` / `session_id` / `title?` / `workspace_root?` / `updated_at?` / `source_path` / `workspace_id?` / `deletable`（`herdr/src/api/schema/response.rs`）。`deletable` 由产出该会话的 store 决定，为 false 时面板不显示删除按钮。
- `agent.list` — 参数 `{}`，返回 `{"type":"agent_list","agents":[AgentInfo]}`（`schema/agents.rs:190-232`）。
- `workspace.create` — `{"cwd"?,"focus","label"?,"reuse_root"?,"env"?,"source_workspace_id"?}`，返回 `{"type":"workspace_created","workspace","tab","root_pane"}`（`schema/workspaces.rs:8-28`）。
- `workspace.close` — `{"workspace_id","close_group"?}`。
- `agent.start` — `{"kind","pane_id","args":[...],"name"?,"timeout_ms"?,"resume_session"?}`，返回 `{"type":"agent_started","agent","argv"}`（`schema/agents.rs:166-183`）。`resume_session` 是恢复通道，argv 由服务端 `resume_args_for` → `agent_resume::plan` 生成后回传（`agents.rs:568-590`），返回的是完整 argv（含 argv[0]）。`resume_session` 与 `args` 互斥，同传返回 `invalid_agent_argument`；没有恢复命令的 agent 返回 `resume_unsupported`（`agents.rs:355-370`）。
- `agent.rename` — `{"target","name"?}`，`name` 必须匹配 `^[a-z][a-z0-9_-]{0,31}$`，`null` 表示清除；返回 `{"type":"agent_info","agent"}`（`herdr/src/app/agents.rs:165-218`、`:423-461`）。
- `agent.session.delete` — `{"agent","source_path"}`，返回 `{"type":"agent_session_deleted"}`（`schema/common.rs`）。`agent` 与路径同来：守护进程据此把请求转给产出该会话的 store，由那个 store 自己判断路径是否在它的根内、以及一条会话由哪些文件组成。
- `pane.list` / `workspace.list` / `ping` — 参数 `{}`。

**没有的方法（查过全表，不要编）**：

- 历史会话的**改名 / 置顶 / 归档 / 恢复归档 都没有 RPC**。daemon 只负责收集；这三样是面板的本地状态，存在 `<data_dir>/agent/history.json`（`crates/agent_panel/src/session_service.rs:1-6`、`:106-124`；路径见 `crates/agent_panel/src/agent_panel.rs:395`）。所以 `test_panel_ops.py` 是对这个文件做读写断言，而不是调 RPC。
- 没有 `agent.resume`。恢复 = `workspace.create(reuse_root)` + `agent.start{resume_session}`，见 `crates/agent_panel/src/client.rs:185-262`。
- 删除也只有一条路：面板里的删除按钮最终仍走 `agent.session.delete`（`crates/agent_panel/src/agent_panel.rs:1593-1611` → `crates/agent_panel/src/client.rs:282-290`），应用的进程自己没有 unlink 分支（`EPERM`，见那段注释）。所以测试用同一个方法即可覆盖面板行为。

## 造会话命令（`--create-cmd`）

删除校验必须先造一条属于自己的会话。各 agent 的可复现命令、必需 env、配置位置与会话落盘路径记在记忆文件 `reference_agent_launch_recipes.md`（`~/.codebuddy/projects/storage-Users-currentUser-workspace-hicodeer/memory/`），这里只列要点：

- 通用前提：`PATH` 加上 `~/npm/bin` 与 `~/.harmonybrew/bin`；`TMPDIR` 指到 `/data/storage/el2/base/haps/entry/files/tmp`（`/storage` 是 hmdfs，绑 UDS 会 `EPERM`）。
- 删除能力由产出会话的 store 决定（即 `agent.sessions` 的 `deletable`），删掉的东西随存储形态不同：`.jsonl` / `.json` 会话就是 unlink 那个文件；codewhale 删文档 + 同名目录 + `session_boot_owners.json` 里的条目；atomcode 删同 stem 的全部 sidecar；opendesk 删 task 文档 + 其子任务 + 从 `task_list.json` 移除条目；deveco / zcode / minimax 走 `sqlite3` 删行（外键级联）。每个实现都先确认路径在该 store 根内，且排除该 store 自己的簿记文件（索引、boot owner 表）。
- 造会话的例子：codebuddy 用 `codebuddy -p "只回复两个字：成功" --port 62966`，会话在 `~/.codebuddy/projects/<slug>/<id>.jsonl`；claude 同理，落在 `~/.claude/projects/*.jsonl`。其它 agent 的命令见上面那份记忆文件。
- **codebuddy 必须带 `--port`**：它默认在一个固定端口上起本地 server，只要机器上已有一个 codebuddy 实例（面板里的 pane、用户自己的终端），新实例就会 `listen EADDRINUSE 127.0.0.1:62965` 直接退出 1。换一个没被占用的端口即可。

示例：

```bash
python3 test_panel_ops.py --agent codebuddy --session <id> \
  --allow-delete \
  --create-cmd 'PATH=$HOME/npm/bin:$HOME/.harmonybrew/bin:$PATH TMPDIR=/data/storage/el2/base/haps/entry/files/tmp codebuddy -p "只回复两个字：成功"'
```

## 已知限制

- **只扫鸿蒙上能装的 agent。** daemon 在 `herdr/src/sessions/fad.rs::fad_stores()` 里逐个构造 FAD connector，覆盖 claude / codex / gemini / hermes / kimi / openclaw / opencode / qwen 以及 pi 的两个（`pi_agent`、`pi_durable`），共 10 个；加上本仓自写的 codebuddy / deepseek / deveco / zcode / codewhale / atomcode / minimax / opendesk，合计 17 个 agent。名单之外的 agent（cline、continue、amp、cursor、copilot、droid、kiro、muse、openhands、grok、devin …）不会返回任何行，**这不是扫描坏了**：它们在鸿蒙上没有可装的 CLI。要加新 agent，先在 `fad_stores()` 里加一行，同时把 `herdr/Cargo.toml` 里 FAD 的 feature 集补上它需要的后端（那份 feature 列表与 `fad_stores()` 必须同步，否则编译期就把 backend 剔掉了）。
- **`agent.sessions` 在 daemon 主循环里同步执行。** 它的耗时等于整个 daemon 的卡顿时间：面板那条 RPC 读超时、agent-tui 的终端帧写不出去，都会在这段时间发生。实测 112 条会话冷启动 0.12s / 热 0.03s；如果某天这个数字回到秒级，先找哪个 store 在遍历大目录（FAD 的 aider 就曾因为回退到进程 cwd `/` 而扫了 14 秒）。

- **不验 UI。** 本机没有 gpui-mcp（inspector 报 `No running GPUI app found`），脚本覆盖的是 RPC + 数据层。面板行排序、分组折叠、按钮文案这类表现只能人工看。
- **daemon 不重启，新代码不生效。** `hicodeerd` 是 HNP 服务，不随 HAP 覆盖安装重启（内存里跑的还是旧映像）。改了 `herdr/` 或 daemon 侧代码必须重启设备（或重装 bundle）才会变。
- **`/storage` 不能 bind UDS。** 共享用户文件系统上 bind 会 `EPERM`，所以 socket 只能在 `/data/storage/el2/base/haps/entry/files/.hicodeer/agent/` 下（`hicodeerd/src/main.rs:435-449`）。
- **`history.json` 的写入不是实时的。** 面板启动时读一次（`session_service.rs:98`），之后一直用内存副本；反过来，面板下一次 pin/archive/rename 会把整份文件重写，可能覆盖本脚本的改动。所以这一步断言的是文件内容，不是界面表现；跑的时候最好别同时在面板里点这几项。
- **`agent_session` 是同步写入的。** `resume_session` 在 `agent.start` 返回前就转成 `PersistedAgentSession` 并 `set_managed_agent_launch_session`（`herdr/src/app/agents.rs:237-243`、`:337`），所以紧接着读 `agent.list` 就该看到。脚本只兜 3 秒边角，不是长轮询。**为空是真实失败信号**：settle 超时、进程退出（`terminal/state.rs:2355-2364`）或 agent 自己开了另一个会话都会清掉/覆盖它，不要当环境问题跳过。
- **恢复会改变重启行为**：被记入 `agent_session` 的 pane 在应用重启后会被自动重新恢复（与 codex 面板恢复一致）。看到「重启后自动重开 pane」属预期。
- **删除按 store 实现，不存在统一的「只认 .jsonl」。** 请求必须带 `agent`，daemon 把删除交给该 agent 的 store；store 自己校验路径在自己的根内，并清理一条会话涉及的全部文件或行。所以 SQLite 存储的 agent（deveco / zcode / minimax）也能删，面板是否显示删除按钮看 `deletable`。
- **有些会话永远没有 `workspace_root`**（`hermes` 这类：FAD 猜不出目录、连接器也没有该字段）。这不是缺陷：daemon 一直照常返回该行（`workspace_root: null`），面板把它们归进「未知目录」组，恢复时省略 `cwd` 由服务端推导目录（属面板行为，API 观测不到）。`test_scan.py` 只告警；`test_resume.py` 对这类会话走同一条「省略 cwd」路径。
- **daemon 里一个 space 都没有时，省略 cwd 也能成功，但落点不是项目目录**：`workspace_creation_source()` 返回 None 后退到 `new_terminal_cwd` 策略（Follow/Home → `$HOME`，Current → 进程 cwd，兜底 `/`，`creation.rs:103-116` → `:10-28`）。这是有效路径、不会报错，所以**没有「无 space 就失败」可断言**；脚本只把实际创建目录打出来（`workspace 创建目录: <dir>（不是 agent 最终 cwd）`，取自 `root_pane.cwd`，缺失时回落 `foreground_cwd` 再补一次 `pane.get`），要断言具体目录得先 `workspace.create` 一个已知目录。
- **`deepseek` 本轮由 `--all` 跳过**（`test_resume.py::SWEPT_AGENTS_SKIPPED`）：`dsh` 先吃 profile 名再转发 app 参数，服务端下发 `dsh tui --resume <id>`，但 dsh 0.2.1-rc.4 内置的 profile 只有 `headless` / `web` / `sdk` / `sdk-minimal` / `acp`，发行包里没有 tui bundle，npm 上也没有 `@deepseek-ai/dsh-tui`（404），pane 里只会报 `profile "tui" does not exist`。`headless` 用 `--session-id <id>` 能带历史，但它是「答一次就退出」，不构成交互恢复。跳过会以 WARN 显示并在汇总里单列，**不算通过**；官方发布 TUI bundle 后删掉那条登记即可。
- **`gemini` 的恢复依赖目录正确**：`gemini --resume <uuid>` 是支持的（`SessionSelector::findSession` 先按完整 UUID 匹配，找不到才退回索引号），但它只在**当前项目**的 `chats/` 里找。Gemini 把「目录 → slug」记在 `~/.gemini/projects.json`，而 FAD 报上来的 workspace 是存储目录 `<home>/.gemini/tmp/<slug>`（不是用户工作目录），所以 `sessions/gemini_store.rs` 用 `projects.json` 反查真实目录。实测：修正后 workspace_root 从 `~/.gemini/tmp/tmp` 变回 `/storage/Users/currentUser/tmp`，pane 里能看到历史消息。
  首次在某个目录恢复会弹 `Do you trust the files in this folder?`（Gemini 自己的安全提示），选信任后会话正常显示，不属失败。
- **`openclaw` 的恢复预期失败**：CLI 的 `resume <query>` 无法按 UUID 匹配，请求照发、按真实结果判定（`--expect-known-failure` 只是把它记成预期，不改变默认行为），且跳过 session 断言（起不来会把 session 清掉）。**`openclaw` 叠加 `--require-ready` 大概率超时**——它可能停在 picker 里永远不就绪，所以要么不带 `--require-ready`，要么带 `--expect-known-failure` 一起跑。其余 agent 没有硬编码名单：服务端回 `resume_unsupported` 才是能力边界（退出码 2）。
- **`agent.start` 可能踩 `agent_pane_busy` / `agent_pane_unavailable`。** 新 tab 的 shell 未必立刻可交互，脚本按面板的节奏重试 8×300ms（`client.rs:229-259`）。
- 恢复默认**不**关闭新开的 space（与面板行为一致），重复跑会累积 space，需要清理时加 `--cleanup`。
