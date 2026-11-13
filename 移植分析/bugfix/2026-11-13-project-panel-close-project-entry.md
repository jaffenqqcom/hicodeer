# 项目面板无法关闭当前项目

## 问题描述

Project Panel 里打开过一个项目之后，用户没有办法把它关掉，只能打开另一个项目去替换它。OHOS 版没有应用菜单栏，关项目的入口原本挂在菜单上，被裁掉之后就再没有第二个入口。用户要求把入口放回「文件树面板（Project Panel）→ 最近项目 → 打开本地文件夹」的同一组按钮里，打开与关闭合在一处最符合直觉。

## 问题表现

- 打开任意项目后，Project Panel 的最近项目弹出层里只有「打开本地文件夹」「远程连接」等条目，没有任何关闭入口。
- 命令面板里搜不到可用的关闭动作（`CloseProject` 在 OHOS 上没有可点击的承载控件）。
- 只能通过再打开一个项目来切换，当前项目无法回到「未打开」状态。

## 问题原因

`CloseProject` 这个 action 本身在 `crates/workspace` 里一直存在，处理逻辑完整。问题在于分发入口：

1. 桌面版把「关闭项目」挂在应用菜单里（`crates/zed/src/app_menus.rs`），菜单由平台菜单栏承载。
2. OHOS 走 `OhosPlatform`，没有应用菜单栏，也没有 `set_menus` 的实际呈现，于是这个 action 没有任何可见控件去 dispatch。
3. 第一次修复时改错了文件：改动落在 `crates/recent_projects/src/sidebar_recent_projects.rs`，而该模块虽然声明为 `pub mod`，实际并没有被任何视图引用；真正渲染弹出层的是 `crates/recent_projects/src/recent_projects.rs` 里的 `RecentProjectsDelegate::render_footer`。所以第一次改完界面没有变化。

## 解决方案

在 `RecentProjectsDelegate::render_footer` 的按钮区最前面插入一个 OHOS 专属条目，位置在 `open_local_folder` 之前，点击后 `window.dispatch_action(Box::new(CloseProject), cx)`。

要点：

- 用 `#[cfg(target_env = "ohos")]` 门控，非 OHOS 平台渲染 `gpui::Empty`，菜单入口保持原样，其它平台行为零变化。
- 用 `ButtonLike::new("close_project")` 与相邻条目同款控件，宽度由 `w_full` + `justify_between` 撑满，视觉上与「打开本地文件夹」成对。
- 动作类型直接复用 `workspace::CloseProject`，不新造 action，避免与菜单那条路径出现两套语义。

错误的第一次尝试（改 `sidebar_recent_projects.rs`）已完全回退，没有留下无效代码。

## 修改文件

- `crates/recent_projects/src/recent_projects.rs` — 在 `render_footer` 的按钮区新增 OHOS 专属「关闭项目」条目并 dispatch `CloseProject`；顶部按 OHOS 条件引入 `workspace::CloseProject`。

## 验证

- `./script/bundle-ohos`：EXIT=0，`=== HAP BUILD SUCCESSFUL ===`，控件与 action 接线通过编译期检查。
- 设备交互（弹出层按钮顺序、点击后回到无项目状态）需人工在设备上确认：本轮只做到编译验证，没有做截图确认（项目规则禁止截图）。
