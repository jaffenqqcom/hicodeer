# 去 herdr 商标：把上游品牌从交付物里剥离

日期：2026-10-29 首版，2026-10-31 重做后重写本文
状态：已实现（提交 `c79eeb8`），实机验证由主人完成
范围：`herdr/`（内嵌的 agent server）、`crates/agent_panel/`、`agent-tests/e2e/`

本文最初随 `ebb0f35` 写于 2026-10-29，2026-10-30 那次改动把品牌替换连同本文一起回退了（`e5b9a29` 删除了 `herdr/src/brand.rs` 与本文）。2026-10-31 重新实现后，本文按当前代码重写：**做法变了（不再有 brand 模块），边界也变了（插件协议面必须保留 herdr 标识）**，旧文中「brand.rs 是唯一来源」「上游域名全部改为环境变量」两条已不成立。

## 1. 目标与边界

HiCodeer 的交付物里不得出现上游产品名 `herdr`（与「去 Zed 化」并列的两条品牌线）。但品牌词在上游代码里同时承担三种角色，只有第一种要清掉：

- **对外面**：用户能看见、能读到、能落到盘上的字符串 —— 必须换成 HiCodeer。
- **协议面**：插件与 agent 按它写死的契约（环境变量、清单文件名、配置键）—— **必须保留**，改名会让现成插件与 hook 全部失配。
- **标识符**：Rust 模块名、类型名、变量名、目录名 —— 不进用户配置、不进协议、不上界面，按约定保留。

版本尚未发布，因此不做老机器兼容：不写「把旧 herdr 目录/配置迁移过来」的代码，新装机器从第一次启动起就只有新品牌。

## 2. 品牌的现值

- 状态与配置目录：OHOS 上为 `hicodeer/agenttui`（`herdr/src/config/io.rs:22` 的 `app_dir_name`，用 `#[cfg(target_env = "ohos")]` 门控）。非 OHOS 平台仍返回 `herdr` / `herdr-dev`——上游的调试版/发行版隔离原样保留。
- API socket：`hicodeer.sock`（`herdr/src/session.rs:163` 的 `data_dir_for`，`:175` 的 `active_api_socket_path`）；客户端 socket 为 `hicodeer-client.sock`；daemon 侧固定为 `agent.sock`。
- 日志文件：`hicodeer.log`、`hicodeer-server.log`、`hicodeer-client.log`（`herdr/src/logging.rs:41`、`herdr/src/cli/server.rs:232`）。
- 临时文件：`.hicodeer-config-<pid>-<序号>.tmp`（`herdr/src/integration/config_file.rs:107`）、`hicodeer-native-sources-*`、`hicodeer-pane-graphics-*`。
- hook 与集成资产文件名：`hicodeer-agent-state.sh / .ps1 / .ts / .js`（`herdr/src/integration/mod.rs:64`、`:74`、`:77`、`:79`）。
- 环境变量：`HICODEER_AGENT_SOCKET_PATH`（`herdr/src/api/mod.rs:20`）、`HICODEER_AGENT_CONFIG_PATH`（`herdr/src/config.rs:56`）、`HICODEER_AGENT_SOCKET_MODE`（`herdr/src/server/socket_paths.rs:86`）、`HICODEER_AGENT_INTEGRATION_ID/VERSION`（写进 hook 脚本）。
- 命令名：`hicodeerd`（`crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs:100` 的 `CLI_PROGRAM_NAME`）；TUI 客户端进程名 `hicodeerd-tui`，便于 `ps` 区分。
- 会话来源：`hicodeer:<agent>`（`herdr/src/integration/assets/claude/hicodeer-agent-state.sh:32`，其余集成资产同形）。
- 界面文案：窗口标题、onboarding、右键菜单、toast、错误前缀统一为 HiCodeer。

## 3. 四类面的落地方式

### 3.1 落盘产物

目录、socket、日志、临时文件、hook 文件名全部改为 `hicodeer*`，旧路径不再创建也不清理。判据是「新装的设备上 `find` 不到 herdr 命名的产物」。

### 3.2 二进制内的字符串

- 环境变量读写两侧同改：`HICODEER_AGENT_*` 既在服务端注入（`herdr/src/integration/env.rs` 的 `apply_pane_base_env`），也在 hook 脚本与集成资产里读取，改一处不改另一处会表现为「状态永远不上报」。
- 进程名与帮助文本走 `hicodeerd`；诊断前缀同理。
- 包里只交付 `hicodeerd`（HNP 清单为 `hap/entry/src/main/module.json5:169` 的 `hicodeerd.hnp`）。`herdr/Cargo.toml:24`、`:28` 的 `[lib] name = "herdr"` 与 `[[bin]] name = "herdr"` 仍未改，但该 bin 不进交付物；`hicodeerd` 通过依赖重命名引用它（`crates/gpui_ohos/depend/cmd-agent/hicodeerd/Cargo.toml:22` 的 `package = "hicodeer-agent"`）。

### 3.3 日志

tracing 的 target 默认是模块路径，会带上游 crate 名。这里不去改 crate 名，而是在格式化时 **关闭 target 输出**（`herdr/src/logging.rs:34` 的 `with_target(false)`），每行改为携带 `subsystem` 字段表示来源。过滤不受影响：`EnvFilter` 读的是事件元数据。

代价是过滤环境变量仍叫 `HERDR_LOG`，默认过滤器仍是 `herdr=info`（`herdr/src/logging.rs:23`）。它们只在调试时使用，不进用户配置。

### 3.4 用户可见文案

- `herdr/skills/herdr/SKILL.md` 全篇改为 HiCodeer / `hicodeerd`，作为 `--skill` 的输出（`herdr/src/lib.rs:467` 用 `include_str!` 编进二进制）。
- API schema 的 `title` 改为 `HiCodeer API`（`herdr/docs/next/api/herdr-api.schema.json`），文件名保留。
- 集成资产（`herdr/src/integration/assets/` 下 24 项：23 个 agent 目录，覆盖 claude、codex、codebuddy、deveco、zcode、kimi、qwen、gemini 系等，外加一个测试脚本）里的提示语、状态文本、hook 注释同步改写；脚本本身的行为不变。

## 4. 有意保留 herdr 的面（本版最重要的取舍）

这些不是漏改，是**协议兼容面**：

- `HERDR_ENV=1`：SKILL 的第一句就是 `test "${HERDR_ENV:-}" = 1`，agent 靠它判断自己是否运行在 HiCodeer 的 pane 内。pane 环境由 `herdr/src/integration/env.rs` 注入，插件运行时也由 `herdr/src/app/api/plugins/runtime.rs:44` 注入。改这个名字会同时打断 SKILL 与所有插件的入口判断。
- `HERDR_PLUGIN_*`、`HERDR_WORKSPACE_ID`、`HERDR_TAB_ID`、`HERDR_PANE_ID`、`HERDR_BIN_PATH`：插件 manifest 里写的命令直接引用这些变量（示例见 `herdr/src/app/api/plugins/mod.rs` 的测试 manifest），这是插件作者眼里的稳定契约。
- `herdr-plugin.toml` 与 `min_herdr_version`：插件清单文件名与版本字段（`herdr/src/app/api/plugins/manifest.rs:124`）。改了之后所有现成 herdr 插件都装不进来，与「让插件在 HiCodeer 里运行」的目标直接冲突。
- `[ui.toast.herdr]` 与 `delivery = "herdr"`：用户已有配置文件里的取值。改名要写迁移代码，而本版按「不改协议值」处理。
- `herdr.dev`：更新清单与远程分发地址（`herdr/src/update.rs:25`、`herdr/src/remote/attach.rs:33`）。只在 update / version-check 路径使用，不落盘、不上界面。
- 目录与标识符：`herdr/` crate 目录、`herdr/skills/herdr/`、`herdr-api.schema.json` 文件名、`crates/agent_panel/src/herdr_settings.rs` 文件名、Rust 类型与函数名。判据是它们不出现在用户配置与界面里。

一句话：**品牌只从「用户看得见」的地方剥离，插件生态的入口一个都不动。**

## 5. 与首版方案的差异

- 首版新增 `herdr/src/brand.rs`，用 `NAME / DIR / FILE_PREFIX / COMMAND / SESSION_KEY_PREFIX` 五个常量派生所有名字。本版没有这个模块，品牌值直接写在使用点，代价是替换点分散、容易漏（见 §6）。
- 首版把更新与分发域名改为显式环境变量（`HICODEER_AGENT_UPDATE_MANIFEST_URL` 等），本版未恢复这一改动，仍编译进 `herdr.dev` 地址。
- 首版删除了 `LEGACY_*` 一类「清理上一代 herdr 安装」的代码，本版部分保留（例如 `herdr/src/integration/file_ops.rs:31` 仍识别 `HERDR_INTEGRATION_ID=` 与 `HICODEER_AGENT_INTEGRATION_ID=` 两种写法），因为 hook 安装仍需处理设备上已经存在的旧脚本。

## 6. 残留清单（如实记录）

下面这些地方在二进制或界面里仍会出现 `herdr`，按当前口径属于「待处理」而不是「有意保留」：

- `hicodeerd --default-config` 的输出仍写 `# Place this file at ~/.config/herdr/config.toml`（`herdr/src/lib.rs:85`），实际路径已经是 `hicodeer/agenttui`，文案是错的。
- 同一份默认配置里还有 `directory = "~/.herdr/worktrees"`（`herdr/src/lib.rs:252`）、`[ui.toast.herdr]`（`:394`）、`Check herdr.dev for ...`（`:146`、`:149`）。
- `hicodeerd <组> --help` 的尾部仍打印 `Env: HERDR_CONFIG_PATH overrides config file path` 与 `Home: https://herdr.dev`（`herdr/src/lib.rs:742`、`:743`）。
- `herdr/src/lib.rs:1` 的 crate 文档注释仍是 `//! herdr — terminal workspace manager ...`。
- `crates/agent_panel/src/herdr_settings.rs:45` 的 `CLIENT_CONFIG_DIR` 仍是 `"herdr"`，`:457` 的 `client_config_dir` 因此指向 `<sandbox>/herdr`；而 OHOS 上 server 的 `config_dir` 是 `<sandbox>/hicodeer/agenttui`（`herdr/src/config/io.rs:22`），应用侧没有设置 `XDG_CONFIG_HOME`。**判据**：面板的「关闭首次引导」写的是 `<sandbox>/herdr/config.toml`，内嵌 server 读的是 `<sandbox>/hicodeer/agenttui/config.toml`，两者不是同一文件——这条需要实际复核 onboarding 是否因此反复出现。
- `herdr/distribution/latest.json`（993 处）与 `install.ps1` 是上游发布资产。`latest.json` 只在测试函数里被 `include_str!` 读取（`herdr/src/update.rs:3924` 的 `checked_in_distribution_manifest_matches_update_schema`），不进产品二进制；`install.ps1` 是 Windows 安装脚本，OHOS 不打包。
- `herdr/Cargo.toml` 的 `[lib]`/`[[bin]]` 名、`herdr/` 目录名、`skills/herdr` 目录名按 §4 保留。

## 7. 验证口径

以下由主人在设备上实测，本文只记口径，不重复执行：

- hook 上报：安装后 hook 文件的 `HICODEER_AGENT_INTEGRATION_ID/VERSION` 齐全，agent 状态能上报到面板。
- 会话恢复：`agent_session.source` 为 `hicodeer:<agent>`，恢复后能接回原会话。
- 插件：`plugin link` → `plugin action invoke` 能真跑，日志里 `exit_code=0`。
- SKILL：`hicodeerd --skill` 输出与仓库内 `SKILL.md` 逐字节一致。
- 构建：`./script/bundle-ohos` 成功。注意 HAP 覆盖安装不会重启 HNP 里的 `hicodeerd`，设备上要跑到新版 daemon 必须重启设备或卸载重装 bundle。

## 8. 踩过的坑

1. **脚本化全量替换会误伤标识符**：`remote_herdr`、`herdr.executable`、函数名与测试名都会被改。正确做法是只替换字符串字面量与注释，改完用「把新名还原成旧名后与基线逐字节比较」验收，而不是肉眼比对。
2. **值域枚举不能跟着改**：`--right-click` 只接受 `in_app`，批量替换若把它写成品牌名，解析器直接拒绝。
3. **读写两侧必须同批改**：pane 环境变量的写侧在 `integration/env.rs`，读侧在 hook 脚本与集成资产里，漏一处就是「状态静默」而不是报错。
4. **进程名与二进制名分离**：`hicodeerd` 既是 daemon 又是 CLI 客户端，TUI 客户端必须单独改名成 `hicodeerd-tui`，否则 `ps` 里两条一模一样的记录无法区分。
5. **品牌回退会把设计文档一起带走**：首版文档与 `brand.rs` 在同一次回退里被删。品牌类改动最好把「保留清单」写进代码注释（`integration/env.rs` 里已有），只靠文档容易丢。

## 9. 边界与后续

- 老机器上的 `herdr/` 目录与旧 hook 文件不做清理，新装机器不受影响。
- 若要彻底清零 `herdr.dev`，需移除 update / manifest 功能——那是独立决策，不在本文范围。
- 插件协议面（§4）是硬边界：任何后续品牌替换都不得触碰 `HERDR_ENV`、`HERDR_PLUGIN_*`、`herdr-plugin.toml`、`min_herdr_version`。
