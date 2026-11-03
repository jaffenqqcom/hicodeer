# OHOS 数据目录去品牌化与删除 onboarding 死路径（已解决）

## 问题描述 (Problem Description)

HiCodeer 在 OHOS 上的运行目录仍大量沿用上游 Zed / herdr 的命名：配置与数据落在 `$HOME/.config/zed`、`$HOME/.local/share/zed`、`$HOME/.cache/zed`，agent 客户端配置被写进一个无人读取的 `$HOME/herdr/config.toml`，agent 状态目录叫 `$HOME/.hicodeer`（与产品名重复且语义不清）。本次把目录名统一收敛到 hicodeer / agentpanel，并删除那条写入死文件的 onboarding 旁路。

## 问题表现 (Symptoms)

- 沙箱 `$HOME` 下可见（改动前）：

```
.config/zed/            settings.json, themes/
.local/share/zed/       db/, extensions/, languages/, logs/, hang_traces/, terminal/
.cache/zed/
herdr/config.toml       内容仅 onboarding = false
.hicodeer/              agent/（socket）、agent-config.toml
```

- `herdr/config.toml` 写入后没有任何进程读取：herdr 客户端实际日志写在 `.config/hicodeer/agenttui/hicodeer-client.log`，证明其 config 目录不是 `$HOME/herdr`。
- `.hicodeer` 与 `.config/hicodeer`、`.local/share/hicodeer` 语义重叠，不便区分"编辑器数据"与"agent 状态"。
- 复现方式：全新安装后启动，列 `$HOME` 即见上述目录。

## 问题原因 (Root Cause)

### 1. 目录名全部由 `APP_NAME` 派生，OHOS 未做品牌区分

`crates/paths/src/paths.rs` 的 `config_dir/data_dir/cache_dir/state_dir/temp_dir` 统一使用 `APP_NAME = "Zed"` / `APP_NAME_LOWERCASE = "zed"`，因此 OHOS 构建也生成 `zed` 目录。

同时 `crates/zed/src/main.rs:11-20` 有编译期断言：

```rust
const _: () = assert!(paths::APP_NAME_LOWERCASE == env!("CARGO_BIN_NAME"));
```

OHOS 只构建 `[lib]`（`script/bundle-ohos:530` 用 `cargo build --lib`），cargo 不会为 lib 设置 `CARGO_BIN_NAME`，所以由 `crates/zed/build.rs` 手动注入 `"zed"` 来满足断言。改名必须同时处理这两处，否则编译失败。

### 2. onboarding 旁路写的是一个无人读取的文件

- 写入方：`crates/agent_panel/src/herdr_settings.rs` 的 `client_config_dir()` → `$SANDBOX_FILES_DIR/herdr/config.toml`。
- 实际读取方：herdr 的 `config_path()`（`herdr/src/config/io.rs:206-210`）优先使用环境变量 `HICODEER_AGENT_CONFIG_PATH`，该变量由 daemon 指向 `$HOME/.hicodeer/agent-config.toml`（`hicodeerd/src/main.rs:220-235`），再经 `control.config_path` → `crates/terminal_view/src/terminal_view.rs:311` 传给 TUI 客户端。
- OHOS 上 onboarding 还恒为关：`herdr/src/config.rs:100-105` 在 `cfg!(target_env = "ohos")` 时直接 `return false`，不读配置项。

两者叠加：该写入既落错文件，又无行为作用，属于死代码。

### 3. agent 状态目录有两份独立常量

`$HOME/.hicodeer` 由两处同名常量约定：`agent_panel/src/herdr_settings.rs`（写 `agent-config.toml`）与 `hicodeerd/src/main.rs`（放 `agent/agent.sock`）。任一处不一致，面板改写配置后客户端读不到，或 app 拿不到 socket 路径。

## 解决方案 (Solution)

### 1. `APP_NAME` 按平台取值，`build.rs` 同步注入

```rust
// crates/paths/src/paths.rs
#[cfg(target_env = "ohos")]
pub const APP_NAME: &str = "HiCodeer";
#[cfg(not(target_env = "ohos"))]
pub const APP_NAME: &str = "Zed";
```

```rust
// crates/zed/build.rs
let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
let bin_name = if target_env == "ohos" { "hicodeer" } else { "zed" };
println!("cargo:rustc-env=CARGO_BIN_NAME={bin_name}");
```

build script 在 host 上编译，必须用 `CARGO_CFG_TARGET_ENV` 判断目标平台，不能用 `cfg!(target_env)`。

改后目录变为 `.config/hicodeer`、`.local/share/hicodeer`、`.cache/hicodeer`、`.local/state/hicodeer`。

### 2. 删除 onboarding 旁路

删除 `disable_startup_onboarding()`、`client_config_dir()`、`set_top_level_bool()`、常量 `CLIENT_CONFIG_DIR` / `CLIENT_CONFIG_FILE` / `ONBOARDING_KEY`，以及对应单测。`write_atomically()` 因 toast 迁移仍在用而保留。

### 3. agent 状态目录改名

`$HOME/.hicodeer` → `$HOME/.agentpanel`，两处 `AGENT_STATE_DIR` 同步修改：

- `crates/agent_panel/src/herdr_settings.rs`
- `crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs`

### 4. 不迁移旧目录（决策与风险）

本轮范围只涉及目录名，刻意未加迁移。当前设备是全新安装，旧目录不存在；但**覆盖安装且保留数据**的环境会同时存在旧 `zed`/`.hicodeer` 目录与新目录，表现为设置、扩展、会话历史"消失"。需要时再补一次性 rename 迁移。

## 修改文件 (Modified Files)

- `crates/paths/src/paths.rs` — `APP_NAME` 改为 cfg 成对常量（OHOS `HiCodeer` / 其他 `Zed`）
- `crates/zed/build.rs` — 按 `CARGO_CFG_TARGET_ENV` 注入 `CARGO_BIN_NAME`（ohos → hicodeer）
- `crates/agent_panel/src/agent_panel.rs` — 删除 `disable_startup_onboarding()` 调用
- `crates/agent_panel/src/herdr_settings.rs` — 删除 onboarding 旁路、相关常量与单测；`AGENT_STATE_DIR` 改为 `.agentpanel`
- `crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs` — `AGENT_STATE_DIR` 改为 `.agentpanel`（与面板侧保持一致）

## 验证

- `./script/bundle-ohos`：`=== HAP BUILD SUCCESSFUL ===`，退出码 0
- `./install-local.sh` 安装后，设备实测：

```
$HOME/.agentpanel/agent/agent.sock
$HOME/.agentpanel/agent-config.toml
$HOME/.config/hicodeer/{settings.json,themes/,agenttui/hicodeer-client.log}
$HOME/.local/share/hicodeer/{extensions,languages,db,logs/hicodeer.log,hang_traces}
$HOME/.cache/hicodeer/
旧 .hicodeer / .local/share/zed / .config/zed / .cache/zed 均不存在
```

- daemon 重启（`killall hicodeerd` + `setsid /data/service/hnp/bin/hicodeerd --log`）后，`agent-tui` 的 cmdline 显示 `--socket .../.agentpanel/agent/agent.sock`，证明新旧路径已对齐。

## 死路与排除项

- 只把 `CLIENT_CONFIG_DIR` 从 `"herdr"` 改成 `"hicodeer"`：只会把死文件换个位置，客户端依旧不读，未采纳。
- 曾误判 `$HOME/herdr/config.toml` 是客户端读取路径：实际客户端 config 目录由 `herdr/src/config/io.rs` 的 `app_dir_name()` 决定（OHOS 为 `hicodeer/agenttui`），且被 `HICODEER_AGENT_CONFIG_PATH` 覆盖到 `.hicodeer/agent-config.toml`。
- 曾误判 `$HOME/.local/share/Trash`（12G）属于应用：其 HOME 与文件同挂载点时才走 home trash；当前应用 HOME 在沙箱，删除落点是挂载点根的 `.Trash-<uid>`。

[[ohos-debug-lessons]]
