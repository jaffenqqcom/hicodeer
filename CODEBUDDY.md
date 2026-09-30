# CODEBUDDY.md

This file provides guidance to CodeBuddy Code when working with code in this repository.

## 项目定位

HiCodeer 是把 Zed 编辑器移植到 HarmonyOS NEXT（OHOS）的产品：代码库本体是 Zed 上游 + OHOS 移植层。产品名 HiCodeer，包名 `com.hicodeer.studio`，引擎库 `libhicodeer.so`。

仓库里并存三套规矩，改码前都要过一遍：

- `.rules`（`AGENTS.md`、`CLAUDE.md`、`GEMINI.md` 均符号链接到此）：上游 Zed 的 Rust 编码规范、GPUI 用法、PR hygiene、崩溃调查流程。
- 本文件：OHOS 移植特有的构建入口、架构与坑。
- 全局 `~/.codebuddy/CODEBUDDY.md`：语言、日志、授权、禁 clean / 禁 release / 禁子 agent 等硬规则。

## 常用命令

构建（唯一入口，不要手拼 cargo）：

- `./script/bundle-ohos` — debug 构建 + 打包签名。产物固定在 `hap/entry/build/default/outputs/default/entry-default-signed.hap`。
- `./script/bundle-ohos --hap-only` — 只跑 `ohpm install` + `hvigor assembleHap`，跳过 Rust 编译。
- `--release` 属需先获明确授权的动作（全局规则第 17 条），禁止自行启动。
- 脚本内硬编码 `RUSTUP_TOOLCHAIN=1.97.1`、目标三元组 `aarch64-unknown-linux-ohos`；引擎 crate 是 `launch-zed`，其 `[lib] name = "hicodeer"` 使产物直接叫 `libhicodeer.so`。
- 库名三方必须一致：`OHOS_LIB_NAME`（脚本）、`NAPI_BUILD_TARGET_NAME=hicodeer`（脚本）、`EntryAbility.moduleName = "hicodeer"`（`hap/entry/src/main/ets/entryability/EntryAbility.ets`）。改一处就得改齐。注意 `zed` crate 的 `[lib] name` 不能改（会断掉 `use zed::`）。

装包与设备（本机即设备）：

- `./install-local.sh` — 覆盖安装最新 HAP 并启动，保留沙箱数据，属常规授权范围。
- `./install-local.sh --reinstall` 会**先卸载再装**（清空沙箱，丢用户配置），属需授权动作（全局规则第 21 条）。`--port <设备调试端口>` / `--hap <路径>` / `--no-start` 可选。
- hdc 连接与 hilog 落盘一律走固化入口：skill `ohos-hdc-connect` 与 `~/.codebuddy/scripts/hilog-persist.sh`。不要临场手搓 `hdc` 序列，也不要 `hdc kill`。

Lint / 格式：

- `./script/clippy` — 用这个，不要直接 `cargo clippy`。它默认 `--workspace --release --all-targets --all-features -- --deny warnings`，并在非 OHOS 目标上自动排除 OHOS 专属 crate（`launch-zed`、`gpui_ohos`、`gpui_ohos_linker`、`qemu-ssh-agent`、`qemu-ssh-agent-linker`）；本地装了 `cargo-machete` / `typos` / `buf` 时还会顺带跑这些检查。
- `cargo fmt`（配置见 `rustfmt.toml`）。
- `./script/check-licenses` — 许可证合规，CI 会卡。

测试：

- 全量：`cargo test --workspace`，或 `cargo nextest run --workspace --no-fail-fast`（超时与串行组见 `.config/nextest.toml`）。
- 单个测试：`cargo test -p <crate> <test_name>`（或 `cargo nextest run -p <crate> <test_name>`）。
- GPUI 测试里驱动 `run_until_parked()` 时用 `cx.background_executor().timer(...)`，不要用 `smol::Timer::after(...)`（见 `.rules`）。

## 架构

### 三层进程模型

- 外壳：ArkTS 应用 `com.hicodeer.studio`（uid 20020220），负责窗口、输入法接入、权限与生命周期。
- 引擎：`libhicodeer.so`（Rust/gpui），与应用同进程同 uid。
- 守护进程：`hicodeerd`（uid 20020117），以 HNP 公共包由 HNP 框架拉起，**父进程不是应用**，因此读不到应用的环境变量、也不知道应用的工作目录。

关键推论：数据根只能靠 SSH 握手传递；调用方那些「只在它自己沙箱内能解析」的环境变量值，跨 uid 传过去必然失效。

### 启动链路

```
系统拉起 HAP
 → EntryAbility (ArkTS, RustAbility 基类)
 → 加载 libhicodeer.so → NAPI init（launch-zed 的 #[ability] 宏生成）
 → launch_app() (crates/gpui_ohos/depend/launch-zed/src/launch_app.rs)
 → set_global_app() + zed::start_zed_main(base_path)
 → start_zed_main() / main() (crates/zed/src/main.rs)
 → build_application() → OhosPlatform::new() (crates/gpui_ohos/src/ohos/platform.rs)
 → app.run() → OhosPlatform::run() 注册事件回调
 → 收到 Event::SurfaceCreate → 触发 on_finish_launching → 顺序 init 全部模块 → 打开首个窗口
```

- `on_finish_launching` 只在 `SurfaceCreate` 到达时触发一次，全部模块 init 都挤在这一个闭包里、按固定顺序执行（`crates/zed/src/main.rs`）。窗口创建依赖它，也依赖 `Event::UserEvent` 驱动 foreground 任务队列——前者不来，窗口永远不创建（黑屏）。
- 日志双 tag：`zlog::init()` 之前（`start_zed_main`）直连 hilog，tag=`hicodeer-boot`；之后 `log::*` 走重定向，tag=`HiCodeer`。抓日志两个 tag 都要过滤。
- 初始化卡点定位：hilog 里 `[boot] enter init: <模块>` 出现而对应 `exit init` 未出现，即卡在该模块；一个 `enter init` 都没有则卡在更早。

### 命令执行链路

OHOS 上所有外部进程经 `crates/util/src/command/ohos.rs` 这一唯一入口：

- `local_exec_matches()` 按可执行文件 basename 匹配 HNP 名单；命中 → 应用进程内本地 fork。
- 未命中 → 组装 `ExecSpec` 交守护进程执行。
- 客户端走连接池（下限 5 / 上限 16）复用 loopback SSH 连接；守护进程双 listener：4022 命令（每次运行现场生成动态密钥）、4023 管理（固定密钥，只服务握手，把动态密钥交给客户端）。
- 协议常量在 `cmd-client/src/protocol.rs` 与 `hicodeerd/src/protocol.rs` 手抄两份、刻意不共享依赖：改一侧必须同步另一侧。
- 远端命令**不带 env**，子进程继承守护进程自身环境。一次性初始化（会话临时目录、Node 身份垫片）只能挂在握手处，因为那一刻数据根才知道。

### 关键代码位置

- `crates/gpui_ohos/` — OHOS 平台后端：`src/ohos/platform.rs`、`window.rs`、`wgpu_context.rs`、`wgpu_renderer.rs`、`dispatcher.rs`；`depend/` 下是 OHOS 专属 crate：`launch-zed`（NAPI 入口，产出 `libhicodeer.so`）、`cmd-agent/`（`cmd-client` + `hicodeerd`）、`qemu-mngt`、`ohos-libc-shim`。
- `crates/zed/src/main.rs` — Zed 完整启动逻辑与模块 init 顺序。
- `hap/` — HAP 工程：ArkTS 入口、`build-profile.json5`（签名与包名）、`module.json5`（权限与 HNP 声明）。
- `patches/` — 需 OHOS 改动的第三方 crate 本地补丁（`cap-primitives`、`rustix-openpty`、`which`、`wasmtime` 等），由根 `Cargo.toml` 的 `[patch.crates-io]` 消费。

### 平台边界与硬事实

- 改共享码必须用 `#[cfg(target_env = "ohos")]` 门控；无法门控处用 `[OHOS PORT BEGIN/END]` 注释对，新增 OHOS 专属文件加头注释。
- 可改范围：`crates/gpui_ohos/**`、`hap/**`、`patches/**`（路径含 `ohos` 的文件）。`crates/gpui/**` 上游代码禁改。
- 平台硬事实：`/tmp` 是 erofs 只读，`/var/tmp` 与 `/data/local/tmp` 不存在 ⇒ 临时文件必须靠 `TMPDIR`（标准库 `temp_dir()` 只认它）。任何新落盘 ELF 必须自签（段表出现 `.codesign`）再 `chmod 0o775`，否则内核拒绝 exec 报 `Permission denied (os error 13)`。内核缺 `openat2`（探测会 SIGSYS 直接杀进程，已 patch `cap-primitives`）。wasm JIT 需在 HAP 声明 `ohos.permission.kernel.ALLOW_WRITABLE_CODE_MEMORY`。
- OHOS 只支持单窗口：`OhosPlatform::open_window` 对第二个窗口直接 `bail!`。依赖新开窗口的功能在 OHOS 不可用——miniprofiler 面板因此改走非窗口出口，设置界面因此改用 tab。
- 后台任务是 worker pool 线程（`gpui-ohos-bg-N`），不是主线程；任务内直接触 NAPI 会 SIGABRT。跨线程 NAPI 必须走 `OpenHarmonyApp::bridge()` 的 TSFN 封装。
- HNP 服务进程（`hicodeerd`）**不随 HAP 覆盖安装重启**：磁盘上的 `.hnp` 换了，跑着的旧映像仍占 4022/4023。换新版 daemon 只能重启设备或卸载重装 bundle。

## 知识库（分析 / 定位 / 改码前先读）

- `.workbuddy/memory/ARCH/README.md` — 架构知识库索引：01 总览 / 02 命令代理 / 03 QEMU / 04 构建 / 05 平台硬事实 / 06 LSP / 07 IDE / 08 ACP / 09 坑索引。风格为「结论先行 + 每条带 `file:line` 取证」；行号是易碎品，引用前当轮核过，核不动就只写路径。
- `.workbuddy/memory/MEMORY.md` 与同目录 `YYYY-MM-DD.md` — 项目长期记忆与日流水（原始素材，允许啰嗦/被推翻）。
- `~/.codebuddy/skills/hicodeer-codemap/SKILL.md` — 现成的代码地图：启动流程、模块入口、跨运行时边界、terminal / hicodeerd 运行路径、出网总闸、profiler 采样链路。不要重复推导。
- `移植记录/` — **只读，勿改动**。`design/` 是设计方案，`bugfix/` 是按日期命名的缺陷记录（约 68 篇），`zed问题定位手段/` 是定位手段与检索工具。
- 检索反模式：本仓 `grep` 不可信，零命中必须用 `python3 移植记录/zed问题定位手段/psrch.py <root> <regex>`（搜 `depend/` 加 `--depend`）复核。

## 上游规则要点（`.rules`，同样必须遵守）

- HARD RULE：改任何源文件前，若 `README.md` 顶部没有，须先补两行 `> [!IMPORTANT]` 与 `> Remove this line to confirm you've reviewed this PR before submitting.`；且**永不自行删除**这两行（删掉是人工确认才做的步骤）。
- 失败操作不要用 `let _ =` 静默吞掉；避免 `unwrap()`，用 `?` 传播；用 `.log_err()` 等保留可见性。
- 新建模块用 `src/some_module.rs`，不要 `mod.rs`；新建 crate 在 `Cargo.toml` 里用 `[lib] path = "..."` 指定库根。
- 变量名用全词、不用缩写；async 上下文中用 shadowing 限定 clone 生命周期。
- 用 `./script/clippy`，不要直接 `cargo clippy`。
- PR 标题用祈使句、无 conventional 前缀、无尾标点；PR body 末尾必须有 `Release Notes:` 段（`- Added ...` / `- Fixed ...` / `- Improved ...`，或 `- N/A`；改动 gpui 时加 `[GPUI]` 前缀条目）。
- 崩溃调查入口：`script/sentry-fetch <issue-id>`、`script/crash-to-prompt <issue-id>`，prompt 位于 `.factory/prompts/crash/`。
- Rules hygiene：`.rules` 不在日常功能/修复工作中内联编辑（它被三个 agent 文件名共享）；新规则需满足「非显然 + 反复遇到 + 具体可操作」，由专门的 commit 加入。
