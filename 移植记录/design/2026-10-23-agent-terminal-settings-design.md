# Agent 终端设置迁移设计

日期：2026-10-23
状态：已实现（编译通过，待设备验证）

## 1. 目标

把 herdr TUI 设置里仍需要的项搬进 hicodeer 的设置界面，并把 Agent 集成的安装改成可配置的自动行为。

保留并迁移：

- 声音开关（herdr `ui.sound.enabled`）
- 通知方式（herdr `ui.toast.delivery`，四档：off / herdr / terminal / system）
- Agent 集成：一个「自动安装」开关，加各集成的安装状态列表

不迁移：

- 主题：固定为 `dracula`。
- 状态指示器（`ui.status_indicators`）：Agent panel 自己画状态图标，不读该配置，TUI 里保持默认。

TUI 的 settings overlay 本期不删除，先保留。

## 2. 现状

### 2.1 TUI 设置的三段路径

TUI 改一次设置由 `save_settings_edit` 完成三件事（`herdr/src/client/shell/settings.rs:162-182`）：

1. `config::write_edit` 把值以文本 upsert 的方式写回配置文件（`herdr/src/config/write.rs:1-67`）。
2. `reload_client_config()` 在本进程内立即重读并应用。
3. 异步调用 `server.reload_config`。

服务端收到后，`apply_live_config` 末尾无条件置位 `request_client_config_reload`（`herdr/src/app/mod.rs:1053`）。主循环每帧 drain（`herdr/src/server/headless.rs:541`），发现置位就向所有已连接客户端广播 `ServerMessage::ReloadSoundConfig`（`herdr/src/server/headless.rs:1561-1567`）。客户端收到后执行 `apply_reload`（`herdr/src/client/mod.rs:2007`），其中 `shell.reload_client_config()` 重读配置文件（`herdr/src/client/config_reload.rs:16-60`）。

结论：服务端到客户端已经有一条现成的「配置变了，请重读」的推送通道，不需要键盘注入。

### 2.2 配置文件

配置路径只有一个来源：环境变量 `HICODEER_AGENT_CONFIG_PATH`（`herdr/src/config.rs:56`），由 `config_path()` 解析（`herdr/src/config/io.rs:195-200`）。

- 服务端进程：指向 HNP 包内的只读模板 `conf/agent-config.toml`（`crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs:284-291`）。
- TUI 客户端：启动时把该路径复制到应用沙箱的 `.hicodeer/agent-config.toml`，改为读副本（`main.rs:137-162`、`main.rs:485-499`）。

`sound` 与 `toast` 只被 TUI 客户端消费（`herdr/src/client/shell/config.rs:127-129`、`herdr/src/client/shell/notification_policy.rs:211`），服务端不读。主题两边都读（`herdr/src/app/theme_sync.rs:31-50`），所以主题走包内默认值。

### 2.3 集成

- 目前只能手动安装，唯一入口是 TUI 设置的 Integrations 页（`herdr/src/client/shell/settings.rs:225`、`247-285`）；启动与首次运行都不安装。
- 状态查询用 `integration.list`（`herdr/src/api/schema.rs:225`），返回 label、可用性与三态（未安装 / 已安装 / 需更新）。
- 安装用 `integration.install`（`herdr/src/app/api.rs:1220`）。是否可装由该 agent 命令是否存在决定（`herdr/src/integration/registry.rs:99-108`）。
- 安装落点基于进程的 `HOME`（`herdr/src/integration/env.rs:214`）。守护进程由系统终端启动，继承其环境，`HOME` 固定为 `/storage/Users/currentUser`，与 agent 进程一致。

## 3. 设计决定

### 3.1 一份用户配置文件

用户可写的那份配置文件就是 TUI 客户端在用的 `.hicodeer/agent-config.toml`。设置界面写的也是它，因此不存在第二份状态：

- 在 TUI 改，设置页下次读取即看到新值。
- 在设置页改，TUI 经服务端广播重读后生效。

服务端继续读包内只读模板，不做改动。理由：sound / toast 服务端不消费；主题已固定在模板里；集成由应用侧调用 API 触发。

### 3.2 写方

设置页和应用侧写文件都在应用帐号下，TUI 客户端也在应用帐号（`main.rs:416-420` 注释），所以文件由应用帐号创建、属主一致，双方都可写。`.hicodeer` 目录由守护进程创建且权限为 0777（实测），应用侧可以直接在其中建文件。

目录不存在时（例如卸载重装后守护进程尚未启动），应用侧创建为 0777，与守护进程建出来的形态一致；否则守护进程无法在该目录下绑定自己的 socket。

写入用「临时文件 + rename」原子替换，缩小与 TUI 并发写的窗口。

### 3.3 生效链路

设置页保存后的顺序：

1. 以文本 upsert 方式写 `.hicodeer/agent-config.toml`。
2. 若 agent 端点可用，经 40240 桥调用 `server.reload_config`。
3. 服务端广播 `ReloadSoundConfig`，所有 TUI 客户端重读。
4. 集成安装在同一步直接调用 `integration.install`，完成后重拉 `integration.list` 刷新状态。

40240 桥是逐字节转发、不做协议翻译（`crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/agent_control.rs:1-12`），三个 method 均可直接使用，协议零改动。端点与 token 已由 `AgentControl` 下发（`crates/gpui_ohos/depend/cmd-agent/cmd-client/src/protocol.rs:93-105`）。

### 3.4 两种情况

server 未启动（或 agent 端点不可用）：

- 仍然写文件，设置保存成功，TUI 下次启动读到。
- 不调用任何 API，不报错。
- 自动安装跳过；等 Agent panel 首次连上服务端时补做一次。

server 已启动：

- 写文件后调用 `server.reload_config`，TUI 立即重读。
- 自动安装立即执行，状态列表立即刷新。

## 4. 改动清单

### 4.1 默认主题

`crates/gpui_ohos/depend/cmd-agent/hicodeerd/conf/agent-config.toml` 增加：

```toml
[theme]
name = "dracula"
```

打包脚本按原样拷贝到 HNP（`script/bundle-ohos:587-589`），不改脚本。`auto_switch` 默认为 false（`herdr/src/app/state.rs:1103`），不会被系统深浅色覆盖。

### 4.2 配置副本复制判据

`crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs` 的 `writable_agent_config`（`main.rs:485-499`）改为：本地文件已存在就直接复用，不再按模板指纹覆盖。

原因：模板指纹变化时整体覆盖会把用户在设置页或 TUI 里改过的值一起冲掉。改成「不存在才复制」后，用户配置优先。

### 4.3 自动安装开关

`crates/settings_content/src/settings_content.rs` 的 `AgentPanelSettingsContent`（`settings_content.rs:751-769`）增加：

```rust
pub auto_install_integrations: Option<bool>,
```

默认 true，由 `crates/agent_panel` 的 `AgentPanelSettings` 读取（`crates/agent_panel/src/agent_panel.rs:129-152`）。

### 4.4 agent_panel

新增模块 `crates/agent_panel/src/herdr_settings.rs`：

- 读取 `.hicodeer/agent-config.toml` 的声音与通知方式；缺省时用 herdr 的默认值。
- 逐行读取，不用 TOML 解析器。原因见 5.1：`toml` crate 在当前特性组合下解析「父表在前、子表在后」的文件会失败，而这份文件正是这种形状，解析失败会让页面永远显示默认值。
- 文本 upsert 写回这两项，保留其他键与注释；写临时文件后 rename。
- 暴露一个快照给设置页：声音、通知方式、集成条目与状态、自动安装开关的当前值。

`crates/agent_panel/src/client.rs` 增加三个调用：

- `reload_config()`：`server.reload_config`。
- `integration_list()`：`integration.list`。
- `integration_install(target)`：`integration.install`，单独放宽超时（现有 `READ_TIMEOUT` 为 3 秒，`client.rs:96-130`），结果以随后的一次 list 为准。

路径来源：优先 `SANDBOX_FILES_DIR`（本进程自己的沙箱根，必然可写），退回 `AgentControl.socket_path` 的父目录的父目录。两者指向同一目录，因为终端客户端就是这样推导它自己的副本的（`main.rs:143-146`）。

自动安装触发点：Agent panel 首次成功拉取服务端数据时检查一次；之后每 60 秒复查一次；只在端点可用且开关为真时执行；只装「可用且需要安装」的项；失败记录后不循环重试。

### 4.5 settings_ui

页面不是二级子页，而是一级页面：`SettingsPageItem` 新增 `Custom` 变体，页面项可以直接渲染任意内容，因此打开设置就能看到控件，不需要再点一次「Configure」。页面位置在 Terminal 之后（`crates/settings_ui/src/page_data.rs`），因为它配置的是那个面板里的终端。

页面内容（一级页面只占一项，块内自带分隔线，与其它页面的行距一致）：

- Sound Alerts：开关。标题与描述取自终端自己的设置页，大小写按编辑器其它设置页的规范（标题 Title Case、描述句首大写并带句号）。
- Notification Popups：`Off / Inside herdr / Via terminal / Via system`，措辞与终端的选择项相同。
- agent integrations：标题行右侧是 `check now`；下面是 agent 按钮网格。
- agent 按钮：与 base keymap 选择器同一种画法——`ToggleButtonGroup::single_row` 配 `ToggleButtonWithIcon`，`Outlined` + `Medium` + 整宽均分，同一行的按钮**连体**（相邻边框合并，只有整行外圈圆角），行与行之间留间距。每行三列，最后一行按实际数量成组，不留空槽。
- 已安装的按钮在名字后面加 ` ✓`。连体分组内的按钮挂不上角标装饰，所以用文本符号；同时按钮本身也处于选中态。
- 所有 target 都显示，包括工具未安装、集成未安装的；差别只在是否带勾。工具不在的 target 点击无动作。

数据与动作全部调 `agent_panel` 的公开函数，页面本身不解析文件、不发请求。下拉复用 `crates/settings_ui/src/components/dropdown.rs`。

`crates/settings_ui/Cargo.toml` 增加 OHOS 专属依赖 `agent_panel`。

## 5. 边界与风险

### 5.1 已知并已修的问题

- 通知方式改了不生效：页面用 `toml::Value` 解析配置文件时，遇到「`[ui]` 段在前、`[ui.toast]` 段在后」的文件会解析失败（`toml` 0.9.8 在当前 feature 组合下），于是每次都读回默认值 `off`。改为逐行读取后修复。

- 设置页与 TUI 同时写同一文件：写入窗口已用 rename 压到最小，但仍可能互相覆盖最后一次写入；两者都是用户主动操作，可接受。
- 包内模板更新后不再覆盖已有用户副本：这是 4.2 的预期行为。但设置页可能在 daemon 首次启动前就建出该文件，那时文件里只有本页拥有的两个键；因此 daemon 在文件已存在时会补齐包内模板中该文件缺少的整张表（只追加缺失表，不覆盖任何已有行），见 `hicodeerd/src/main.rs` 的 `add_missing_config_tables`。
- 声音与通知方式由页面读取，值缓存 1 秒（`SETTINGS_CACHE_TTL`），避免每次重绘都读盘、也避免读失败按帧刷日志；本页写入后立即失效缓存，TUI 里的修改最多 1 秒后可见。集成列表来自缓存，由自动复查（60 秒）或页面上的「立即检查」刷新。
- 集成安装失败不会影响设置保存；错误只在日志与状态列表里体现。

### 5.2 复审后的修正

- hermes 的启用/停用改成按插件名参数化，停用旧插件名时不会再误删新条目（`config_edit.rs` 的 `update_hermes_named_plugin`）。
- pi / omp / kilo / opencode 除了清 hooks 配置，还会删除重命名前的插件文件与旧插件条目（opencode 含 `./herdr-tui-session.js`、`./herdr-opencode`），否则升级用户会同时跑两个上报者。
- `SettingsPageItem::Custom` 与 `CustomItem` 用 `#[cfg(target_env = "ohos")]` 门控；图标枚举不门控：`icons` 的 `test_no_dangling_icons` 会把 `assets/icons/*.svg` 反解成 `IconName`，门控变体会让非 OHOS 平台的文件名解析失败，属「改变其它平台的编译/测试」，故保持纯增量数据。
- 新增的 deepseek / zcode / minimax / trae 只做识别层（枚举、可执行名、sound 覆盖项、图标）；`IntegrationTarget` 是冻结的客户端协议枚举，这四个没有安装资产，因此不会出现在集成网格里，这是本批的边界而非遗漏。
- 集成网格里的产品名不再只做首字母大写（`CodeBuddy` / `OpenCode` / `Antigravity CLI` / `Qoder` / `OMP`），映射在 `agent_panel::agent_display::integration_name`。

## 6. 验证

- 编译：`script/bundle-ohos`。
- 启动后确认默认主题为 dracula。
- 在设置页改声音与通知方式，确认 TUI（已打开时）立即重读、重开 TUI 后仍保持。
- 在 TUI 改这两项，重开设置页确认显示一致。
- 关闭自动安装，确认服务端重启后不再自动装；打开后确认缺失项被装上且按钮出现对勾。
- 删除某个 agent 已安装的 hook 文件，确认下一次自动检查把它装回来。
