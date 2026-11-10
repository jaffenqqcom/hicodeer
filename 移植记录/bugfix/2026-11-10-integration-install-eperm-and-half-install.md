# 集成安装/卸载在 OHOS 上写入配置失败（codex / gemini / qwen / codebuddy）

## 问题描述

hicodeerd 的 `integration install` 与 `integration uninstall` 对 codex、gemini、qwen、codebuddy 四个 agent 直接失败，报 `Operation not permitted (os error 1)`。claude、kimi、atomcode 等其余 agent 正常。

更隐蔽的是另一种状态：脚本文件写成功了、配置里的注册条目却没有写进去，而 `integration status` 仍然报 `current`，面板因此不会重装——表现为「装好的好像不对，但工具说它是好的」。

## 问题表现

- 对四个 agent 执行卸载，每个只输出一行 `Operation not permitted (os error 1)`，没有 `removed ...` 行，配置未被改动。
- 对同一批执行安装，脚本落到磁盘，但 `hooks.json` / `settings.json` 里没有对应命令条目；agent 不会触发任何上报。
- `integration status` 仍显示 `current (vN)`，因为判定只看脚本里的版本号。
- 卸载后再安装、或面板周期的自动补装，都会重复上述过程。

## 问题原因

### 直接原因：删除多余 xattr 没有 OHOS 容错

写配置文件走 `herdr/src/integration/config_file.rs` 的 `Replacement`：先在目标同目录新建临时文件，写完整内容，再 rename 覆盖。写入前由 `herdr/src/platform/linux.rs` 的 `copy_config_xattrs` 复制访问控制属性：

1. 删除「临时文件有、原文件没有」的 xattr；
2. 补上「原文件有、临时文件没有」的 xattr。

第 2 步有 OHOS 门控：`fsetxattr` 返回 EPERM/ENOTSUP 时降级为 warn 继续。**第 1 步没有**，`fremovexattr` 失败直接返回错误。

hmdfs 会给新建文件自动打上 `security.isolate`，而部分 agent 的配置文档没有这个属性，于是必须删掉它——普通进程删这个属性返回 EPERM，整次写入失败。

实测对照：

- 成功组 `.claude/settings.json`、`.kimi-code/config.toml`、`.atomcode/hooks.json`：xattr 含 `security.isolate`
- 失败组 `.codex/hooks.json`、`.gemini/settings.json`、`.qwen/settings.json`、`.codebuddy/settings.json`：不含该属性

### 同类隐患：fchown 没有容错

`write_config_temporary` 为了让替换后的文件保持原属主，会对临时文件调 `fchown(fd, 原uid, 原gid)`。普通进程无法把文件交给另一个 uid，必然 EPERM。本机因为新文件继承目录属主（`20001006`）而恰好跳过，但换个目录布局或文件系统就会命中。

### 半装的成因：安装不是原子的

`install_codex`、`install_gemini`、`install_qwen`、`install_codebuddy`、`install_claude` 都是先 `fs::write` 脚本、再写配置。配置写失败时函数返回 Err，但脚本已经落地，留下「脚本在、注册没了」的状态；调用方 `refresh_integrations` 只打一条 warn，不会回滚。

### 为何无人发现：状态判定只看脚本

`herdr/src/integration/registry.rs` 的 `integration_status_at` 默认只读脚本里的 `HERDR_INTEGRATION_VERSION`。只有 grok、opencode、deveco 三个 target 额外校验配置（`grok_hook_config_is_valid` 等）。这四个 agent 的注册条目丢了也照样报 `current`，重装永远不会被触发。

## 解决方案

### 写入容错（`herdr/src/platform/linux.rs`，均用 `target_env = "ohos"` 门控）

- `fremovexattr` 返回 EPERM/ENOTSUP 时：记 `config xattr not removed` 的 warn 并继续。该属性只做文件隔离，保留它不影响可读性。
- `fchown` 返回 EPERM 时：记 `config file ownership not preserved` 的 warn 并继续，保留继承来的属主/组；权限仍按原文件设置，替换仍然是原子的。
- 非 OHOS 平台行为不变：同样的错误仍然返回。

### 安装顺序（`herdr/src/integration/targets.rs`）

五个 `install_*` 改为「先写配置、后落脚本」，并把 `remove_legacy_bash_hook_file` 一起挪到最后。配置写失败时不会再留下孤立的脚本。

### 状态判定（`herdr/src/integration/registry.rs`）

新增 `hook_config_registers_script`：按 target 找到对应的配置文档（codex 是同目录 `hooks.json`，其余是上一层目录的 `settings.json`），递归检查文档里是否出现该脚本的完整路径。脚本版本当前但配置里找不到时，报 `Outdated`，让面板触发重装。

### 版本对齐

`herdr/src/integration/assets/openclaw/hicodeer-agent-state.js` 的 `HERDR_INTEGRATION_VERSION` 由 1 改为 2，与 `OPENCLAW_INTEGRATION_VERSION` 一致；此前两者不一致导致 openclaw 永远 `outdated`，面板每 60 秒重装一次。

## 验证

- 实机往返：修复后的 daemon 对 codex、gemini、qwen、codebuddy 执行 `uninstall` + `install` 全部成功，不再出现 `Operation not permitted`。
- 配置保全：codebuddy 卸载后，用户的 `run-verify.sh` 与 `CODEBUDDY_PLUGIN_ROOT` 两处钩子仍在，13 条 hook 命令一条不少；codex、gemini、qwen 往返后与备份逐字节一致。
- 半装验证：把 `codex/hooks.json` 写成非法 JSON 再执行安装，报解析失败且脚本没有被写入；恢复配置后安装正常，状态回到 `current`。
- 新增单测 `codex_status_is_current_only_when_the_config_registers_the_hook`：配置为空时 `Outdated`，写入命令后回到 `Current`。
- 既有 `integration` 测试中 10 个失败与本改动无关：4 个因本机禁止 hard link，1 个因只读权限位不生效，其余为版本断言过期（codex 断言 13、实际 17；qwen 断言 7、实际 11；minimax 断言旧顺序）。
