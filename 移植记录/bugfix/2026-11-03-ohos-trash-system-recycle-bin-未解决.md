# OHOS 删除文件到回收站未对接系统回收站（未解决）

## 问题描述 (Problem Description)

HiCodeer 的"移到垃圾桶"（Project Panel 右键 Trash）在 HarmonyOS 上没有对接系统回收站，而是走 `trash` crate 的 freedesktop 实现，把文件搬进**项目文件所在挂载点根**的 `.Trash-<uid>` 目录。该目录既不在系统回收站里（文件管理器「最近删除」看不到），也不会被系统清理；同时它的创建依赖 app 对挂载点根目录（如 `/storage/Users/currentUser`）有写权限，而 app 默认只被授予用户打开过的项目目录。目标是改为对接 OHOS 系统回收站，但受"三方无恢复 API"与"沙箱回收站需跨分区复制"两条硬约束阻塞，方案尚未定稿。

## 问题表现 (Symptoms)

- 删除项目内文件后，文件进入 `/storage/Users/currentUser/.Trash-20020243/{files,info}`，系统文件管理器的「最近删除」中不可见。
- 该 `.Trash-<uid>` 目录只增不减：系统只清理它自己的 `/storage/Users/currentUser/.Trash`（属主 `file_manager`），不认 `.Trash-<uid>`。
- app 内 Ctrl-Z（Project Panel 的 Undo）只在本次进程内可用；重启后 undo 栈与 `TrashId` 表都消失，文件无法从 UI 恢复。
- 历史残留：`/storage/Users/currentUser/.local/share/Trash` 占 12G（2026-08-19 至 09-17），无任何清理机制。
- 默认权限下的删除存在失败风险：若 app 未被授予 `/storage/Users/currentUser` 的写权限，无法在挂载点根创建 `.Trash-<uid>`，`trash()` 直接返回错误。

复现步骤：

1. 打开一个位于 `/storage/Users/currentUser/workspace/...` 的项目；
2. 在 Project Panel 中右键某个文件 → Trash；
3. 观察文件出现在 `<挂载点根>/.Trash-<app uid>/files/`，`.trashinfo` 记录原路径。

## 问题原因 (Root Cause)

### 1. 调用链与目录拼接规则

```
crates/fs/src/fs.rs:926  RealFs::trash()
  └─ std::path::absolute(path)（故意不解析 symlink）
  └─ trash::delete_with_info(path)            [fs.rs:935]
       └─ OHOS 命中 freedesktop 分支          [trash-rs src/lib.rs:48]
            ├─ home_trash   = $XDG_DATA_HOME/Trash 或 $HOME/.local/share/Trash
            ├─ topdir(file) = get_first_topdir_containing_path(path)   [freedesktop.rs:51]
            ├─ topdir == home_topdir ?
            │    是 → move_to_trash(path, home_trash)                  [freedesktop.rs:53-55]
            └─   否 → execute_on_mounted_trash_folders(uid, topdir)    [freedesktop.rs:72-76]
                       ├─ topdir/.Trash/<uid>（要求 .Trash 带 sticky 位）
                       └─ topdir/.Trash-<uid>（不存在则 create_dir）    [freedesktop.rs:441-474]
```

关键点：`.Trash-<uid>` 的前缀是**被删文件所在挂载点根**，不是 `$HOME`。`$HOME` 只参与"是否走 home trash"这一次相等判断。

### 2. 本机的实际取值

| 项 | 值 | 依据 |
|---|---|---|
| app 的 HOME | 沙箱 `/data/storage/el2/base/haps/entry/files` | `SHOW_HOME_PICKER=false`（`EntryAbility.ets:37`）+ `launch_app.rs:164-168`；用户实测确认 |
| 文件所在挂载点 | `/storage/Users/currentUser` | `/proc/mounts`：`/data/service/el2/100/hmdfs/account /storage/Users/currentUser hmdfs rw` |
| uid | `20020243` | 目录名 `.Trash-20020243` 即 `libc::getuid()` |

因为 `topdir(HOME)=/data` ≠ `topdir(file)=/storage/Users/currentUser`，走 else 分支，得到：

```
/storage/Users/currentUser/.Trash-20020243/{files,info}
```

实测样本：`testdekete`，`info/testdekete.trashinfo` 中 `Path=/storage/Users/currentUser/workspace/hicodeer/testdekete`，`DeletionDate=2026-11-03T19:38:24`。

### 3. 历史残留的成因

`/storage/Users/currentUser/.local/share/Trash` 产生于 2026-08-19～09-17 的旧版本：当时代码中没有任何 `set_var("HOME")`（`git grep` 于 `275f878^` 为空），HOME 继承平台默认 `/storage/Users/currentUser`，与文件同挂载点 → 走 home trash。10-23 `9e43326` 关闭 picker 后 HOME 改为沙箱，落点随之变成 `.Trash-<uid>`。

### 4. 为什么不能简单地改用系统回收站

OHOS 三方接口 `fileManagerService.deleteToTrash(uri)`（`@kit.FileManagerServiceKit`，API 17+，2in1 支持，无需声明权限）可以把文件移进系统回收站，返回 `trashUri`。但恢复接口 `@ohos.file.trash.recover/listFile` 是**系统 API**（仅文件管理器可调），三方无法：

- 列出回收站内容；
- 把回收站里的文件恢复到原路径。

因此"系统回收站"与"app 内 undo（Ctrl-Z）"无法同时成立。系统回收站中的文件也不在 app 的授权范围内，app 读不到。

## 解决方案 (Solution)

**当前状态：未解决，方案未定稿。本次只完成根因定位与方案评估，未修改代码。**

### 已评估的方案

| 方案 | 额外授权 | 跨分区 | app 内 undo | 系统可见/可恢复 | 自动清理 | 结论 |
|---|---|---|---|---|---|---|
| 挂载点根 `.Trash-<uid>`（现状） | **需要挂载点根写权限** | 否（rename） | 是 | 否 | 否 | 默认不可靠 |
| 系统回收站 `deleteToTrash` | 不需要 | 系统内部处理 | 否 | 是 | 系统策略清 | 语义正确，但丢 undo |
| 沙箱回收站（files/cache） | 不需要 | **是（复制+删）** | 是 | 否 | cache 超配额/空间紧张时 | 大文件删除太慢，已否决 |
| 项目内隐藏目录 | 不需要（项目已授权） | 否（rename） | 是 | 否 | 需自己做 | 有污染项目/散落/删项目根无解等副作用 |

### 死路与已排除项

- **沙箱回收站**：跨挂载点无法 `rename`，只能复制后删除；删除大量文件时慢且峰值占双倍空间。用户明确否决。
- **cache 目录自动清理**：官方语义是"超过配额或系统空间达到一定条件才触发"，并非定时清理，空间充足时会长期堆积；且清理不通知应用，undo 可能静默失效。
- **通过 `XDG_DATA_HOME` 把 home trash 指向别处**：`XDG_DATA_HOME` 同时被 `crates/paths` 用于推导 HiCodeer 数据目录，会把 db/logs/extensions 一并搬家，不可接受。
- **trash crate 配置回收站目录**：`TrashContext` 只有不可配置的 `platform_specific` 字段（`trash-rs src/lib.rs:61-71`），没有此能力，只能自建。
- **项目内每项目一个回收站**：技术可行（同分区 rename），但会污染项目树与 `git status`、位置散落、无法统一清理，且删除项目根目录时无"项目内"位置可放，仍需兜底。未采纳为主方案。

### 待决策

1. 接受"进系统回收站、放弃 app 内 undo"，恢复交给系统文件管理器；
2. 自建沙箱回收站并自设容量上限 + 淘汰策略，保住 undo，承担跨分区复制成本；
3. 折中：系统回收站为主，沙箱仅作为 `deleteToTrash` 失败时的兜底（仍会在失败场景下复制大文件）。

### 若继续推进，需改动的落点（尚未实施）

- `openharmony-ability-zed` 新增插件（Rust crate + ArkTS 模块），ArkTS 侧调用 `fileManagerService.deleteToTrash`；
- `hap/entry/oh-package.json5` 与 `EntryAbility.ets` 注册插件；
- `gpui_ohos/Cargo.toml` 加依赖、`crates/gpui_ohos/src/ohos/platform.rs` 的 `register_plugins` 注册；
- `crates/fs/src/fs.rs` 的 `trash()` / `restore()` 增加 `#[cfg(target_env = "ohos")]` 分支，失败时回退现有 freedesktop 实现。

## 修改文件 (Modified Files)

无。本次仅为调研与一次删除功能验证，未修改任何代码或配置。

相关待改文件（尚未改动）：

- `crates/fs/src/fs.rs` —— `trash()`（:926）与 `restore()`（:1418）需要 OHOS 分支
- `crates/gpui_ohos/src/ohos/platform.rs` —— 插件注册
- `crates/gpui_ohos/Cargo.toml` —— 新插件依赖
- `hap/entry/oh-package.json5`、`hap/entry/src/main/ets/entryability/EntryAbility.ets` —— ArkTS 插件接线

[[ohos-debug-lessons]]
