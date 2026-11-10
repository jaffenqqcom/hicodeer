# daemon 重启后 mcode 不再自动恢复原会话

## 问题描述

hicodeerd 重启（改代码后必须重启 daemon）会让面板里原来开着的 agent 一起停掉。codebuddy 等 agent 能按会话 id 恢复，但 mcode 不会：重启后 pane 退回普通 shell，原来那段对话没了。

## 问题表现

- 重启 daemon 后，原 mcode pane 只回放历史画面，看不到 mcode 进程，也没有恢复的对话。
- 用 agent.list 看，mcode pane 的 `agent_session` 是 `null`，而同屏的 codebuddy pane 有值。
- `agent.explain` 里没有恢复计划，`persist` 快照里该 pane 的 `agent_session` 为空。

## 问题原因

恢复计划由会话 id 驱动：`herdr/src/agent_resume.rs` 的 `plan` 需要 `AgentSessionRef`，`herdr/src/persist/restore.rs` 在快照里没有会话时也没有别的来源。

mcode 的钩子表只接受 `UserPromptSubmit` 与 `Stop` 两个事件（源码里是一份两元素的白名单），所以它从不触发 `SessionStart`，也从不上报 session id。没有会话引用就没有恢复计划——这是 mcode 与其他 agent 的结构性差异，不是钩子装错了。

## 解决方案

### 会话来源

`agent_resume.rs` 新增三件事：

- `minimax_directory_sessions`：直接读 mcode 自己的会话库 `~/.minimax/v2/sqlite/runtime-state.sqlite`，按 `local_runtime_sessions.workspace_dir` 取该目录最近的会话 id。daemon 的 HOME 与 mcode 相同，已验证可读。
- `session_plan`：生成 `mcode --session <id>`，`dedupe_key` 含会话 id，保证同目录多个 pane 各取一个会话。
- `continue_plan`：会话库不可用时的兜底，生成 `mcode --continue`（目录内最近会话）。

### 记住 pane 上是哪个 agent

会话 id 不存在时，恢复方式要按 agent 选。`herdr/src/persist/snapshot.rs` 的 `PaneSnapshot` 新增 `detected_agent` 字段，记录该 pane 上实际检测到的 agent；写入时用 `effective_agent_label()` 过滤，只有仍持有 agent 的 pane 才写，避免把已退出的 agent 在下次启动时又拉起来。

`persist/restore.rs` 读取时优先 `managed_agent_kind`，为空再取 `detected_agent`；无会话时调用 `resume_plan_without_session`，先按目录分配具体会话，再退到 continue。

## 验证

- `mcode --help` 确认存在 `--session [id]` 与 `-c, --continue`。
- 单测：`agent_resume` 新增 `minimax_session_plan_names_the_conversation`、`minimax_without_a_session_continues_the_workspace_session`、`agents_that_report_a_session_never_fall_back_to_continue`；`persist::restore` 新增 `saved_agent_kind_prefers_the_managed_kind_and_falls_back_to_the_detected_one`、`pane_restore_startup_continues_an_agent_that_reported_no_session`、`pane_restore_startup_leaves_a_sessionless_agent_without_a_flag_alone`，均通过（`pane_restore_startup` 6 项全绿）。
- 实机：重启 daemon 后，两个 mcode pane 都带着原对话回来。
