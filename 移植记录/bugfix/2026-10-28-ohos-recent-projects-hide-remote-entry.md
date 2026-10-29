# OHOS 最近项目弹层隐藏「打开远程文件夹」入口

## 问题描述 (Problem Description)

OHOS 版 HiCodeer 的 Project panel 顶部显示的是从标题栏搬进面板的项目行（`ProjectPanel::render_title_bar_project_items`），点开它会弹出最近项目列表。该弹层底部固定渲染两个入口：「Open Local Folders」和「Open Remote Folder」。OHOS 没有 SSH / WSL / Docker 远程连接能力，远程入口点开后无法走通，属于纯粹的无用入口；侧栏 Add Project 图标弹出的另一个最近项目弹层同样带这两个入口。本次把两处的「Open Remote Folder」在 OHOS 上隐藏，只保留本地入口。

## 问题表现 (Symptoms)

- Project panel 顶部项目名/分支行点开后，弹层底部分两行：`Open Local Folders`、`Open Remote Folder`，各带快捷键提示。
- 侧栏右上角「Add Project」（FolderAdd 图标）弹层底部同样是这两行。
- 远程项点击后派发 `zed_actions::OpenRemote`，OHOS 上不存在可用的远程连接后端，等于死入口。
- 非 OHOS 平台行为正常，因此这是一个"移植后未做平台门控"的问题，而非上游缺陷。

## 问题原因 (Root Cause)

两个弹层共用 `recent_projects` crate 的渲染代码，远程按钮是上游为桌面平台写的，移植时未门控：

1. Project panel 顶部 → `title_bar::TitleBar::render_project_items` → `PopoverMenu` → `RecentProjects::popover(...)`，即 `ProjectPickerStyle::Popover`。
   在 `RecentProjectsDelegate::render_footer` 里，`popover_style == true` 时直接返回一个 `v_flex()`，先放本地按钮、再放远程按钮，然后提前 `return`（不走下面带列表选择态的通用 footer）。
2. 侧栏 Add Project → `sidebar::Sidebar::render_recent_projects_button` → `SidebarRecentProjects::popover(...)` → `SidebarRecentProjectsDelegate::render_footer`，结构与上面几乎一致，同样固定渲染两个按钮。

因此要在"入口层"隐藏，只能改这两个 footer；在 `title_bar` 或 `sidebar` 的弹层容器层拦不掉，因为按钮在 picker 内部。

关键约束：这两个 crate 是上游共享代码，直接删按钮会影响 Linux/macOS/Windows，必须用 `#[cfg(target_env = "ohos")]` 做编译期门控（移植标记规范要求：可门控必须门控）。

## 解决方案 (Solution)

思路：把每个远程按钮所在的 `.child(...)` 包成一个块表达式，用两条互斥的 `cfg` 分支决定渲染内容：

- `target_env = "ohos"`：返回 `gpui::Empty`。
  `Empty` 实现了 `IntoElement`，其 `request_layout` 用 `display: None` 申请布局，既不参与排版也不占位，比"返回 `div()`"更干净。
- `not(target_env = "ohos")`：原按钮代码整体后移一层缩进，末尾补 `.into_any_element()`（`Empty` 与 `AnyElement` 两种类型在不同 cfg 下各自成立，不会同时参与类型检查）。

改动示例（`crates/recent_projects/src/recent_projects.rs`）：

```rust
// 改动前
.child(
    ButtonLike::new("open_remote_folder")
        .child(/* Label + KeyBinding */)
        .on_click(/* dispatch OpenRemote */),
)

// 改动后
.child({
    // [OHOS PORT BEGIN] OHOS offers no remote folders, so the
    // recent-projects popover only shows the local entry.
    #[cfg(target_env = "ohos")]
    {
        gpui::Empty
    }
    // [OHOS PORT END]
    #[cfg(not(target_env = "ohos"))]
    {
        ButtonLike::new("open_remote_folder")
            .child(/* 原样 */)
            .on_click(/* 原样 */)
            .into_any_element()
    }
})
```

`crates/recent_projects/src/sidebar_recent_projects.rs` 同样处理。该文件里 `use zed_actions::OpenRemote;` 只被这个按钮使用，OHOS 下会变成 unused import，故一并加门控：

```rust
#[cfg(not(target_env = "ohos"))]
use zed_actions::OpenRemote;
```

未采用的方案与原因：

- 直接删除按钮代码：会让非 OHOS 平台丢功能，违反共享代码门控规则。
- 在 `title_bar` / `sidebar` 的容器层隐藏：按钮由 picker 的 footer 渲染，容器层看不到也管不到。
- `if cfg!(target_env = "ohos")` 运行时判断：代码仍会参与编译（`OpenRemote` 引用不消失），且不符合本仓库"可门控必须门控、用 `#[cfg]` 编译期裁剪"的移植标记规范。
- 把 footer 拆成两个函数（OHOS 版/通用版）：改动面更大，两个 crate 各要动整段渲染逻辑，收益为零。

验证：

- `RUSTUP_TOOLCHAIN=1.97.1 cargo check -p recent_projects --target aarch64-unknown-linux-ohos` 通过（含两处 cfg 分支的 OHOS 侧）。
- `bash ./script/bundle-ohos` 的 Rust 阶段完整通过（`recent_projects`、`project_panel`、`title_bar`、`gpui_ohos`、`launch-zed` 均 Compiling 成功，`Finished dev profile`，`libcore.so` 已 staging）。
- 同一次 bundle 在随后的 hicodeerd 构建阶段被终止，stderr 为空、无编译错误，当时系统 23G 内存仅剩 733MB，判断为 OOM，与本次改动无关；HAP 打包未完成。

回归面：仅影响 OHOS 编译产物；非 OHOS 分支的 DOM 结构与事件回调与改动前逐字一致（只多了一层 `AnyElement` 类型擦除，不改变布局与行为）。

## 修改文件 (Modified Files)

- `crates/recent_projects/src/recent_projects.rs` — `RecentProjectsDelegate::render_footer` 的 `ProjectPickerStyle::Popover` 分支：远程按钮改为 cfg 门控，OHOS 渲染 `gpui::Empty`。
- `crates/recent_projects/src/sidebar_recent_projects.rs` — `SidebarRecentProjectsDelegate::render_footer`：同样门控远程按钮；`use zed_actions::OpenRemote` 加 `#[cfg(not(target_env = "ohos"))]`。

相关实现位置（未改动，供定位）：

- `crates/project_panel/src/project_panel.rs:711` — OHOS 下把标题栏项目行搬进面板的入口。
- `crates/title_bar/src/title_bar.rs:818` — `recent-projects-menu` 弹层。
- `crates/sidebar/src/sidebar.rs:6776` — `sidebar-recent-projects-menu` 弹层。

提交：`8360ec3 OHOS 下最近项目弹层只保留打开本地文件夹入口`

参见 [[ohos-debug-lessons]]。
