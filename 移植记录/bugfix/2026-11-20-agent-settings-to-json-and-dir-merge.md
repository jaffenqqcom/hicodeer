# Agent panel 的 sound/toast 配置迁至 settings.json + agent 运行时目录三合一

## 问题描述 (Problem Description)

agent panel 设置页的 Sound Alerts / Notification Popups 两个开关的配置存在一个"第二份"：它们写进 herdr 的 `agent-config.toml`（由 `herdr_settings.rs` 读写），而同页其它开关（Prevent Idle Sleep / Auto Install / Show History）都已存应用自己的 `settings.json`。用户要求把这两个开关也统一到 settings.json 只存一份，hicodeer 界面按 settings.json 自己决定响/弹，且不同步给 herdr。同时设备上 hicodeerd 用了三个目录（`hicodeer/agenttui/`、`.agentpanel/`、`.hicodeer/`）放 agent 运行时数据，要求按 herdr 原生单根布局合并成一个。

## 问题表现 (Symptoms)

- 设置页 sound/toast 开关的值写到 `agent-config.toml`，与 settings.json 分离；排查"设置没保存"时要在两处找。
- 该文件由应用进程（uid 20020220）与 herdr daemon（uid 20020117）共享，迁移/清理困难。
- 三个 agent 目录并存：`hicodeer/agenttui/`（herdr 原生 state 根，存 session）、`.agentpanel/agent/`（daemon socket）、`.hicodeer/`（旧 config + 过期 client sock）。
- 实测 `agent-config.toml` 里**没有** `[ui.sound]` / `[ui.toast]` 段，即当前生效值就是 herdr 代码默认（`sound.enabled=true`、`toast.delivery=Off`）。

## 问题原因 (Root Cause)

- 历史设计：sound/toast 存 herdr 的 agent-config.toml，是因为该文件是应用与 herdr 双方都能访问的共享位置。当这两个值改为纯 editor 侧消费（`notify_agent_transitions` 自己比对 agent 状态、自己播放/弹窗，不依赖 herdr 事件）后，这份共享存储就成了多余的"第二份"。
- 跨 uid 隔离：settings.json 在应用沙箱，herdr 读不到——所以"存 settings.json 且不同步 herdr"要求 herdr 端彻底不消费这两个值，而不是改存储路径。
- 目录分裂：`.agentpanel` 是 OHOS 适配时为 daemon socket 另起的目录（`main.rs` 的 `AGENT_STATE_DIR`），`.hicodeer` 是历史遗留；两者都不是 herdr 原生的单根布局（`config_dir()` 下放 config、session、socket）。

## 解决方案 (Solution)

**核心思路**：sound/toast 变成 settings.json 的普通设置项，走 `AgentPanelSettings`；herdr 完全不感知（默认保持当前值）；socket 目录回归 herdr 原生根 `hicodeer/agenttui`。

具体改动：

- `settings_content.rs`：`AgentPanelSettingsContent` 加 `sound_enabled`、`notifications_enabled` 两个 `Option<bool>`（默认 true）。
- `agent_panel.rs`：`AgentPanelSettings` 加对应两个 bool；`from_settings` 用 `unwrap_or(true)`；`notify_agent_transitions` 改从全局设置读这两个值；删掉 `migrate_legacy_delivery` 调用和 `TerminalSettingsCache` 注册。
- `agent_container_settings.rs`：sound/toast 开关的 `on_click` 改用 `update_settings_file(SettingsUiFile::User, ...)` 写 settings.json，与同页其它三个开关一致。
- `herdr_settings.rs`：删除 sound/toast 读写配置文件与 reload 的全部逻辑（config_path/current_config_path/TerminalSettings*/set_*/notifications_enabled/sound_enabled/migrate_legacy_delivery/load/parse/upsert/write/cached_settings 及测试），只保留集成列表相关；`snapshot()` 改从 `AgentPanelSettings` 读两个开关；`ToastDelivery` 精简为 Off/System 两态。
- `hicodeerd/src/main.rs`：`AGENT_STATE_DIR` 从 `.agentpanel` 改成 `hicodeer/agenttui`，socket 与可写 config 副本自动落到同一个原生根。

**herdr 代码零改动**：设备实测当前 agent-config.toml 无这两个段，代码默认（sound=true、toast=Off）就是当前值，符合要求。

**目录合并**：设备上删除旧的 `.agentpanel/`、`.hicodeer/` 目录；新 socket 落在 `hicodeer/agenttui/agent/agent.sock`。注意 hicodeerd 不随 HAP 覆盖安装重启，改 daemon 需重启设备才生效。

**已知行为变化（主人接受）**：老用户在旧 agent-config.toml 里关掉的 sound/toast 偏好，升级后不会迁移，settings.json 无值时默认重新开启（"残留不用管"）。

## 修改文件 (Modified Files)

- `crates/settings_content/src/settings_content.rs` — `AgentPanelSettingsContent` 新增两个 `Option<bool>` 字段。
- `crates/agent_panel/src/agent_panel.rs` — `AgentPanelSettings` 新增两个 bool 字段；from_settings 默认值；notify_agent_transitions 改读全局设置；删 migrate/cache 注册。
- `crates/agent_panel/src/herdr_settings.rs` — 清理 sound/toast 文件读写与 reload 逻辑，snapshot 改读 settings，ToastDelivery 精简两态，仅保留集成列表。
- `crates/settings_ui/src/agent_container_settings.rs` — sound/toast 开关改存 settings.json。
- `crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs` — `AGENT_STATE_DIR` 改为 `hicodeer/agenttui`，三目录合一。

[[ohos-debug-lessons]]
