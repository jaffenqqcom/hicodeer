# 项目本地配置目录与任务变量仍暴露 Zed

## 问题描述

HiCodeer 的项目本地配置目录与任务变量前缀仍然沿用 Zed 的命名。用户从「最近项目 → 工作树」点击「自动化工作树」设置后，应用在项目根创建并打开 `.zed/tasks.json`，文件内容是工作树钩子任务模板，示例命令里写着 `$ZED_WORKTREE_ROOT` / `$ZED_MAIN_GIT_WORKTREE`。同样的问题散布在调试配置模板、各语言默认任务、项目信任安全弹窗、撤销安全检查提示、调试进程创建弹窗中。这些都属于用户直接看得见的面，与「交付物不得含任何 Zed 字样」的目标冲突。

该问题不是单点文案问题：路径常量、目录名比较、任务变量前缀是三条互相独立的数据链，只改其中一条会导致「改了却没生效」或「模板变量解析不出来」。

## 问题表现

- 点击「自动化工作树」后，项目根出现 `.zed/` 目录与 `tasks.json`，文件头部注释与示例命令含 `$ZED_*`。
- 调试面板新建配置时的「工作目录」占位符是 `Ex: $ZED_WORKTREE_ROOT`；「程序」占位符是 `ENV=Zed ~/bin/program --option`。
- 各语言默认任务（Go / Rust / TypeScript / JSON / Python）与 DAP 适配器默认场景里写的是 `$ZED_FILE` / `$ZED_WORKTREE_ROOT` / `$ZED_CUSTOM_*`。
- 打开不受信任项目时，安全弹窗正文是 `Review .zed/settings.json for any extensions or commands configured by this project.`；撤销安全检查告警、新进程调试弹窗提到 `.zed/settings.json`。
- 隐蔽表现：把路径常量改成 `.hicodeer` 之后，项目里已有的 `.hicodeer/tasks.json` 不会被任务清单识别，任务面板里看不到该文件定义的任务。原因是另有字面量比较没跟着改，见根因二。

## 问题原因

### 根因一：路径常量单点硬编码

`crates/paths/src/paths.rs:504` 的 `local_settings_folder_name()` 直接返回字符串 `".zed"`；同文件 `local_settings_file_relative_path()`（`:523`）、`local_tasks_file_relative_path()`（`:541`）、`local_debug_file_relative_path()`（`:575`）把 `.zed/settings.json`、`.zed/tasks.json`、`.zed/debug.json` 写死在 `LazyLock` 里。所有「创建 / 打开 / 监听项目本地配置」的代码都经过这四个函数，所以这四个函数决定了目录名从哪里来。

### 根因二：两处字面量比较与常量脱钩（「改完不生效」的真凶）

`crates/project/src/task_inventory.rs:92` 与 `:504` 用 `name == ".zed"` 判断工作树里是否存在项目配置目录，据此决定要不要扫描项目任务文件。目录名换成 `.hicodeer` 后这里恒为 false，扫描直接被跳过——任务清单于是「看不见」`.hicodeer/tasks.json`。`crates/debugger_ui/src/new_process_modal.rs:1080` 也有同类字面量比较。

这解释了为什么「只改 paths.rs」的做法在自查时看起来对、实际功能是坏的。

### 根因三：任务变量前缀是独立数据链

`crates/task/src/task.rs:254` 定义 `ZED_VARIABLE_NAME_PREFIX = "ZED_"`，`FromStr for VariableName`（`:218`）按它解析模板里的 `$ZED_XXX`，`Display`（`:260` 起）按它生成环境变量名。模板文件（`assets/settings/initial_debug_tasks.json`、`initial_worktree_setup_tasks.json`、各语言默认任务）里写的是字面量 `$ZED_*`。

两条必须同改：只改前缀常量而模板还是 `$ZED_*`，模板变量解析失败、原样留在命令行里；只改模板而前缀还是 `ZED_`，新模板同样解析失败。

### 根因四：默认配置里的文件模式

`assets/settings/default.json` 的 `private_files` 与 `file_types.JSONC` 只列了 `.zed/` 模式，`.hicodeer/settings.json` 既不会被当成私密文件，也不会按 JSONC（允许注释）高亮。

## 解决方案

### 一、路径按平台分叉

`local_settings_folder_name`、`local_settings_file_relative_path`、`local_tasks_file_relative_path`、`local_debug_file_relative_path` 四个函数各写成一对互斥分支：`#[cfg(target_env = "ohos")]` 返回 `.hicodeer` 系列，`#[cfg(not(target_env = "ohos"))]` 保留 `.zed` 系列。原始值一行未删，只在前面插入 OHOS 分支。

### 二、把字面量比较收敛到常量

`task_inventory.rs` 两处与 `new_process_modal.rs` 一处改为与 `local_settings_folder_name()` 比较。这一步是功能性修复，不做则第一步等于没做。

### 三、变量前缀与模板同名同改

- `crates/task/src/task.rs`：`ZED_VARIABLE_NAME_PREFIX` → `HICODEER_VARIABLE_NAME_PREFIX`（值 `"HICODEER_"`），`ZED_CUSTOM_VARIABLE_NAME_PREFIX` → `HICODEER_CUSTOM_VARIABLE_NAME_PREFIX`；`FromStr` 与 `Display` 全部改用新常量。
- 所有模板、默认场景、用户提示里的 `$ZED_*` → `$HICODEER_*`。
- 不保留旧 `ZED_` 前缀的解析回退：回退会让 `FromStr` 多一条分支，且本项目只支持最新 HiCodeer 的配置，没有要兼容的历史用户。

### 四、用户可见文案与默认配置

- 安全弹窗、撤销检查告警、新进程调试弹窗里的 `.zed/settings.json` 一律 cfg 双分支，OHOS 用 `.hicodeer/settings.json`，非 OHOS 保留原文。
- 调试弹窗两个占位符：工作目录 `Ex: $HICODEER_WORKTREE_ROOT`，程序 `ENV=HiCodeer ~/bin/program --option`（后者是本轮补齐时发现的遗漏，见下）。
- `assets/settings/default.json` 追加 `.hicodeer/settings.json` 与 `.hicodeer/*.json` 两条模式，原 `.zed` 模式保留。

### 为什么这样选

- 用常量收敛而不是把 `.hicodeer` 字符串抄到每个调用点：抄字符串正是这次出问题的形态（paths.rs 改了，另两处没改）。
- 路径函数用 cfg 双分支而不是运行期判断 `cfg!`：保持零分配、保持 `&'static str` 返回类型不变，也避免运行期分支。
- 变量前缀走全局改名而不是 cfg 双常量：模板文件（assets 与各语言默认任务）是运行时资源，同一份资源服务所有平台，无法按平台分叉。若常量分叉而模板不分叉，非 OHOS 平台拿到 `HICODEER_` 模板却按 `ZED_` 解析，结果比统一改名更糟。这里如实记下这条已知偏离：非 OHOS 平台的 `$ZED_*` 变量随之改名。

## 修改文件

路径与配置来源：

- `crates/paths/src/paths.rs` — 项目本地配置目录/设置/任务/调试文件四个路径函数按平台分叉为 `.hicodeer` 与 `.zed`。
- `crates/project/src/task_inventory.rs` — 两处 `name == ".zed"` 改为与 `local_settings_folder_name()` 比较（功能修复：否则 `.hicodeer/tasks.json` 不被扫描）。
- `crates/project/src/project_settings.rs` — 监听器注释同步为项目本地配置口径。
- `crates/project/src/project.rs` — `disable_ai` 项目级配置路径注释同步。
- `crates/debugger_ui/src/new_process_modal.rs` — 工作目录判断改用 `local_settings_folder_name()`；两个占位符按平台分叉（`$HICODEER_WORKTREE_ROOT`、`ENV=HiCodeer ...`）。

任务变量前缀与模板：

- `crates/task/src/task.rs` — 前缀常量改 `HICODEER_`，`FromStr`/`Display` 同步，不保留 `ZED_` 回退。
- `crates/task/src/task_template.rs` — 模板解析与校验用例改用新前缀。
- `crates/task/src/vscode_format.rs`、`crates/task/src/vscode_debug_format.rs` — VS Code 格式转换后的变量改用新前缀。
- `crates/dap_adapters/src/go.rs`、`javascript.rs`、`python.rs` — 适配器默认调试场景变量。
- `crates/debugger_ui/src/debugger_panel.rs` — 新建调试配置的默认模板变量。
- `crates/debugger_ui/src/session/running.rs` — 会话提示里的变量写法。
- `crates/languages/src/go.rs`、`json.rs`、`python.rs`、`rust.rs`、`typescript.rs` — 各语言默认任务模板变量。
- `crates/project/src/debugger/locators/python.rs` — Python 解释器定位里读取的变量名前缀。
- `crates/tasks_ui/src/modal.rs`、`crates/tasks_ui/src/tasks_ui.rs` — 任务弹窗提示与用例。
- `crates/zed/src/zed.rs` — 工作树钩子任务示例文本（注释与示例命令）。
- `assets/settings/initial_debug_tasks.json` — 默认调试任务模板变量。
- `assets/settings/initial_worktree_setup_tasks.json` — 工作树钩子模板变量与注释。

用户可见文案：

- `crates/workspace/src/security_modal.rs` — 项目信任弹窗正文按平台分叉。
- `crates/ui/src/components/notification/alert_modal.rs` — 撤销安全检查告警文案按平台分叉。
- `crates/debugger_ui/src/debugger_panel.rs` — 同上（调试面板内的提示）。
- `crates/terminal_view/src/terminal_panel.rs` — 用例里的命令示例变量。
- `crates/migrator/src/migrator.rs` — 迁移用例注释与断言。

默认设置与 i18n：

- `assets/settings/default.json` — `private_files` 与 `file_types.JSONC` 追加 `.hicodeer` 模式。
- `assets/locales/zh-CN.json` — 新增 `Ex: $HICODEER_WORKTREE_ROOT`、`Review .hicodeer/settings.json ...` 等新文案的中文词条。

测试同步（`#[cfg(test)]` 区与集成测试）：

- `crates/project/tests/integration/context_server_store.rs`、`lsp_store.rs`、`project_tests.rs`、`task_inventory.rs`、`debugger.rs` — 用例里的目录名与变量名。
- `crates/debugger_ui/src/tests/new_process_modal.rs` — 目录名断言。
- `crates/editor/src/editor_tests.rs`、`crates/git/src/status.rs`、`crates/git_ui/src/git_graph.rs`、`crates/git_ui_core/src/worktree_service.rs`、`crates/repl/src/kernels/native_kernel.rs`、`crates/settings_ui/src/settings_ui.rs` — 测试区内的目录名与变量名。

## 验证

- `./script/bundle-ohos`：EXIT=0，日志出现 `=== HAP BUILD SUCCESSFUL ===`，产物 `hap/entry/build/default/outputs/default/entry-default-signed.hap`。
- `cargo test -p task`：27 passed，0 failed（前缀改名后用例断言同步）。
- `script/check-i18n`：2634 keys，全部 bundle 完整。
- 源码扫描：`$ZED_` 在 Rust 源码里只剩 5 处，全部属于非 OHOS 分支、CI 工作流或 WSL 测试辅助，无 OHOS 可达路径。
- `assets/` 下（不含 locales）无 `$ZED_` 残留、无 Zed 字样。

## 遗留

- `crates/debugger_ui/src/new_process_modal.rs` 的 Program 占位符是本轮复核时才补的（原文 `ENV=Zed ~/bin/program --option`），此前只改了工作目录占位符。

## 关于「其它 Zed 文案是否会进 image」的核查

初稿曾把欢迎页 `Welcome to Zed`、Inspector `Click to open by running Zed CLI`、onboarding `Welcome to Zed` 等列为「待处理的遗留」，这个判断是错的：只 grep 到源码文本就下结论，没有看 cfg 上下文。实际核查结果如下。

代码侧文案全部不进货：

- `crates/workspace/src/welcome.rs` 的 `welcome_headline()` 与 `welcome_tagline()`、`crates/onboarding/src/onboarding.rs` 的 `onboarding_title()`、`crates/inspector_ui/src/inspector.rs` 的 tooltip 都已按平台分叉，OHOS 走 `Welcome to HiCodeer` / `Click to open the source location`。
- `crates/title_bar/src/title_bar.rs` 的 `Please restart/update Zed to Collaborate` 整段在 `#[cfg(not(target_env = "ohos"))]` 内。
- `crates/auto_update`、`crates/install_cli`、`crates/ui/src/components/collab/`、`crates/zed/src/zed/move_to_applications.rs` 不参与 OHOS 构建或属 macOS 专属。

实测判据（对 HAP 内 `libs/arm64-v8a/libcore.so` 流式逐块搜索，362 392 968 字节）：

- `Welcome to Zed`、`Welcome back to Zed`、`Click to open by running Zed CLI`、`Please restart Zed to Collaborate`、`Checking for Zed Updates`、`Move Zed to Applications`、`Zed was installed via a package manager`：全部 0 命中。
- 对照组 `Welcome to HiCodeer`、`Click to open the source location`、`Close Project`：全部命中。

语言包资源侧会进 image，但没有运行时出口：

- `assets/locales/zh-CN.json` 由 rust-embed 嵌入 `libcore.so`（DEFLATE 压缩，直接 `strings` 搜不到），文件名表里 `zh-CN.json`、`index.json`、`default.json` 均命中，已删除的 `ja-JP.json`、`fr-FR.json` 为 0。
- 该语言包里有 129 条 key 与 value 含 `Zed`（例如 `About Zed` → `关于 Zed`、`Click to open by running Zed CLI` → `点击即可通过 Zed CLI 打开`）。这些条目对应的代码都在非 OHOS 分支，OHOS 运行时不会请求，用户看不到。
- 按本项目既定口径「只删代码已被删除的 orphan 条目，代码仍在的即使没编译也不删」，这 129 条保留不动：它们是非 OHOS 平台的正常词条，删掉会导致那些平台缺词条。
