# git 面板的“信任目录”提示点击后不消失，只有重启应用才恢复

## 问题描述

当仓库的 `.git` 目录属主与执行 git 的进程 euid 不一致时，git 会拒绝访问并报 “detected dubious ownership”。HiCodeer 的 git 面板据此显示一条 “Detected dubious ownership” 提示，并提供 “Trust Directory” 按钮。

在本机上，点这个按钮没有效果：提示一直在，git 内容也一直空着。只有把整个 HiCodeer 应用重启，提示才消失、内容才正常出现，而重启后按钮再次点击也看不出任何作用（此时已经没有提示可点）。

## 问题表现

- git 面板顶部常驻 “Detected dubious ownership” 提示条，点 “Trust Directory” 后不消失。
- 面板里看不到文件变更、分支等信息。
- 退出并重新打开应用后，同一条提示消失，git 内容立刻正常。
- `~/.gitconfig` 的 `[safe]` 段里，同一个仓库路径被重复写入了 68 条 `safe.directory`，说明按钮每次点击都真的写成功了，但判定始终不认。
- 清空这 68 条之后，当前仓库打开依然是好的（因为刚重启过），无法再触发提示，回归只能等下一次现场出现。

## 问题原因

根因不在 git 的检查本身，而在“一次失败的仓库初始化被永久缓存”，加上 OHOS 上缺少触发重试的那条事件。

1. 首次打开仓库时，`safe.directory` 还没写，git 因属主不匹配拒绝工作，`LocalRepositoryState::new` 失败（`crates/project/src/git_store.rs:10290` 的 `state.await.log_err()` 取不到状态）。
2. 该失败直接让 git worker 退出。worker 一退出，它持有的 `job_sender` 永久 closed，之后所有 git 作业的接收端都会被取消。
3. 用户点 “Trust Directory”，`git config --global --add safe.directory <path>` 确实写成功——这就是配置里堆了 68 条的原因。
4. 但仓库的失败状态是内存里缓存的（`repository_state` 是 `Shared<Task<..>>`），不重建就永远是失败。面板再问一次 access，请求被投进已经关闭的 `job_sender`，oneshot 接收端丢弃，得到 `Err(Canceled)`；上游把取消一律当成不安全（`crates/git_ui/src/git_panel.rs:5094` 附近的注释自己写明 “We assume `GitAccess::No` on cancellation”），于是提示继续显示。
5. 上游本来有自愈路径：`crates/project/src/git_store.rs:908` 起监听 `.gitconfig` 与 `git/config`，文件一变就对 `job_sender.is_closed()` 的仓库调 `respawn_local_worker` 重建 backend，然后 `emit(GlobalConfigurationUpdated)`。**但这条路径依赖文件监听**。OHOS 上应用 HOME 与 git 执行环境的 HOME 不是同一个，`.gitconfig` 的 watch 永远不触发，这段自愈代码等于不存在。
6. 重启应用之所以有效：进程重启后 Repository 对象重建，`LocalRepositoryState::new` 重新执行，此时配置里已经有了 `safe.directory`，第一次就成功，worker 活着，access 自然为 Yes。

所以现象看起来像“git 信任不生效”，实际是“失败被记住，且没有人去重建那条连接”。

## 排查过程中被排除的假设

- **“git 自己不认这个 safe.directory”**：不成立。在本机以 uid 20020117（与 daemon 相同）直接执行 `git status` / `git rev-parse`，即使把 global config 清空、HOME 指到空目录、关掉 system config，也不报 dubious。可见触发条件只在应用那条执行链路上。
- **“配置写到了错误的文件”**：不成立。全盘只有一份 `.gitconfig`（`/storage/Users/currentUser/.gitconfig`），`/etc/gitconfig`、`~/.config/git/config`、沙箱内 `git/config` 都不存在，仓库本地 `.git/config` 也没有 safe 条目。
- **“68 条重复条目导致解析失效”**：不成立。清空后行为不变，重复只是“每次都写成功、每次判定都失败”留下的痕迹。
- **“需要给每次 git 调用注入 `-c safe.directory=*`”**：这是另一条可行路线（herdr 就是这么做的，`herdr/src/worktree.rs:164`），但从源头关闭这项检查，等于对所有仓库关掉 git 的保护，代价比补一次重建更大，未采用。

## 解决方案

把上游那条“配置变了就重建 worker”的逻辑，从文件监听里解出来，变成一个可显式调用的方法，并在写入 `safe.directory` 之后立刻调用它。

`crates/project/src/git_store.rs` 新增 `respawn_closed_local_repositories`（OHOS 门控）：

- 遍历 `self.repositories`，挑出 `job_sender.is_closed()` 的仓库；
- 对每个这样的仓库，用与上游相同的参数调 `respawn_local_worker`，再 `schedule_scan`；
- 结束时 `emit(GitStoreEvent::GlobalConfigurationUpdated)`，让订阅者（git 面板）重算 access 并刷新。

`crates/git_ui/src/git_panel.rs` 把信任按钮原来那段内联 OHOS 补丁收敛成一个专用方法 `recheck_git_access_after_safe_directory_write`，按顺序做三件事：

- 等 `git config` 写完 → 重建已关闭的 git 连接；
- 重新问一次 access；
- 把新结论写回面板（`git_access = Some(..)`）并触发重绘。

按钮处只留一行调用，非 OHOS 平台仍走原来的 `config_task.detach_and_log_err(cx)`，行为不变。

关键点在于：**不是去问 git “现在还信任我吗”，而是先把那条已经死掉的连接换掉，再去问。** 拿坏连接去问，多少次都是坏的。

## 修改文件

- `crates/project/src/git_store.rs` — 新增 `respawn_closed_local_repositories`，把上游 `.gitconfig` 监听里的重建逻辑暴露成可显式调用的入口，并补发 `GlobalConfigurationUpdated`。整体 `#[cfg(target_env = "ohos")]` 门控。
- `crates/git_ui/src/git_panel.rs` — 信任按钮后的处理收敛为 `recheck_git_access_after_safe_directory_write`：重建连接 → 重查 access → 刷新面板；异常分支补 `error` / `warn` 日志（正常路径不打日志）。

## 验证

- `cargo check -p git_ui`：通过，仅 3 个既有 warning（`git_commit_editor_style` 的 unreachable statement 与未使用参数，与本次改动无关）。
- `./script/bundle-ohos`：`HAP BUILD SUCCESSFUL`，产物 `hap/entry/build/default/outputs/default/entry-default-signed.hap`。
- 现场回归判据：再次出现 “Detected dubious ownership” 时，点一次 “Trust Directory”，提示应**当场消失**、git 内容直接出现，不需要重启。
- 本轮未能现场复现：清空 68 条重复条目后当前仓库打开正常（刚重启过），提示不再出现，触发条件需要等下一次真实出现。已在修改中按上游同一套重建调用对齐，行为等价。

相关：[[ohos-debug-lessons]]
