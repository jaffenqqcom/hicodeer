# 终端路径/链接识别修复：目录识别、进程名误判与范围行号跳转

## 问题描述 (Problem Description)

HiCodeer 终端（移植自 warp-ohos 的 cell 级 hover 检测）在识别可点击路径时存在三个问题，均在 OHOS 真机上复现：普通 `ls` 输出只能识别带扩展名的文件、无法识别无扩展名的目录；`ps` 输出里的点分进程名被误识别成可打开目录，Ctrl+点击把文件管理器打开到当前工作目录；形如 `file.rs:195-234` 的范围链接点击后跳转行号错误（落到第 1 行而非起始行 195）。三者同属一条检测→解析链路，本轮一并修复。

## 问题表现 (Symptoms)

- `ls` 输出中 `AGENTS.md`、`Cargo.toml` 等带点文件名可识别，但 `docs`、`target`、`crates`、`hap` 等无扩展名目录完全无法识别为链接。
- `ps` 输出里 `lx.danmaku.bili`、`.xingin.xhs_hos`、`m.hicodeer.edit`、`hms.pushservice` 等点分进程名被识别为「可打开的目录」；Ctrl+点击后文件管理器统一打开到 `~/workspace/hicodeer`（终端 cwd）。
- 范围链接如 `crates/terminal/src/alacritty/hyperlinks.rs:195-234`：点击后跳到文件第 1 行；而单行的 `file.rs:195` 跳转正确。
- 复现条件：光标悬停在终端输出对应文本上，观察是否出现链接下划线/hand 光标；Ctrl+点击观察打开目标。
- 频率：稳定复现（前两个 100%，范围链接在带点/点分格式下 100%）。

## 问题原因 (Root Cause)

三个问题源自同一条链路的两层缺陷：**检测层形态判断** + **视图层文件系统解析**。

**问题一（目录识别不出，假阴性）**：检测层 `looks_like_path`（`crates/terminal/src/alacritty/hyperlinks.rs`）是鼠标探测热路径上的「值不值得查文件系统」形态初筛。其结尾硬性要求 `text.contains('/') || text.contains('\\') || text.contains('.')`（或纯数字）才放行。`ls` 输出的 `docs`、`target` 等裸目录名既无点也无斜杠，被这一行直接拒绝，根本到不了视图层的文件系统解析，即使它们真实存在。带点的文件名则通过——表现为「只认文件不认目录」。

**问题二（进程名误识别，假阳性）**：同一函数反向过松——任何含 `.` 的字符串都被当成路径候选。`ps` 的点分进程名含点，放行进入候选。视图层 `possibly_open_target`（`crates/terminal_view/src/terminal_path_like_target.rs`）用 `PathMatching::Heuristic` 解析，`resolve_open_target_internal`（`crates/workspace/src/path_link.rs`）会剥 `a/`/`b/`/`.` 前缀、在找不到精确匹配时对 worktree 做尾部扫描兜底。这套宽松兜底把不存在的点分词解析到当前工作目录 `hicodeer`（一个目录），于是走 `cx.reveal_path` 打开文件管理器到 hicodeer。促成条件之一是实测确认的 `Path::ends_with("") == true`：空 path 会命中工作区根目录本身。（注：点分词究竟经哪一分支落到 hicodeer 未逐分支实测，属推断；但「解析结果为 hicodeer 目录」与现场现象一致。）

**问题三（范围行号跳转错误）**：zed 的 `ROW_COL_CAPTURE_REGEX`（`crates/util/src/paths.rs`）只认 `:行`、`:行:列`、`(行,列)` 等格式，**没有** warp `LINE_AND_COLUMN_REGEX` 里的 `:(\d+)-(\d+)` 分支。`PathWithPosition::parse_str("file.rs:195-234")` 因尾部 `-234` 非纯数字而行号解析失败，把整串当路径、`row=None`，视图层跳转时落到第 1 行。单行 `:195` 能被正则命中，所以正确。

**code-review 中又发现一处**（问题三修复的残留场景）：范围折叠最初用原始文本 `file.rs:195-234,` 判断，尾部逗号/句号使 `end` 段 `234,` 非纯数字，折叠失效；且折叠候选排在范围串之后，视图层可能先命中 `row=None` 的范围串。

## 解决方案 (Solution)

核心思路：**「像不像路径」只做初筛，「是不是文件/目录、跳哪一行」全交给文件系统与专用解析裁决**；OHOS 专属行为一律用 `#[cfg(target_env = "ohos")]` 门控，不影响上游其他平台。

1. **裸目录名识别**：`looks_like_path` 在 OHOS 分支去掉「必须含 `.`/`/`」的硬要求，裸词也进候选，由视图层文件系统解析决定是否存在；非 OHOS 平台保留原严格判断（门控 + `[OHOS PORT BEGIN/END]`）。

2. **进程名误判拦截**：视图层新增 `candidate_names_directory`（OHOS 门控），对**目录**目标追加一道「候选末段名 == 解析结果末段名」校验。`lx.danmaku.bili` 解析出的目录名是 `hicodeer`，与候选不符 → 拒绝 reveal；文件目标不校验（符号链接/行号/git 前缀会合法改名）。

3. **范围行号**：检测层新增 `path_with_line_range_start`（OHOS 门控），把 `:start-end` 折叠为 `:start`（取起始行）。关键实现细节：先剥尾部标点得到 `base`，再基于 `base` 折叠，并把折叠候选排在范围串**之前**，保证第一个可解析候选就带正确 `row`。

4. **code-review 修复**：范围折叠改为基于去标点后的 `base` 文本；候选顺序调整为 `折叠起始行 → 去标点范围串 → 原始串`；补带尾部标点的端到端测试；`path_with_line_range_start` 收窄为私有函数。

为何选检测层而非改通用 `util::paths`：`ROW_COL_CAPTURE_REGEX` 是上游共享代码，改动它会影响所有调用方与平台；折叠逻辑本身是 warp 移植层专属行为，放在检测层并用 cfg 门控，风险最小。

配套测试：`looks_like_path` 裸词/散文各一组；`path_with_line_range_start` 折叠与非法格式；范围链接端到端（含尾部逗号/句号、并断言起始行候选排在范围串之前）；hover 测试初始 id 按 target 分支（OHOS 上裸词先被探测消耗 id 0）。

## 修改文件 (Modified Files)

- `crates/terminal/src/alacritty/hyperlinks.rs` — `looks_like_path` OHOS 分支放行裸词（含无扩展名目录），非 OHOS 保留严格形态判断；新增裸词/散文两组单测；cfg 门控 + `[OHOS PORT BEGIN/END]`。
- `crates/terminal/src/alacritty/hyperlinks_detection.rs` — 新增 `path_with_line_range_start` 将 `:start-end` 折叠为起始行；重构 `push_path_variants` 使折叠基于去标点 `base` 且折叠候选排在范围串之前；新增折叠单测与范围链接端到端测试（含尾部标点）；cfg 门控 + OHOS PORT 注释对。
- `crates/terminal/src/terminal.rs` — hover 测试初始 id 改为按 `target_env` 分支（OHOS 用 1、其他平台用 0），消除裸词先探测带来的 id 偏移；cfg 门控 + 注释对。
- `crates/terminal_view/src/terminal_path_like_target.rs` — 新增 `candidate_names_directory`（OHOS 门控），对目录目标做候选名一致性校验，拦截点分标识符被兜底解析成当前目录；悬停与打开两处接入；新增单测。

验证：`cargo test -p terminal` 相关测试全绿；`cargo check -p terminal`、`cargo check -p terminal_view --lib` 通过；清理现场后 `./script/bundle-ohos` 全量打包成功（`EXIT=0` + `HAP BUILD SUCCESSFUL`，签名验签通过）。两个环境相关测试失败（`test_no_pty_task_terminal_captures_output` 因本机未初始化 hicodeerd executor；`test_terminal_closes_after_nonzero_exit` 为并发下 `/usr/bin/zsh` 偶发 Permission denied，单独重跑通过）与本修复无关。

相关：[[ohos-debug-lessons]]
