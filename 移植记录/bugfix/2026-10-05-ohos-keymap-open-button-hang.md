# 设置页 Open Keymap 按钮卡死 / 不打开编辑器

## 问题描述

在 HiCodeer（Zed 移植到 HarmonyOS NEXT）的**设置 → Keymap → Edit Keybindings** 区域点击
**Open Keymap** 按钮，界面卡死无响应。该按钮位于设置页的 `ActionLink` 上，代码是
`crates/settings_ui/src/page_data.rs` 的 `keymap_page()`。同一区域相邻的
**Edit in settings.json** 按钮工作正常（打开 settings.json 并关闭设置页），这一对照是
定位本问题的关键。

OHOS 只支持单窗口（`OhosPlatform::open_window` 对第二个窗口直接 `bail!`），因此设置页
是**主窗口里的一个 tab**，不是独立窗口。同一问题在 `main` 与 `hicodeer-de` 两个分支
都存在，与去 Zed 化无关，属通用移植缺陷。

## 问题表现

- 点击 Open Keymap 后应用无响应，界面冻结。
- 复现步骤：启动应用 → 打开设置 → Keymap 页 → 点 Open Keymap。
- 100% 必现。
- 旁证：`ctrl-k` 与标题栏的 Keymap 入口**完全正常**，同样在主窗口 tab 里打开编辑器。
  这个反差说明 `KeymapEditor` 与 `open_keymap_editor` 本身没问题，问题出在设置页
  这个入口的**触发方式**。

## 问题原因

### 两个入口的机制差异

根因是两条打开路径用了**根本不同的机制**。能正常工作的 `Edit in settings.json`
（`settings_ui.rs` 的 `open_current_settings_file`，OHOS 分支）**不派发 action，而是
直接调函数**：

```rust
workspace
    .with_local_or_wsl_workspace(window, cx, open_user_settings_in_workspace)
    .detach_and_log_err(cx);
close_settings_tab_in_workspace(workspace, settings_item_id, window, cx);
```

而 Open Keymap 走的是 `dispatch_action(OpenKeymap)`，要连过四道关：

```
dispatch_action
  → gpui/src/window.rs:2442  抓 self.focused(cx) 的焦点节点
  → cx.defer
  → cx.on_action 转发到 keymap_editor::init 注册的处理器
  → with_active_or_new_workspace 里再 cx.defer + 查 cx.active_window()
```

`Window::dispatch_action` 的第一道关就要求**当前窗口有焦点节点**，且它从
**设置窗口的 `Context<SettingsWindow>`** 发起时，`WindowHandle::update`（`window.rs:7230`）
要过 `root_view.downcast::<MultiWorkspace>()` 这一关。设置窗口尚未关闭时该 `update`
**返回 `Err`**，而原代码写的是 `.ok()` —— **错误被静默吞掉**，于是：

- `OpenKeymap` 从未被派发出去，编辑器从未被创建
- 但 `window.remove_window()` 照常执行，设置窗口被摘掉
- OHOS 只有这一个窗口 → 界面失去内容，看起来就是「卡死」

### 卡死与「只关不打开」是同一个原因的两面

调试中期出现过一个中间状态：**设置页正常关闭、但 keymap 不出现**。那是因为当时已把
同步的 `remove_window()` 换成了关 tab，卡死被治好了，但派发仍然落空。**这反过来印证了
根因判定** —— 卡死只是「摘掉唯一窗口」这一个动作造成的后果，编辑器从未存在是另一个
独立事实。两次修的都是同一处。

### 关键误判：defer 层数

排查中最大的弯路是把问题归因于 `cx.defer` 的**层数与顺序**。实际统计到的 defer 有五层：

| 层 | 位置 |
|---|---|
| 1 | `Window::dispatch_action` 内部（`window.rs:2445`） |
| 2 | `with_active_or_new_workspace` 内部（`workspace.rs:12434`） |
| 3–4 | 我尝试时自己套的两层 |
| 5 | 与本次缺陷无关的其他路径 |

曾在「一层 defer」「两层 defer」「塞进同一个 `update`」三种方案间反复，每次都基于
「defer 顺序」的假设，都失败。**真正的问题不在顺序，而在派发机制本身在这个上下文里
根本不通** —— 无论 defer 怎么排，`WindowHandle::update` 的 downcast 都会失败。

### 诊断教训

按 `/debug-log-process` 的规范复盘，本次排查**违反了「一次运行必须能收敛」**：

- 未先列出互相竞争的假设就开始改代码
- 分三轮加日志（8 个判别点 → 3 个 → 2 个），每轮一个编译，实际是 3 次盲试
- 中途还发散去查「标题栏那个按钮调了什么函数」「`DropdownMenu` 是什么类型」等
  **不在假设列表内**的问题

第三轮补的 `2b-window-update-failed` 才是决定性的：如果第一轮就把「从 on_click 到
`on_action` 再到 `open_keymap_editor`」整条边界一次打全，中间两轮根本不需要。判别点
`2-before-dispatch` 的**缺失**（而非内容）才是关键信息 —— 它证明闭包根本没执行。

## 解决方案

### 核心洞察

不派发 action，**照抄 `Edit in settings.json` 已验证可行的模式**：在 workspace 的
Context 里直接调函数 + 关 tab。

### 关键点：`ActionLink.on_click` 的签名必须能拿到 entity id

关 tab 需要设置页自己的 `EntityId`，而 **`entity_id()` 只存在于 `Context` 上，`App`
上没有**（`app.rs` 里只有 `notify(entity_id)`）。所以 `ActionLink` 的
`on_click` 签名从 `&mut App` 改为 `&mut Context<SettingsWindow>`：

```rust
// crates/settings_ui/src/settings_ui.rs:1754
on_click: Arc<
    dyn Fn(&mut SettingsWindow, &mut Window, &mut Context<SettingsWindow>) + Send + Sync,
>,
```

这里 `Context<SettingsWindow>` 必须写全称：**在结构体定义里 `Self` 指的是
`ActionLink` 而不是 `SettingsWindow`**（踩过这个坑）。另一处 `ActionLink`
（audio 测试按钮）同步改了签名，行为不变。

### 提取公共函数

两处调用（keymap 按钮、`open_current_settings_file`）需要完全相同的「defer → 打开
→ 关 tab」流程，提取到 `settings_ui.rs:7120`：

```rust
#[cfg(target_env = "ohos")]
pub(crate) fn open_in_workspace_then_close_settings_tab<T, F>(
    original_window: WindowHandle<MultiWorkspace>,
    settings_item_id: EntityId,
    cx: &mut App,
    open: F,
) where
    T: 'static,
    F: 'static + FnOnce(&mut Workspace, &mut Window, &mut Context<Workspace>) -> T,
{
    cx.defer(move |cx| {
        if let Err(err) = original_window.update(cx, |multi_workspace, window, cx| {
            multi_workspace.workspace().clone().update(cx, |workspace, cx| {
                workspace
                    .with_local_or_wsl_workspace(window, cx, open)
                    .detach_and_log_err(cx);
                close_settings_tab_in_workspace(workspace, settings_item_id, window, cx);
            });
        }) {
            log::error!("[ohos] failed to update the workspace: {err:?}");
        }
    });
}
```

泛型约束 `F` 与 `with_local_or_wsl_workspace`（`workspace.rs:3540`）完全一致，所以
两个调用点传的东西类型兼容。

### 改动前后的关键对比

```rust
// 改动前（派发 action，且错误被吞掉）
original_window
    .update(cx, |_workspace, original_window, cx| {
        original_window.dispatch_action(zed_actions::OpenKeymap.boxed_clone(), cx);
        original_window.activate_window();
    })
    .ok();                                  // ← Err 在此被丢弃
window.remove_window();                    // ← 摘掉唯一窗口 → 卡死

// 改动后（直接调函数 + 关 tab）
crate::open_in_workspace_then_close_settings_tab(
    original_window,
    cx.entity_id(),
    cx,
    |workspace, window, cx| {
        keymap_editor::open_keymap_editor(None, workspace, window, cx)
    },
);
```

### 顺带清理的两处

1. **非 OHOS 死代码**：`return` 在 `#[cfg]` 块内，非 OHOS 平台下其后的
   `original_window.update(...)` 与 `window.remove_window()` 是死代码，`window` 参数
   变成未使用 → `unused_variables` 警告。`script/clippy:28` 用
   `--deny warnings`，会直接挂 CI。修法：非 OHOS 那段整体收进
   `#[cfg(not(target_env = "ohos"))]`。

2. **条件依赖**：`keymap_editor` 只在 OHOS 分支被引用，加在通用 `[dependencies]`
   会让其它平台白挂一个 crate 及其依赖树。挪进
   `[target.'cfg(target_env = "ohos")'.dependencies]`（该段本已存在，用于 `rodio`）。

### 验证

- `./script/bundle-ohos`（debug）与 `--release` 均 `HAP BUILD SUCCESSFUL`
- release 产物 `nm` 可查到 `keymap_editor::open_keymap_editor` 引用，确认编进去了
- 覆盖安装后应用正常启动（`libcore.so` 的 NAPI 模块加载正常）
- 设置页 Edit in settings.json、`ctrl-k`、标题栏 Keymap 三个入口行为不变

## 修改文件

- `crates/keymap_editor/src/keymap_editor.rs` — 把 `open_keymap_editor` 从 `fn init`
  内部移到模块级并加 `pub`（`:120`），供设置页直接调用。**函数体逻辑一字未改**，
  仅 rustfmt 折行不同；两个原有调用点（`OpenKeymap` action、`ChangeKeybinding`）
  行为不变。
- `crates/settings_ui/src/settings_ui.rs` — 新增
  `open_in_workspace_then_close_settings_tab`（`:7120`，`#[cfg(target_env="ohos")]`）；
  `open_current_settings_file` 的 OHOS 分支改为调用它；`ActionLink.on_click` 签名
  第三参数改为 `&mut Context<SettingsWindow>`（`:1754`）。
- `crates/settings_ui/src/page_data.rs` — keymap 页的 `ActionLink` 改调上述公共函数
  并直接调 `keymap_editor::open_keymap_editor`（`:1631`）；非 OHOS 路径收进
  `#[cfg(not(target_env="ohos"))]`；audio 测试按钮的闭包签名同步。
- `crates/settings_ui/Cargo.toml` — `keymap_editor` 移入
  `[target.'cfg(target_env = "ohos")'.dependencies]`。

## 遗留与参考

- 本次修复在 `main` 分支上（去 Zed 化只在 `hicodeer-de` 分支，本问题与它无关）。
- 诊断日志（8 个 `[diag]` 判别点）已全部撤销，`keymap_editor.rs` 相对 `main` 的
  函数体逐 token 比对一致。
- 相关：[[ohos-debug-lessons]]
