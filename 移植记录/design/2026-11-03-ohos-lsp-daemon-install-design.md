# 数据根移入应用沙箱后，语言服务器改由命令守护进程安装与运行

- 日期：2026-11-03
- 提交：`c034334`（数据目录移动到沙箱后，解决lsp无法运行的问题），前置 `fa8c092`（`SHOW_HOME_PICKER = false`）
- 范围：`crates/lsp/src/ohos_lsp_install.rs`（新增）、`crates/lsp/src/lsp.rs`、`crates/lsp/Cargo.toml`、
  `crates/language/src/language.rs`、`crates/language_extension/src/extension_lsp_adapter.rs`、
  `crates/util/src/command/ohos.rs`、`crates/util/src/command.rs`、
  `crates/extension_suggest/src/extension_suggest.rs`、`crates/zed/src/main.rs`
- 结论：语言服务器的**存在性判定、安装、启动**三条链路全部改由 `hicodeerd` 一侧完成，
  应用侧不再往自己的数据目录里下载二进制；**扩展自带的**语言服务器在执行前换成系统安装的同名程序，
  系统没有就报与 `which` 失败完全相同的文案；另有一处 LSP 初始化参数调整，避开跨 uid 的父进程看门狗误判。
  桌面构建（`cfg(not(target_env = "ohos"))`）走原逻辑，行为不变。

## 1. 背景：数据根从用户目录搬到沙箱

`fa8c092` 把 `SHOW_HOME_PICKER` 置为 `false`（`hap/entry/src/main/ets/entryability/EntryAbility.ets:37`）：
ets 侧不再弹目录选择页，`prepareHomeDirectory` 直接加载主页面并放行原生模块
（`EntryAbility.ets:167`），因此 `app.home_directory()` 为空。

Rust 侧的连锁反应（`crates/gpui_ohos/depend/launch-zed/src/launch_app.rs`）：

- `start_zed_main(base_path, home_directory)` 拿到 `None`，不调用 `paths::set_custom_data_dir`
  （`launch_app.rs:82` 起），数据根退回平台默认；
- `ensure_shell_env` 在无 home 时把 `HOME` 设为 `base_path`，即 el2 沙箱的 `files/` 目录
  （`launch_app.rs:160` 附近）；
- `paths::data_dir()` 在非 macOS/Windows 下取 `dirs::data_local_dir()/hicodeer`
  （`crates/paths/src/paths.rs:144`），于是整个数据根落在
  `/data/storage/el2/base/haps/entry/files` 之内（下文称**沙箱数据根**）。

问题就出在这里：**沙箱数据根只属于应用自己的 uid**。上游把语言服务器下载进
`<数据根>/languages/<server>-<version>/`，再由 `util::command` 启动；而 OHOS 上命令统一交给
`hicodeerd`（uid 20020117）执行，它和应用（uid 20020220）不在同一个账号、也不共享同一个数据视图，
沙箱内那条绝对路径在它那里没有意义。语言服务器于是「装了却起不来」。

## 2. 原设计的三处独立失效点

改的是三件事，任何一处漏掉都会继续失败：

1. **安装位置**：二进制装进沙箱 → 守护进程执行不了。
2. **存在性判定**：上游先扫本地 PATH 与容器目录。本地扫到沙箱内的副本会误判「已安装」；
   而真正能用的副本（守护进程 PATH 里的那一个）本地的扫描看不见。
3. **进程协议**：`InitializeParams.process_id` 上报应用自己的 pid
   （`crates/lsp/src/lsp.rs:890`）。`vscode-languageserver` 一类的服务端会周期性
   `process.kill(pid, 0)` 做父进程看门狗；跨 uid 下这个调用返回 `EPERM`，服务端把它读成
   「编辑器已退出」，于是首次 tick 后静默退出——表现是语言服务连上了又立刻断开。

## 3. 设计原则：守护进程的 PATH 是唯一真源

一旦承认「程序实际在哪个账号下跑，就以那个账号的 PATH 为准」，三件事就收敛成一句话：

- **存在性**：问守护进程 `which`，本地扫描只作快速通道；
- **安装**：用守护进程能跑的 `npm --global` 装，落在守护进程 PATH 覆盖的目录里；
- **下发**：交给守护进程的程序路径如果是应用私有视图内的，就退化成**程序名**，
  让它按自己的 PATH 重新解析。

这样做还有一个附带好处：`which` 成功即代表「能启动」，不再有「装上了但路径无效」的中间态。

## 4. 实现分解

### 4.1 存在性判定：`which` 的 resolver 兜底

`patches/which/src/finder.rs:122` 起，`Finder::find` 在 OHOS 上把本地扫描结果收集成 `Vec`；
**只有本地一个都没找到时**，才调用 `crate::hook::intercept(name)` 向守护进程求值。
本地扫描保持权威，因此已经能解析的程序行为完全不变。

- hook 注册点是 `crates/util/src/command/ohos.rs:268` 的 `install_remote_which_resolver`，
  在 `init()` 里与 executor 一起装好，进程内只生效一次；
- resolver 内部走 `executor.resolve_program(name)`，失败或超时一律当作「那边没有」，不向上抛错；
- 查询本身不能在 tokio worker 上跑（`block_on` 会 panic），因此由专用线程
  `ResolveQueryWorker` 承接（`crates/gpui_ohos/depend/cmd-agent/cmd-client/src/executor.rs:527`）；
- hook 带线程内深度计数（`patches/which/src/hook.rs`），resolver 自己再查一次时不会无限递归。

各 adapter 的 `check_if_user_installed` 本来就是 `delegate.which(...)`（例如
`crates/languages/src/json.rs:167` 查 `vscode-json-language-server`），所以这一层不需要改 adapter，
改 `which` 一处即可全部受益。

### 4.2 安装：`crates/lsp/src/ohos_lsp_install.rs`

新模块只有一件事：给定 adapter 名，确保对应的 npm 包被**全局**装好。

- **表驱动**：`npm_package_for`（`ohos_lsp_install.rs:71`）按 adapter 的 `name()` 查包名。
  键必须是 adapter 名而不是可执行文件名——JSON 服务器的 adapter 名是 `json-language-server`，
  真正的可执行文件叫 `vscode-json-language-server`。表键错位的后果是「安装成功但仍查不到」。
- **两个服务器共用一个包**：CSS 与 JSON 都来自 `vscode-langservers-extracted`。
  因此引入进程级「已完成安装」缓存 `COMPLETED_INSTALLS`（`:45`）：第二个请求在锁上等到时，
  包已经装好，直接返回，不再重跑 npm。
- **串行化**：`NPM_INSTALL_LOCK`（`:36`）保证同一时刻只有一次 `npm install`，
  否则并发写同一个 prefix 会互相破坏。
- **超时与可中断**：`INSTALL_TIMEOUT = 120s`（`:33`），`run_with_deadline` 用
  `futures::future::select` 把计时器与 `command.output()` 并置，且**计时器先 poll**——
  `output()` 里有同步的连接交接（可长达连接池的重连预算），若让它在前面跑，
  这段时间不会计入超时。`kill_on_drop(true)` 保证超时后进程被回收；
  注释里明确：被打断的安装由下一次尝试修复（npm 本身可重入）。
- **四态结果**：`InstallOutcome`（`:52`）区分 `Installed` / `DaemonUnavailable` / `NoPackage` / `Failed`。
  调用方要据此给出不同文案：守护进程没起和这个服务器本来就没有包，修法完全不同。
  先做一次 `/bin/sh -c true` 探活（`:91` 起），避免把「守护进程挂了」误报成「没装」。

### 4.3 决策点：`crates/language/src/language.rs` 的 OHOS 早返回

`get_language_server_command` 的缓存分支里插入一段 `#[cfg(target_env = "ohos")]` 块
（`crates/language/src/language.rs:840` 起），并在块内直接 `return`：

- 顺序是「先 `which`（经 4.1 落到守护进程），没有再装」；
- **忽略 `allow_path_lookup`**：OHOS 上不存在「Zed 自装副本」与「用户 PATH 副本」的二分，
  两边都是同一个守护进程 PATH，这个开关没有可分辨的语义；
- **尊重 `allow_binary_download`**：安装就是下载，用户关掉下载就不能替他装，直接返回
  `downloading language servers disabled`；
- 安装放到 `cx.background_executor()` 上跑（`:863`）。守护进程连接交接是同步的，
  在 UI 线程上做会把窗口卡住数十秒；
- 状态提示：安装前 `BinaryStatus::Downloading`，结束后复位 `None`；
- 安装成功后再查一次 `which`。若仍查不到，说明 npm 的全局 prefix 不在守护进程 PATH 上，
  打一条 warn 与「从未安装」区分开（`:876` 起）——这是本设计唯一需要人工核查的失败形态。

### 4.4 路径下发：`daemon_program_name` 增加沙箱前缀

`crates/util/src/command/ohos.rs:540` 起，原先只把 `/data/app/bin`（HNP 私有挂载）下的路径
退化成 basename；本次把应用数据目录也纳入：

```rust
let absent_from_daemon_view = /* /data/app/bin 下 */ || path.starts_with(APP_DATA_DIR);
```

`APP_DATA_DIR = "/data/storage/el2/base/haps/entry/files"`（`ohos.rs:83`）。

含义是：这条路径只在应用自己的视图里成立，交给守护进程必然 `ENOENT`，因此只传程序名，
由守护进程按自己的 PATH 解析。附带效果是「扩展随包附带的工具」可以被系统安装的同名工具顶替——
这正是沙箱数据根场景需要的语义。

### 4.5 协议：不上报 pid

`crates/lsp/src/lsp.rs:890` 起：

```rust
process_id: if cfg!(target_env = "ohos") { None } else { Some(std::process::id()) },
```

`None` 让服务端跳过父进程看门狗；进程生命周期改由**通道关闭**兜底：应用侧的 stdin 断开后，
守护进程会回收子进程（命令通路本就带 `kill_on_drop` 与会话回收）。这条比「上报一个对方杀不动的 pid」
更干净——后者会让服务端每次检查都拿到 `EPERM`，属于必然误判。

### 4.6 同一提交里的两处附带改动

- `crates/extension_suggest/src/extension_suggest.rs:282`：补 `ets → arkts` 的后缀映射。
  静态扩展注册表里有 `arkts`，上游的后缀表没有，导致打开 `.ets` 时不提示安装。
- `crates/zed/src/main.rs:476`：外部打开请求强制 `open_behavior = Add`。OHOS 只有单窗口，
  上游默认的 `ExistingWindow` 在路径不属于任何已开工作区时会新建窗口，而 `open_window` 会 `bail!`。

两者与本主题无关，只是同批提交，记录在此以免读者误以为属于 LSP 链路。

### 4.7 扩展提供的语言服务器：执行前换成系统程序

`ExtensionLspAdapter::get_language_server_command`
（`crates/language_extension/src/extension_lsp_adapter.rs:168`）是**独立实现**，不经过 4.3 的通用分支：
它用 `path_from_extension`（`:222`）把扩展 wasm 返回的命令拼成扩展目录内的绝对路径，
必然落在沙箱里。

两个事实叠在一起，决定了改法：

- 这条路径守护进程执行不了；
- 远端 spawn 是**异步发射**：`executor.spawn` 分配连接、建 socketpair、起 pump 后立即返回
  `Ok(RemoteChild)`（`crates/gpui_ohos/depend/cmd-agent/cmd-client/src/executor.rs:132`），
  真正的 exec 失败不会回到调用方的 `Err` 通道。所以「失败时再换文案」走不通，
  只能在执行前判定。

做法（整段 `#[cfg(target_env = "ohos")]` 门控）：

1. `util::command::is_app_private_path`（`crates/util/src/command/ohos.rs:90`）判定路径是否属于
   本进程的私有视图：el2 数据根前缀，或父目录为 `/data/app/bin`。该判据与
   `daemon_program_name` 共用，口径只有一份。
2. 命中则用 `delegate.which(文件名)` 查系统程序。这与内置 adapter 是同一条查询
   （`crates/project/src/lsp_store.rs:16567` 的 `which::which_in` → OHOS resolver → 守护进程），
   所以不新增任何依赖或查询接口。
3. 查回来的路径**若仍是私有路径**，同样视为没查到。原因：`delegate.which` 是
   「本地 PATH 扫描优先、resolver 只补空」，而进程 PATH 含沙箱 `files/` 根，
   沙箱根下真有同名文件时会被本地扫描命中——那份同样执行不了。
4. 未命中 → `lsp::language_server_not_installed`（`crates/lsp/src/lsp.rs:111`），
   与 4.3 分支共用同一个函数，文案只此一份；4.3 原来的内联字符串已改成调用它
   （`crates/language/src/language.rs:914`）。

代价：这次 `which` 在发起语言服务的线程上同步执行（resolver 内部阻塞），与既有的
`delegate.which` 一致，不引入新的阻塞形态；而且只有路径命中私有视图时才会查。

## 5. 启动时序

```
用户打开文件 / 触发语言服务
  │
  ├─ 扩展提供的服务器 ──▶ ExtensionLspAdapter::get_language_server_command
  │                          │ path_from_extension() → 沙箱内绝对路径
  │                          ▼
  │                        is_app_private_path? ── 否 ──▶ 原样使用
  │                          │ 是
  │                          ▼
  │                        delegate.which(文件名)
  │                          ├─ 命中且不在私有视图 ──▶ 用系统安装的那份
  │                          └─ 否则 ──▶ 「未安装」（与内置同一句）
  ▼
adapter.check_if_user_installed() ──▶ delegate.which("vscode-json-language-server")
  │                                        │
  │                                        ├─ 本地 PATH 扫到？ ── 是 ──▶ 用本地结果
  │                                        └─ 否 ──▶ which resolver ──▶ 守护进程 which
  │                                                                     └ 有则返回其路径
  ▼ 仍未解析
allow_binary_download? ── 否 ──▶ 报「下载被禁用」
  │ 是
  ▼
background_executor: ohos_lsp_install::ensure_installed(adapter 名)
  │  探活 /bin/sh ──▶ 失败：DaemonUnavailable（提示先起 hicodeerd）
  │  查表 ──▶ 无包：NoPackage（提示自行安装）
  ▼
npm install --global <package>（120s 上限，串行，完成后记账）
  │
  ▼
再次 which ──▶ 得到守护进程侧绝对路径 ──▶ LanguageServerBinary
  │
  ▼
lsp::LanguageServer::new ──▶ util::command::new_command(binary.path)
  │  路径在沙箱内 → 退化为程序名，由守护进程按自己的 PATH 解析
  ▼
守护进程 spawn；InitializeParams.process_id = None（跳过看门狗）
```

## 6. 覆盖范围

自动安装（纯 JavaScript，npm 包在守护进程上可用）：

- `vtsls` → `@vtsls/language-server`
- `json-language-server`、`vscode-css-language-server` → `vscode-langservers-extracted`
- `yaml-language-server` → `yaml-language-server`
- `bash-language-server` → `bash-language-server`
- `pyright` → `pyright`
- `basedpyright` → `basedpyright`
- `tailwindcss-language-server` → `@tailwindcss/language-server`

刻意不覆盖：

- **原生二进制**：`clangd`、`gopls`、`ty`、`ruff` 等没有 OHOS 构建，装了也跑不了，
  留在表外，让用户看到「未安装」而不是等一次注定失败的安装。
- **不走 `which` 的 adapter**：`typescript`、`eslint` 用的是各自的内置定位逻辑，
  安装无法让它们启动，同样不入表。

上面两条由单测钉住：`native_servers_have_no_package`、`table_is_keyed_by_adapter_name_not_executable_name`
（`ohos_lsp_install.rs:221`、`:249`），以及共包用例 `css_and_json_share_one_package`（`:238`）。

**扩展提供的服务器**不走这张 npm 表：它们的二进制由扩展自己下载或随包携带，一律落在扩展目录里。
这类服务器由 4.7 处理——执行前按文件名换成系统安装的那份，系统没有就报「未安装」。
扩展 wasm 里那套「按平台下载 release」的逻辑因此只影响它自己目录里存不存在文件，
不影响最终启动谁。

## 7. 失败形态与边界

- **守护进程未启动**：返回 `DaemonUnavailable`，提示从系统命令行终端启动 `hicodeerd` 后重启语言服务。
  注意 `hicodeerd` 不随 HAP 覆盖安装重启（HNP 服务进程），换版本需重启设备。
- **npm 全局 prefix 不在守护进程 PATH**：安装成功但第二次 `which` 仍为空，打 warn
  `installed but not resolvable on the daemon PATH`。这是设计上唯一的隐性失败点，
  已在 2026-11-03 的 release 包里实测走通（见第 8 节）：当前设备的 npm 全局 prefix 落在
  守护进程 PATH 覆盖范围内。换设备或换 node 分发源（harmonybrew / hqzing 的 prefix 不同）时
  仍需按这条 warn 复查。
- **项目工作目录在沙箱内**：`LanguageServer::new` 会把 `root_path` 当工作目录传给子进程
  （`crates/lsp/src/lsp.rs:543` 附近），守护进程若不能 `chdir` 到沙箱路径，启动仍会失败。
  本设计没有处理这一情形：默认预期是用户打开的是沙箱外、双方都能到达的项目目录。**未验证**。
- **版本策略**：`npm install --global` 装的是当时 registry 上的 `latest`，之后不再检查更新。
  升级由用户决定，不在每次启动时比对——这是刻意的取舍，避免每次启动都联网。
- **并发**：同一包只装一次；不同包串行。安装失败不留半成品判断，下次启动重试即可。
- **扩展服务器的参数不重写**：4.7 只替换程序路径，`arguments` / `env` 原样透传。
  扩展若把服务器做成「系统 node + 扩展内 `.js`」，或把 `NODE_PATH` 指向扩展目录，
  替换程序名之后仍然跑不起来；这类形态本轮不处理。**未验证**（当前只用原生二进制
  `taplo` 验证过）。
- **守护进程不可达与未安装同文案**：4.7 的查询失败一律报「未安装」，与 `which` 失败一致
  （按需求刻意为之）。区分两者要看日志：resolver 侧有 `command: which query failed` 一类的记录，
  而真正的「没装」不会产生它。内置分支（4.3）仍保留独立的 `hicodeerd is not running` 提示。
- **可达性表述的精度**：`ohos_lsp_install` 的模块注释写的是守护进程「cannot reach the
  application's files」，而 `hicodeerd` 自己的 agent socket 就放在同一目录下
  （`crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs:544`，注释称该目录是双方都能到达的共享目录）。
  两处不矛盾：目录可以被双方打开（socket、普通文件），但**沙箱内的可执行文件不能指望由另一个 uid 跑起来**。
  精确到「目录可达但不可执行」这一点，本轮未做逐项实测，按设计取舍回避而不依赖它。

## 8. 验证方式

单测（`cargo test -p lsp ohos_lsp_install`，四个用例）覆盖表结构、共包关系、键名口径。

**实测结论（2026-11-03）**：release HAP 安装到设备后语言服务可正常启动并使用。
即「`npm install --global` 的落点在守护进程 PATH 上」与「`process_id = None` 后服务不再秒退」
两条主链成立；第 4.3 节末尾那条 warn 未触发。具体触发的是哪个服务器、`which` 解析出的绝对路径
与 npm prefix 的实际取值未留档，需要时按下面的日志关键字回溯。

**扩展链路的实测（2026-11-03，toml / taplo）**：用 `toml` 扩展验证 4.7。该扩展声明
`[language_servers.taplo]`，wasm 会把 taplo 下载到扩展目录（沙箱内），正好是目标形态。
本轮实测确认「扩展服务器不再启动即失」；具体命中「换成系统程序」还是「报未安装」哪条分支
未留档，复查时按判据区分：换成系统程序后日志里 `starting language server process. binary path:`
显示的是守护进程可见的路径；报未安装则直接出现第 8 节那句文案。

设备侧手工判据：

- 首次打开 `.json` / `.ts` 文件：日志出现
  `ohos_lsp_install: installing <package> for <server>`，随后出现
  `language server for <server>: path: <守护进程侧绝对路径>`；
- 已安装后再次打开：不再有 installing 日志，启动路径直接命中；
- `hicodeerd` 停止时打开文件：提示语是「hicodeerd is not running」，而不是「未安装」；
- 语言服务不再「连上即断」：以前的表现是 `process_id` 看门狗误判，日志里能看到服务端进程秒退；
  现在应能持续提供补全/诊断。

## 9. 改动清单

- `crates/lsp/src/ohos_lsp_install.rs` — 新增，安装器 + 四个单测
- `crates/lsp/src/lsp.rs` — 挂载 OHOS 模块；`InitializeParams.process_id` 置 `None`；
  新增 `language_server_not_installed`（文案单一来源）
- `crates/lsp/Cargo.toml` — `async-io` 只在 `cfg(target_env = "ohos")` 下引入
- `crates/language/src/language.rs` — OHOS 早返回：`which` → 安装 → 再 `which`；
  「未安装」改调上面的函数
- `crates/util/src/command/ohos.rs` — 新增 `is_app_private_path`（私有视图判定），
  `daemon_program_name` 复用它
- `crates/util/src/command.rs` — 按 OHOS 条件 re-export `is_app_private_path`
- `crates/language_extension/src/extension_lsp_adapter.rs` — 4.7：扩展服务器在执行前换成
  系统安装的同名程序，查不到报同一句未安装
- `crates/extension_suggest/src/extension_suggest.rs` — `.ets → arkts`（附带）
- `crates/zed/src/main.rs` — 外部打开强制 `Add`（附带）

> 行号为 2026-11-03 当轮核对结果。行号易碎，引用前请重核；核不动时以符号名/路径为准。
