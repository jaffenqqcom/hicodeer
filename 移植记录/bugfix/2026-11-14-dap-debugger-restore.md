# DAP 调试栈恢复与三个可用适配器的打通

## 问题描述

HiCodeer 在把 Zed 改成产品形态的过程中，把整条 DAP（Debug Adapter Protocol）调试栈裁掉了：调试面板、调试设置页、扩展页的 Debug Adapters 分类、内置适配器注册全部缺席。用户既没有调试入口（`Ctrl+Shift+D` 无反应、状态栏没有图标），也没有可用的调试适配器。本轮要把调试能力恢复回来，并且要在 OHOS 上真正跑通，而不是只恢复代码。

## 问题表现

- `Ctrl+Shift+D` 完全没反应；底部状态栏也没有类似终端、大纲的调试图标。
- 设置里没有 Debugger 页，`窗口与布局 → 状态栏` 里没有「调试器按钮」开关，`Panels` 页里没有 Debugger Panel 分组。
- 扩展页的分类筛选里没有「Debug Adapters」。
- 调试面板里的条目只来自 `.hicodeer/debug.json`，但没有任何适配器能起来。
- 恢复代码后依次暴露四个平台问题：
  - `lldb-vscode` 走 stdio 时，`initialize`、`launch` 都正常、`process` 事件也收到了，随后进程 SIGABRT（exit -6），stderr 为空。
  - `dlv dap` 以 `--listen 127.0.0.1:port` 起，Zed 连上去立刻被断：`closing connection from different user (127.0.0.1:…) `。
  - Python 走上游默认流程时报 `Did not find a .whl in ".../debug_adapters/Debugpy/wheels"`，但该目录里确实躺着 5.4MB 的 `debugpy-1.8.22-py2.py3-none-any.whl`。
  - JavaScript 适配器拉起来后 Zed 报 `debugger shutdown unexpectedly`，adapter 的 stderr 是 `Error: listen EPERM: operation not permitted .../node-cdp.<pid>-<rand>.sock`。
- 另有一次装包失败：`code:9568409 Failed to install the HAP because the extract of the native package failed`。

## 问题原因

### 1. 调试面板从未进入 workspace

`crates/zed/src/zed.rs` 的 `initialize_panels` 里创建了 `debug_panel`，但裁剪过程中漏掉了对应的 `add_panel_when_ready(debug_panel, …)` 那一行。面板构造函数被调用、任务被创建，却没人把它注册进 workspace，于是 `Ctrl+Shift+D` 对应的 `debug_panel::ToggleFocus` 找不到面板，静默无效果。这类"创建了但没注册"的缺口不会报错，只能靠逐行比对裁剪前代码发现。

### 2. lldb-vscode 的 stdio 模式在本平台会崩

stdio 传输意味着适配器进程的 stdin/stdout 被当成 DAP 通道，而这两条 fd 在本机是由命令代理（hicodeerd）中转的。实测 `initialize`/`launch` 都能正常应答、`process` 事件也发出来了，随后进程直接 abort，stderr 空。同一条 `lldb` 命令行跑 `git version` 完全正常，说明不是 ptrace 或签名问题，而是 stdio 这条传输路径本身。lldb-vscode 支持 `-p <port>` 的 TCP 服务模式，改用它即可。

### 3. Delve 的 same-user 校验在本机误判

dlv 默认只允许同用户连接 DAP server（`--only-same-user` 默认 true）。本机即使双方都是 uid 20020117、连的是 127.0.0.1，仍被判成 "different user" 而断开。这是平台身份判定与 dlv 预期不一致，只能显式关掉该检查。

### 4. Python 上游流程跨 uid 断链（本轮最关键的认知）

上游流程是：hicodeer 请求 PyPI 取版本 → 在 app 数据目录建 venv → `pip download` 把 wheel 下到 `<data_dir>/debug_adapters/Debugpy/wheels` → 解压 → 装进 venv。

这条链上有两个执行主体：

- HTTP 请求（PyPI 版本 JSON）与 zip 解压发生在 **hicodeer 进程内**；
- `python -m venv`、`pip download`、`pip install` 都经 `util::command` 路由到 **hicodeerd** 执行。

而 hicodeerd 看不到 hicodeer 下载/生成的 app 数据目录内容（这是本机的硬约束，只有当前打开的项目目录双方都可见）。于是 pip 在自己的视角里往同名路径写，hicodeer 在自己的视角里读，读到的是空目录，报 "Did not find a .whl"。文件确实被创建过（20:34 能在 hicodeerd 侧看到 5.4MB 的 wheel），但两边不是同一份视图。

死路记录：先怀疑是 pip 网络超时、又怀疑是 pip 未写入，最后按"hicodeerd 只认项目目录"的约束才定位。期间用 shell 在 `/data/storage/el2/base/haps/entry/files/.dap-tmp` 下建 venv、跑 pip 都成功，但这些实测是 shell 视图，不代表 hicodeerd。

### 5. JavaScript 适配器的 UDS 死结

js-debug 给被调试进程开的是 Unix domain socket，路径写死为 `os.tmpdir()/node-cdp.<pid>-<rand>.sock`（非 Windows 一律如此，代码里没有 TCP 回退，也没有环境变量开关）。

- `/storage` 全盘 bind AF_UNIX 返回 EPERM（`/tmp`、`/dev/shm` 同样被拒）；
- 唯一能 bind 的位置在 app 数据区（`…/files/tmp`），而按上面的约束 hicodeerd 又看不到。

两个条件互斥，所以 JavaScript 在本平台不可用。另外它的适配器包只能从 GitHub release 资源下载，本机直连该资源域名会卡住（api.github.com 可达，release 下载地址 90 秒 0 字节、240 秒 SSL EOF），即使 UDS 通了也要先解决下载。

### 6. CodeLLDB 不可用

CodeLLDB 的做法是从 VSIX 里解出 `codelldb` 原生 ELF 再 exec。OHOS 内核拒绝执行未签名 ELF（`Permission denied (os error 13)`），下载下来也跑不起来。GDB 则是本机根本没有该程序，harmonybrew 也没有对应 formula。

### 7. 装包失败 9568409

那次构建产出的 HAP 只有 111MB、条目里没有 `hnp/`，而 `hap/entry/hnp/arm64-v8a/` 下的两个 `.hnp` 都已生成。即 hnp 注入那一步没有作用到最终签名产物上。重新构建后日志出现 `verified 15 hap entries + 2 hnp payload(s)`、产物 163MB，装包即成功。

### 8. 改英文 key 导致中文丢失

`localized_str!` 用**英文原文作为 key** 去 `assets/locales/zh-CN.json` 的 `messages` 里查译文。把 "…across Zed sessions." 改成 "…across HiCodeer sessions." 后 key 变了，旧译文对不上，于是整句回退英文。

## 解决方案

### 恢复被裁剪的 DAP 栈

以真正执行裁剪的提交（`4ea6225`）的父提交为基准，逐文件恢复，能保持原样的保持原样：

- `initialize_panels` 补回 `add_panel_when_ready(debug_panel, …)`（这就是 `Ctrl+Shift+D` 失效的根因）。
- `main.rs` 恢复 `debug_adapter_extension::init`、`debugger_ui::init`、`debugger_tools::init`、`dap_adapters::init`，并恢复 `init_paths` 里的 `debug_adapters_dir()`。
- 工作区 `Cargo.toml` / `crates/zed/Cargo.toml` 恢复 `dap_adapters`、`debug_adapter_extension`、`debugger_tools`、`debugger_ui` 四个成员与依赖。
- `assets/settings/default.json` 恢复 `debugger` 默认块；`settings_content` 恢复 `debugger` 字段与 flattened 反序列化项；`vscode_import` 补 `debugger: None`。
- `debugger_panel.rs` 的两处文档链接由 `https://zed.dev/docs/debugger` 改为 GitHub 上的 `docs/src/debugger.md`（`open_url` 对 Zed 自有域名有拦截）。
- 键位：`default-ohos.json` 恢复 `f4`/`f5`/`f6`/`f7`/`f10`/`f11`/`shift-f5`/`ctrl-shift-f5` 及 `DebugPanel`、`VariableList`、`BreakpointList` 三个上下文；`vim.json` 恢复 8 条 Helix 风格 `space shift-g *` 绑定（与裁剪前逐字一致）。`Ctrl+Shift+D` 还给 `debug_panel::ToggleFocus`，agent panel 移到 `Ctrl+Alt+A`（该组合原本空闲）。
- 设置页恢复：状态栏分组补回「Debugger Button」（`debugger.button`，数组长度 11→12），新增 `debugger_page()`（步进粒度 / 保存断点 / 超时 / DAP 通信日志 / 日志格式化）与 `debugger_panel_section()`（面板停靠位置），页面顺序照旧插在 `panels_page` 与 `terminal_page` 之间。
- 扩展页恢复 Debug Adapters 分类：OHOS 门控只保留 MCP 的排除项。

### 三个适配器的平台适配

- **LLDB（新增 `lldb_vscode.rs`）**：直接驱动 ohos-sdk 自带的 `lldb-vscode`，不下载、不改写可执行文件。路径解析顺序为配置项 `lldb_vscode_path` → 用户设置 `dap.LLDB.binary` → `which("lldb-vscode")`。连接方式改为 `-p <port>` + `TcpTransport`，这是绕开 stdio SIGABRT 的唯一办法。整个模块与注册都用 `#[cfg(target_env = "ohos")]` 门控，不影响其它平台的适配器列表。
- **Delve**：OHOS 分支完全绕开 minidelve shim（下载来的二进制无法执行），直接 `dlv dap --listen <host>:<port> --only-same-user=false` 并走 TCP。参数构造抽成纯函数 `system_delve_dap_arguments`，配一条单测钉住这 4 个参数。
- **Python**：放弃上游 venv/wheel 流程，改走系统已安装的 debugpy —— `python -m debugpy.adapter --host=… --port=…`，解释器由上游的 `system_python_name` 动态探测（`python3`/`python`/`py`），不钉版本。参数构造同样抽成 `system_debugpy_arguments` 并配两条单测。用户在 `dap.Debugpy.binary` 里配了自定义适配器时会打一条 warn 说明该设置在本平台被忽略。

### 删除 CodeLLDB

删除 `codelldb.rs`、模块声明与注册行，并把所有会指向它的入口改到 LLDB：`crates/grammars/src/{c,cpp,rust}/config.toml` 的 `debuggers`、`assets/settings/default.json` 的 `Rust.debuggers`、`vscode_debug_format.rs` 的 `cppdbg|lldb` 映射（含对应单测）、仓库自带的 `.zed/debug.json`。不这么做的话删完只会留下"选了就跑不起来"的入口。

### 文案与 i18n

- 设置页两条描述里的 `Zed` 改为 `HiCodeer`；`dap/src/transport.rs` 与 `debugger_ui/src/session/running.rs` 的运行时错误提示同步替换。
- 因为 `localized_str!` 以英文原文为 key，改文案等于换 key，必须在 `assets/locales/zh-CN.json` 补上新 key 的中文译文（沿用原译文，只把 Zed 换成 HiCodeer）。`catalog_sha256` 保持不变，否则整个语言包会被判为不匹配而全部回退英文。改完 `python3 script/check-i18n` 通过（2649 keys，全部 bundle 完整）。

### 遗留问题

- Python 的 attach（`connect.host/port`）在 OHOS 分支仍不支持：`system_debugpy_binary` 固定 listen 模式，会忽略 `connect` 配置。上游 `AttachWithConnect` 的端口语义尚未验证，暂不实现。
- `lldb_vscode_args` 若自带 `-p` 会与追加的端口参数冲突，属边缘情况，未处理。
- `go.rs` 的 `let _ = user_args` 会静默丢弃自定义 adapter 参数（按用户要求保留现状）。

## 修改文件

- `crates/zed/src/zed.rs` — 注册 debug panel、补 DAP 日志工具栏项、测试环境初始化调试 UI
- `crates/zed/src/main.rs` — 恢复 DAP 各 crate 的 init 调用与 `debug_adapters_dir` 路径
- `crates/zed/Cargo.toml`、`Cargo.toml`、`Cargo.lock` — 恢复四个 DAP 相关 crate 的成员与依赖
- `crates/dap_adapters/src/dap_adapters.rs` — 移除 CodeLLDB、注册 LLDB（OHOS 门控）
- `crates/dap_adapters/src/codelldb.rs` — 删除
- `crates/dap_adapters/src/lldb_vscode.rs` — 新增：驱动系统 `lldb-vscode`，TCP 传输
- `crates/dap_adapters/src/go.rs` — OHOS 分支直连 `dlv dap`（关闭 same-user 校验），参数构造抽函数并加单测
- `crates/dap_adapters/src/python.rs` — OHOS 分支改用系统 debugpy（`-m debugpy.adapter`），参数构造抽函数并加两条单测，自定义适配器被忽略时告警
- `crates/dap/src/debugger_settings.rs` — 恢复从 settings 读取调试配置
- `crates/dap/src/transport.rs` — 错误提示中的 Zed 改为 HiCodeer
- `crates/debugger_ui/src/debugger_panel.rs` — 两处文档链接改到 GitHub
- `crates/debugger_ui/src/session/running.rs` — 错误提示中的 Zed 改为 HiCodeer
- `crates/settings_content/src/settings_content.rs` — 恢复 `debugger` 字段与反序列化项
- `crates/settings/src/vscode_import.rs` — 补 `debugger: None`
- `crates/settings_ui/src/page_data.rs` — 恢复 Debugger 设置页、Panels 页的 Debugger Panel 分组、状态栏的 Debugger Button 开关
- `crates/extensions_ui/src/extensions_ui.rs` — 恢复扩展页的 Debug Adapters 分类
- `crates/task/src/vscode_debug_format.rs` — `cppdbg|lldb` 映射由 CodeLLDB 改为 LLDB，同步单测
- `crates/grammars/src/{c,cpp,rust}/config.toml` — 语言首选调试器由 CodeLLDB 改为 LLDB
- `assets/settings/default.json` — 恢复 `debugger` 默认块，`Rust.debuggers` 改为 LLDB
- `assets/keymaps/default-ohos.json` — 恢复调试键位与三个调试上下文，`Ctrl+Shift+D` 归还调试面板
- `assets/keymaps/vim.json` — 恢复 8 条 Helix 风格调试绑定
- `assets/locales/zh-CN.json` — 补两条改动后英文 key 的中文译文
- `.zed/debug.json` — 仓库自带调试配置的适配器由 CodeLLDB 改为 LLDB

[[ohos-debug-lessons]]
