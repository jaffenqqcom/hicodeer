# mcode / openclaw / opendesk 工作状态一直显示空闲

## 问题描述

三个 agent 在 HiCodeer 的 agent 面板里运行时一直显示空闲，既没有「工作中」也没有完成通知与阻塞通知。

## 问题表现

- mcode：发问后回复正常输出，但 `agent_status` 始终是 `idle`，`state_change_seq` 不动。
- openclaw：同样一直 idle；另外它本身在使用前就是坏的（见下）。
- opendesk：同样一直 idle。
- 三者在 `agent.explain` 里都落到 idle 兜底，`matched_rule` 为空。

## 问题原因

### mcode：运行中提示词的写法变了

`herdr/src/detect/manifests/minimax.toml` 的「运行中」规则只认 `esc to interrupt`，当前版本 TUI 打的是 `Esc stop`，规则从不匹配，只能落 idle 兜底。

### openclaw / opendesk：完全没有识屏规则

两者都不在 `SCREEN_MANIFEST_AGENTS` 里，`herdr/src/detect/manifest.rs` 的内置清单表也没有它们，因此任何屏幕都只能走 idle 兜底。

### openclaw 自身配置不可用

三个独立问题叠在一起：

1. 插件注册写到了配置根级的 `load` / `entries`，OpenClaw 拒绝未知根键，整份配置不可用。
2. 插件清单 `openclaw.plugin.json` 缺少 `configSchema`，OpenClaw 拒绝加载该插件。
3. 环境里的 `GEMINI_API_KEY` 把一个 google provider 带进模型列表，每次请求先打 `generativelanguage.googleapis.com` 并超时。

## 解决方案

### 识屏规则

- `herdr/src/detect/manifests/minimax.toml`：运行中规则的正则改为 `(esc\s+to\s+interrupt|esc\s+stop)`，版本号升到 `2026.11.10.1`。
- 新增 `herdr/src/detect/manifests/openclaw.toml`：按底部状态行判定。运行中为「盲文 spinner + 动作词 + 秒数 + `| local ready`」，空闲为 `local ready | idle`。
- 新增 `herdr/src/detect/manifests/opendesk.toml`：按状态栏判定。运行中为「盲文 spinner + `mm:ss` + `│ Tokens:`」，空闲为「`⏱` + `mm:ss` + `│ Tokens:`」。
- `herdr/src/detect/manifest.rs` 注册两份新清单；`herdr/src/detect/mod.rs` 把 `OpenClaw`、`OpenDesk` 加进 `SCREEN_MANIFEST_AGENTS`（25 → 27）。

### openclaw 配置

- 插件注册改到 `plugins` 下；对旧构建写在根级的 `load` / `entries`，只在确实指向本插件时才删除（避免误删用户自己的配置）。
- 插件清单补 `configSchema`（空对象、`additionalProperties: false`）。
- 用户配置里设 `models.mode = "replace"`，只保留自配的 deepseek，屏蔽 `GEMINI_API_KEY` 带进来的 google provider。
- `OPENCLAW_INTEGRATION_VERSION` 1 → 2，让面板重装新清单。

## 验证

- `bash herdr/agent-tests/run-unit-tests.sh 'manifest::tests'`：32 项通过，含三个 agent 的 working/idle 夹具。
- 真实会话实测：mcode `idle → working → done`（state 2→3→4）、openclaw（18→19→20）、opendesk（24→25→26），三者都拿到 `completion_seq`。
- `openclaw config validate` 通过；`openclaw models list` 只剩 deepseek。
