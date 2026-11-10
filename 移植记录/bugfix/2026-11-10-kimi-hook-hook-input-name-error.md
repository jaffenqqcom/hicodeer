# kimi 只上报工作状态，完成与阻塞不上报

## 问题描述

kimi 接入钩子后，运行中能在面板显示 working，但运行结束与等待回答都不上报：面板停在 working，或干脆一直 idle。

## 问题表现

- 发问后状态能到 working，但回复结束后不回 idle，`completion_seq` 不递增。
- 弹出选择题等待回答时，`blocked_seq` 不递增，IM 也收不到等待通知。
- 钩子确实被调用（在脚本末尾加临时轨迹可看到 working 与 idle 都触发了），但 daemon 的路由日志里只有 working 那条。

## 问题原因

`herdr/src/integration/assets/kimi/hicodeer-agent-state.sh` 里有一处改名没改全：

- 脚本把负载变量命名为 `payload`；
- 但 `idle` 与 `blocked` 两条路径调用的是 `describe_prompt(hook_input)`，而 `hook_input` 在该脚本里并不存在。

于是这两条在发送前就抛 `NameError`，异常被 `2>/dev/null` 吞掉，daemon 什么也收不到。working 不调用 `describe_prompt`，所以一直正常——正好对应「只有运行中能感知」。

另一个容易误判的点：还原钩子时如果用 `cp -p` 把仓库资产的 644 权限一起带过去，kimi 根本执行不了这个脚本，所有事件都静默失败，表现是永远 idle。

## 解决方案

- 变量统一回 `hook_input`（与 codex、qwen 的脚本一致），共三处：`json.load(sys.stdin)`、空负载兜底、`session_id` 读取。
- `describe_prompt(hook_input, blocked=(action == "blocked"))`：补上阻塞参数，让 PermissionRequest 一类等待使用工具调用的命令而不是上一轮助手回复作为正文。
- `HERDR_INTEGRATION_VERSION` 13 → 15（14 是 blocked 传参，15 是本次变量名修复），`herdr/src/integration/mod.rs` 的 `KIMI_INTEGRATION_VERSION` 同步。
- 安装后的钩子权限固定为 775。

## 验证

- 钩子调用轨迹显示 working、idle、blocked 三条都触发；daemon 侧 `pane.report_agent` 全部被接受，状态机按 idle → working → blocked → idle 走通。
- `sh -n` 语法检查通过；安装副本与仓库资产 md5 一致。
