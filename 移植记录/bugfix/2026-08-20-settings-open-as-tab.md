# OHOS 设置界面打不开（Tab 方案替代新窗口）+ json 设置文件打不开

## 问题描述

OHOS 移植版（zcoder）打开设置界面失败。桌面端用 `cx.open_window` 打开独立的 SettingsWindow 窗口，但 OHOS 是单个 XComponent surface，无法创建第二个独立窗口。先尝试 ModalView 全屏方案仍不可控（崩溃、Esc 无法退出、点击卡住），最终改用 **Tab 方案**：SettingsWindow 作为 Item 加入主窗口 tab 栏。附带问题：设置里点 "Edit in settings.json" 打不开 json 文件、且设置 tab 打开 json 后不自动关闭。

## 问题表现

- 打开设置：ModalView 方式触发 `double lease panic` 崩溃
- ModalView 全屏时按 Esc 无法退出设置界面
- 点 "Edit in settings.json" 界面卡住、json tab 打不开
- 打开 json 后设置 tab 不关闭
- 各阶段日志均无报错（错误被 `.ok()` 静默吞掉，排查全靠 [diag] 探针日志逐步定位）
- 升级到新版本 Zed 后再次出现：打开设置报 `window not found`；按下面第 5 条修掉后转为 `panicked ... cannot read workspace::Workspace while it is already being updated` 崩溃

## 问题原因

五条因果链叠加：

1. **根因——OHOS 无第二窗口**：GPUI 桌面端设置页由 `open_window` 开独立窗口承载，OHOS 单 XComponent surface 无法开第二个窗口。ModalView（`toggle_modal` + `render_bare=true`）全屏承载设置时初始化时序不可控（workspace mid-update 时读取触发 double lease panic、Esc 走 `menu::Cancel` 与全屏渲染冲突）。

2. **Tab 初始化时序**：SettingsWindow 作为 tab 在 `add_item_to_active_pane` 时，主 workspace 正处于 mid-update，`SettingsWindow::new` 里同步 `observe_workspace_projects` / `fetch_files` 读取 workspace 会 double lease panic → 必须延迟到 effect cycle 末尾。

3. **`cx.defer_in` 嵌套窗口 lease（json 打不开的直接根因）**：`defer_in` 的回调在 `with_window` **已持有主窗口 lease** 的闭包内执行（`context.rs` 的 `defer_in` = `app.defer` + `with_window`）。回调里再 `original_window.update(...)` 操作同一个主窗口 → **嵌套 lease → 返回 `Err`，被 `.ok()` 吞掉** → json 不打开、设置 tab 不关闭、无任何报错。

4. **`close_item_by_id` 异步 Task 被丢弃（tab 不关闭的直接根因）**：`Pane::close_items` 内部 `cx.spawn_in(window, async ...)` 返回 `Task<Result<()>>`。拿到 Task 后直接丢弃 → gpui Task 被 drop 即取消 → 关闭工作从未执行。

5. **上游合并后再现的两条时序链（2026-09-28 复核定案）**：

   - **`window not found`**：合并把「打开设置」的 OHOS 分支回退成**内联**调用 `workspace_handle.update(...)`。该调用发生在一次窗口更新的上下文内，而 gpui 在窗口更新期间会把目标窗口从 `cx.windows` 中 `Option::take` 走（`app.rs` 的 `update_window_erased`），于是同窗口的再入 `update` 返回 `Err("window not found")`（`app.rs` 的 `update_window_id`）。
   - **`cannot read workspace while it is already being updated`**：把上述调用改为延迟到 App 队列后，暴露出合并中**丢失**的 `observe_workspace_projects` 延迟步骤。`SettingsWindow::new` 在构造 tab 时同步读取 hosting workspace，而此刻该 workspace 正被 `Workspace::update` 持有 → `EntityMap` 的「already being updated」panic（SIGABRT）。

## 解决方案

**采用 Tab 方案承载设置界面**，所有修改用 `cfg(target_env = "ohos")` 包裹，桌面端逻辑零改动：

1. **打开设置**：`open_settings_editor_with` 加 OHOS 分支 → **`cx.defer` 延迟调用** `open_settings_editor_in_tab`（已有 tab 则 `activate_item` 聚焦，否则 `cx.new` + `add_item_to_active_pane` 新建）。**必须延迟**：宿主 tab 的插入会更新窗口句柄，而 gpui 在窗口更新期间把该窗口从 `cx.windows` 中 `take` 走，内联调用必报 `window not found`；`workspace_handle` 为空时打 `error` 并直接返回。

2. **Tab 化 trait**：`SettingsWindow` 实现 `Focusable` / `EventEmitter<()>` / `Item`（`tab_content_text` 返回 "Settings"），集中在独立 `ohos_settings_tab_impls` mod（cfg ohos）。

3. **延迟初始化**：`SettingsWindow::new` 桌面路径用 `cfg(not(ohos))` 保持原样；OHOS 走 `initialize_as_tab`（`cx.defer_in` 延迟 `observe_existing_workspaces` + `fetch_files` + `build_ui`）。原 `observe_workspace_projects` 在合并中丢失，本次**恢复为独立函数 `observe_existing_workspaces`**（逻辑与原实现一致：`AppState::global` → `workspace_store` → 逐个 workspace 做 `observe_release_in` / `subscribe_in`）；`SettingsWindow::new` 里仅 `cfg(not(ohos))` 同步调用它，OHOS 一律延迟到 `initialize_as_tab`，否则 tab 构造期读取 hosting workspace 会 `already being updated` panic。

4. **json 打开（关键修复）**：`open_current_settings_file` 的 OHOS 分支改用 **App 级 `cx.defer`**（无窗口 lease 包裹），回调里**一次** `original_window.update` 只 lease 主窗口一次，闭包内提取独立函数 `open_settings_file` 同时做两件事：
   - `with_local_or_wsl_workspace` 打开 settings.json
   - 遍历 `workspace.items_of_type::<SettingsWindow>` + `pane.close_item_by_id` 关闭设置 tab

   ```rust
   // before: defer_in 嵌套 lease，回调内 update 主窗口返回 Err 被吞掉
   cx.defer_in(window, move |this, window, cx| {
       original_window.update(cx, |mw, window, cx| { ... }).ok();  // 嵌套 lease → Err
       this.close_settings_tab(window, cx);
   });

   // after: App 级 defer + 一次 update，避免嵌套 lease；失败打 error
   cx.defer(move |cx| {
       if let Err(err) = original_window.update(cx, |mw, window, cx| {
           mw.workspace().clone().update(cx, |workspace, cx| {
               open_settings_file(workspace, window, cx);
           });
       }) {
           log::error!("[ohos] open_current_settings_file: failed to update workspace: {err:?}");
       }
   });
   ```

5. **tab 关闭（关键修复）**：`close_item_by_id(...)` 返回的 Task 加 `.detach()`，让异步关闭真正执行。

6. **异常路径日志**：保留 `update 失败 → error`、`settings tab 已关闭 → warn`，正常路径日志全部删除。

## 修改文件

- `crates/settings_ui/src/settings_ui.rs` — `open_settings_editor_in_tab`（cfg ohos 新函数，由 `open_settings_editor_with` 经 `cx.defer` 延迟调用，`settings_ui.rs:860`）；`ohos_settings_tab_impls` mod（initialize_as_tab、`observe_existing_workspaces`、Focusable/EventEmitter/Item impl）；`open_current_settings_file` OHOS 分支改 `cx.defer` + `open_settings_file` 独立函数；`SettingsWindow::new` 桌面逻辑 cfg(not ohos) 包裹并在同处仅非 OHOS 调用 `observe_existing_workspaces`（`settings_ui.rs:1926`），OHOS 路径改在 `initialize_as_tab` 延迟调用（`settings_ui.rs:7036`）；`observe_existing_workspaces` 独立函数（`settings_ui.rs:2082`）；全部针对 OHOS 的代码用 `cfg(target_env = "ohos")` 包裹，桌面端零改动

## 验证

- **构建**：`./script/bundle-ohos`（debug）→ `EXIT=0` + `HAP BUILD SUCCESSFUL`；`./install-local.sh` 装包后实机验证。
- **运行判据**：实机日志中 `window not found` 计数归零（修复前每次打开设置必现）；`panicked ... already being updated` 不再出现；设置 tab 可正常打开、可正常关闭。
- **残留噪声（非本问题）**：日志中 `LICENSE ... Permission denied` 等为上游资源访问报错，与本修复无关。
- **复现的两次踩坑**：一是内联 `workspace_handle.update` 在窗口更新上下文内必报 `window not found` —— OHOS 单窗口下任何「在窗口更新期再入同一窗口」的写法都要避免，统一延迟到 App 队列；二是延迟步骤（`observe_workspace_projects` 一类）在合并中极易丢失，回归时应先核对 `initialize_as_tab` 的延迟清单是否完整。

[[ohos-debug-lessons]]
