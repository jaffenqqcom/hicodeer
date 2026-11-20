# herdr 单测从 262 个失败收敛到全绿，并修复继承代码上的移植偏差

## 问题描述 (Problem Description)

herdr 内联进仓库后一直没有全量跑过单测。本轮第一次执行 `herdr/agent-tests/run-unit-tests.sh`，得到 3541 passed / 262 failed。需要回答两个问题：其中哪些是设备环境导致、哪些是移植过程中改坏了继承代码。用户要求环境类问题在测试脚本里一次收口，其余按"继承测试不该失败"的原则逐条定位修复，并且修改后的测试必须仍然验证真实行为，不能靠改断言或跳过作假。

## 问题表现 (Symptoms)

- 首轮全量：3541 passed / 262 failed / 12 ignored，进程还会被 `SIGTRAP` 打断，日志在 2.5 万行处截断。
- 失败面极广：socket 相关 17 个、`client::shell` 数十个、`terminal::state` 22 个、集成 20 余个、平台 10 余个，单看日志无法判断性质。
- 同一条命令在不同轮次失败集合不同（并行时 `XDG_CONFIG_HOME` 互相踩踏），没有稳定基线。

## 问题原因 (Root Cause)

分四层，逐层收口：

1. **设备环境**：`/tmp` 只读、`/storage` 不能 bind UNIX socket、`/var/tmp` 与 `/data/local/tmp` 不存在。凡测试自行拼 `/tmp/...`、`/var/tmp/...` 的用例必然失败。另有 `HICODEER_AGENT_CONFIG_PATH` 会被 agent 面板终端继承，导致加载 live config 的 6 个用例读到包内配置。
2. **平台语义差异**：上游用 `target_os = "linux"` 表示"桌面 Linux"，而 OHOS 的 target_os 恰好也是 linux。于是 `ps -o sid=`、替换运行中的测试二进制、`/data` noexec 上跑脚本、硬链接、OpenSSH 共享控制 socket 这类桌面假设全部被带进 OHOS 构建。
3. **测试前置条件与 OHOS 默认值不匹配**：OHOS 默认隐藏侧栏、无前缀键、无 tab 栏、无 pane 滚动条（这些默认值写在 herdr 代码里，见上一份记录）。而 `client::shell`、`server::headless` 的测试驱动的是独立桌面 TUI 的完整布局。
4. **移植时改坏了继承实现**（最关键）：`0779e6b 修复 agent 退出后残留、重启复活与历史会话重复启动` 与 `4386791 补齐 Agent 历史会话` 在共享代码里加了三个"门槛"，位置都在被 20 余处复用的读取器上，导致 17 个 session 语义用例失败。

## 解决方案 (Solution)

### 一、环境收口全部放进测试脚本

`herdr/agent-tests/run-unit-tests.sh` 三处改动，下次直接生效：

- `TMPDIR` 指向 data 分区 `/data/storage/el2/base/haps/entry/files/tmp`，可用 `HICODEER_TEST_TMPDIR` 覆盖。
- `unset HICODEER_AGENT_CONFIG_PATH`：agent 面板终端会继承该变量。
- 默认 `--test-threads=1`，可用 `HICODEER_TEST_THREADS` 覆盖。全仓有 6 处各自独立的 `env_lock`，并行时会互相踩踏 `XDG_CONFIG_HOME`；串行后全量 175 秒跑完，且不再 `SIGTRAP`。

串行是修好 `env_lock` 共享之后仍然保留的兜底：实测并行仍会因进程级环境变量竞争产生假失败。

### 二、硬编码临时目录改用 `temp_dir()`

`client_transport`、`autodetect`、`socket_paths`（两处）、`session`（两处）、`unix_image_files`、`frame_output_tests`、`custom_commands`、`tab_bar_status` 全部改为 `std::env::temp_dir()`；`ghostty-vt` 的 `native_base()` 用 `#[cfg(target_env = "ohos")]` 门控走 TMPDIR，其余平台仍是 `/var/tmp`。Linux 上 `temp_dir()` 就是 `/tmp`，行为等价。

socket 目录名同时缩短（`hs-{}`、`ha{}`、`hl-{}`），因为本机 TMPDIR 已经接近 `SUN_LEN`。

### 三、桌面专属用例用 cfg 收窄并写明原因

- 运行中测试二进制替换、`ps -o sid=`：`all(target_os = "linux", not(target_env = "ohos"))`。
- pty 前台进程组探测（`detect` 4 个、`pane::terminal::migration` 1 个）：本机 pty 的 `tpgid` 恒 0。
- 硬链接相关的 3 个集成用例：沙箱拒绝 `link()`。
- OpenSSH 共享控制 socket 的 6 个传输用例：路径长度已超 `SUN_LEN`，产品代码改为在 OHOS 返回 `Unsupported`，并补一个断言该返回值的 OHOS 用例。
- `wl-copy` 脚本用例：temp 分区 noexec。
- 音频输出用例：平台没有音频路径，按要求直接删除。

### 四、测试前置条件对齐 OHOS

- `client::shell` 新增 `shell::test_config()`：显式设置 prefix、展开侧栏、tab 栏、滚动条、右键菜单归属、新 tab 命名提示，206 处构造改用它。注意它是"把平台默认值调回独立 TUI 的值"，不是放宽断言。
- `server::headless` 的测试 server 打开 `pane_scrollbars`（测试量的就是服务端自己画滚动条的行数）。
- 需要"已画过内容"的用例先提交一帧（OHOS 有"未画过内容不渲染"的门控）。
- session 身份用例补 `detected_agent`：`current_session_identity_for_persistence` 要求 pane 当前持有 agent。

### 五、继承代码偏差修复（核心）

1. **拆分读取器**：新增 `recorded_session_identity()`（只返回已记录的会话，不看 agent 是否在场），删除 `current_session_identity_for_persistence`，全部 28 处调用改用它。原函数里"只为持有 agent 的 pane 持久化"这道闸加在了共享读取器上，导致"会话有没有变""owner 冲不冲突"一律得到"没有会话"。
2. **快照不再需要闸**：`persist/snapshot.rs` 改用 `recorded_session_identity()`。"不复活已退出 agent"由退出时清记录保证，`exited_pi_session_is_not_replayed_into_a_relaunched_pi` 仍通过。
3. **名字清除推迟到确认退出**：从 `if agent_released` 移到"检测确认 agent 为 None 且 `recent_agent_process_exit` 仍在"。`agent_released` 事件照发。这样既满足"不留幽灵名字"，也不违反"一次退出观察不等于退出"（issue #3225）。
4. **退出只清同一 agent 的会话**：原实现清掉了 pane 里任何来源的会话，foreign 会话被误删。
5. **claude 的 `session_start_source` 白名单回退**：去掉 `startup`（`4386791` 为历史会话恢复放宽了这个白名单，会让 `--resume` 时的 startup 报告抢走当前会话）。
6. **`is_agent_terminal` 恢复"名字算证据"**：名字清除已经推迟到确认退出，残留名字不会再存在，因此名字可以重新作为 agent 在场的证据。同时删除自己加的、与该语义冲突的 `a_leftover_name_does_not_make_a_shell_pane_an_agent`。
7. **`reconcile_agent_name_owner` 增加 `drop_name_on_session_change`**：官方 source 换会话清别名（守住上游两条 `..._clears_the_alias`），自定义 source（`custom:pi` 这类隐藏 transcript）只补绑定、不清名字。三处调用点各自按 source 取值，不再写死 `true`。
8. **自动命名要求 hook 权威在场**：检测到 agent 只算观测，不算启动。否则另一个 agent 接管 pane 后，前一个的名字会残留（`exit_then_different_agent_relaunch_drops_previous_identity`）。
9. **`pane.rs` 的"shell 在前台不报退出"降级判据补进程存活检查**：`in_background` 会被 fg 复位成 false，此时"进程真死了"也只能得到 `Unknown`，只看状态无法区分。改为 `AgentJobTracker::process_alive()`。
10. **`composition.rs` 在导航模式与多机器场景仍绘制侧栏**：OHOS 收起侧栏后，导航和机器切换都没有可点的行。
11. **窗口标题默认值 `herdr` → `HiCodeer`**（用户可见的品牌残留），`remote_reattach_program` 把裸 `herdr` 映射为 `hicodeerd`。

### 六、品牌改名后的断言与产物同步

`window_title`、`actions` 的点击词、插件截断文案、minimax handler 查找、集成 hook 版本常量改为引用 `*_INTEGRATION_VERSION`、`pane.clear`/`pane.link.resolve` 冻结 hash、`docs/next/api/hicodeer-api.schema.json` 重新生成。

契约测试的 37 个 method 形状哈希全变，原因是 `0b1e18b 清理 herdr 品牌露出` 改了 `api/schema/*` 的 description 文本。重新生成 `tests/fixtures/endpoint-method-shapes-v1.json`，并逐个确认没有真实字段增删。

### 七、半角假名尾格用例改为断言渲染帧

`render_keeps_halfwidth_katakana_voiced_tail_empty` 通过 `TestBackend` 读尾格，但 ratatui 0.30 的 `BufferDiff` 在遇到宽字符时会跳过后续列，所以尾格的空符号根本不会下发到 backend。改为在 `draw` 闭包里读 `frame.buffer_mut()`，断言的仍是渲染器写出的真实符号。

## 修改文件 (Modified Files)

测试脚本与资源
- `herdr/agent-tests/run-unit-tests.sh` — TMPDIR、unset 继承变量、默认串行。
- `herdr/tests/fixtures/endpoint-method-shapes-v1.json` — 按品牌改名后的实际哈希重新生成。
- `herdr/tests/fixtures/plugin-smoke/hicodeer-plugin.toml` — 测试引用了不存在 fixture，补齐。

继承偏差修复（产品代码）
- `herdr/src/terminal/state.rs` — 拆分 `recorded_session_identity`、名字清除时机、退出清会话范围、claude 白名单、`is_agent_terminal`、`reconcile_agent_name_owner`。
- `herdr/src/pane.rs`、`herdr/src/pane/background_agent.rs` — 退出降级判据补 `process_alive`。
- `herdr/src/persist/snapshot.rs` — capture 改读 `recorded_session_identity`。
- `herdr/src/app/actions.rs` — 自动命名要求 hook 权威；补名按 source 决定是否清名字。
- `herdr/src/app/agents.rs` — 删除与上游语义冲突的自加用例。
- `herdr/src/client/shell/composition.rs` — 导航/多机器仍绘制侧栏。
- `herdr/src/terminal_effects.rs` — 窗口标题默认值改 `HiCodeer`。
- `herdr/src/platform/unix_common.rs` — OHOS 上共享 SSH 控制 socket 返回 `Unsupported`；`remote_reattach_program` 映射 `herdr` → `hicodeerd`。

测试侧
- `herdr/src/client/shell.rs`、`herdr/src/client/shell/tests/*.rs` — `test_config()` 及 206 处构造替换、不可用 endpoint 场景先画一帧。
- `herdr/src/server/*` — socket 临时目录、headless 测试规模、契约哈希。
- `herdr/src/{session,sound,custom_commands,tab_bar_status,frame_output_tests}.rs` — 临时目录与平台用例。
- `herdr/src/{detect,integration,platform,remote,pane/terminal}` — 平台专属用例 cfg 收窄。
- `herdr/crates/ghostty-vt/src/pane_graphics_files.rs` — OHOS 的 native source 目录走 TMPDIR。
- `herdr/src/config/*`、`herdr/src/api/server.rs`、`herdr/src/product_announcements.rs`、`herdr/src/client/tests/mod.rs` — 品牌断言、默认值断言、`env_lock` 共享。

## 验证

- `bash herdr/agent-tests/run-unit-tests.sh`：**3783 passed / 0 failed / 12 ignored**（起点 3541 / 262）。
- `cargo check -p hicodeer-agent`：通过，无新增 warning（仅既有的 libc/Atomic 弃用提示）。
- `./script/bundle-ohos`：`HAP BUILD SUCCESSFUL`。

[[ohos-debug-lessons]]
