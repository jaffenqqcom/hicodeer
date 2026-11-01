# Agent Panel 在切换布局/打开项目时消失、onboarding 期间出现

## 问题描述

OHOS 移植版中，Agent Panel 的可见性不受用户控制：

- 切换面板布局（Agentic ↔ Classic）或打开项目后，原本显示着的 Agent Panel 会消失；
- 首次设置（onboarding）页面显示期间，Agent Panel 反而出现，占住窗口一侧。

两者根因不同，但都出在同一个判断上：`AgentPanel` 的 `starts_open` 是否可靠。

## 问题表现

- `project panel` 开着，切换布局：Agent Panel 消失，另一侧面板正常显示。
- `project panel` 关着，切换布局：Agent Panel 不消失。
- Agent Panel 关着，切换布局：它不会出现（这一条是正确的）。
- onboarding 页面显示时，左侧出现 Agent Panel，与 setup 页并存。

复现条件：`agent_panel` 在左、`project/outline/git` 在右（Agentic），或反过来（Classic），切换布局开关即可。

## 问题原因

### 1. Agent Panel 读错了 onboarding 标记所在的数据库

`crates/agent_panel/src/agent_panel.rs` 的 `hidden_until_onboarding()` 读的是：

```rust
db::kvp::GlobalKeyValueStore::global().read_kvp("first_open")
```

`GlobalKeyValueStore` 落在 `db/0-global`，而 onboarding 流程写入的是工作区数据库 `db/0-dev`（`KeyValueStore::global(cx)`）。两边不是同一个库，所以这个函数**永远读不到标记**，恒返回「setup 未完成」。

后果：`Panel::starts_open` 恒为 false。

`Dock::add_panel` 在面板被加入某个 dock 时，会按 `starts_open` 决定是否打开这个 dock 并把面板设为当前显示（`crates/workspace/src/dock.rs` 的 `if !restored_state_names_panel && panel.read(cx).starts_open(...)`）。面板换 dock 走的正是这条路径，所以：

- Project Panel 的 `starts_open` 为真（`project_panel.starts_open` 默认 true，且工作区有目录型 worktree）→ 换边后自动打开，看起来"状态保持"；
- Agent Panel 的 `starts_open` 被压成 false → 换边后不打开 → 消失。

这就是为什么只有 Agent Panel 丢，其他面板不丢。

### 2. `first_open` 的语义是「展示过」，不是「走完」

上游 `onboarding::FIRST_OPEN` 在 `show_onboarding_view()` 里、**页面一显示就写入 `"false"`**，作用是「只展示一次」；启动判断只看键是否存在：

```rust
// crates/zed/src/main.rs / crates/zed/src/zed/open_listener.rs
else if matches!(kvp.read_kvp(FIRST_OPEN), Ok(None)) { 显示 onboarding }
```

把读取位置修对之后，onboarding 页面一出现 `first_open` 就存在了，Agent Panel 把它当成「setup 已完成」，`starts_open` 变真。再加上 `show_onboarding_view()` 里的 `workspace.toggle_dock(DockPosition::Left, ...)` 会打开左侧栏并把第一个可用面板设为当前，Agent Panel 就在 setup 期间冒了出来。

另外，空工作区会恢复上次保存的默认布局（`restore default dock state for empty workspaces`），也会把 dock 打开。

## 解决方案

### 关键点

不改 dock 的模式、不给 Agent Panel 开特例；只做两件事：

1. **让 `first_open` 表示「setup 走完」**：`show_onboarding_view()` 不再写 `"false"`，只在 `Onboarding::on_finish()` 里写 `"true"`。这样「键存在 = 完成」，上游两处 `Ok(None)` 判断无需修改，`AgentPanel` 也只需判断存在性。
2. **读对库**：`hidden_until_onboarding()` 从 `GlobalKeyValueStore` 改为工作区库 `KeyValueStore::global(cx)`。
3. **onboarding 独占界面**：删掉 `toggle_dock(Left)`；在 `open_new` 完成（默认布局恢复之后）再关闭该工作区的所有 dock。

### 修改前后

`agent_panel.rs`：

```rust
// 改前：读错库 + OHOS 门控 + 非 OHOS 恒 false
#[cfg(target_env = "ohos")]
fn hidden_until_onboarding() -> bool {
    !matches!(
        db::kvp::GlobalKeyValueStore::global().read_kvp(ONBOARDING_FIRST_OPEN_KEY),
        Ok(Some(_))
    )
}

// 改后：读工作区库，所有平台同一实现
fn hidden_until_onboarding(cx: &App) -> bool {
    !matches!(
        db::kvp::KeyValueStore::global(cx).read_kvp(ONBOARDING_FIRST_OPEN_KEY),
        Ok(Some(_))
    )
}
```

`onboarding.rs`：

```rust
// 改前：页面一显示就标记「已展示」
kvp.write_kvp(FIRST_OPEN.to_string(), "false".to_string())

// 改后：只在完成时标记
fn on_finish(...) {
    kvp.write_kvp(FIRST_OPEN.to_string(), "true".to_string())
}
```

```rust
// 改前：进入 setup 页时打开左栏
workspace.toggle_dock(DockPosition::Left, window, cx);

// 改后：不碰 dock；等窗口建好（默认布局恢复后）关闭所有 dock
cx.spawn(async move |cx| {
    task.await?;
    // 按 entity_id 找到 onboarding 的 workspace，close_all_docks
})
```

### 被否决的方案（记录，避免重走）

1. `Dock::add_panel` 增加 `open_on_add` 参数 + Workspace 级 defer 校正：能同时做到「显示保持显示、隐藏保持隐藏」，但属于按面板身份开例外，影响移动语义的一致性，被否决。
2. 只改 `apply_serialized_state`，让「保存为打开」生效：只覆盖打开项目，不覆盖切换布局，且与其他面板行为不一致。
3. 新增 `onboarding_completed` 键：能用，但与上游 `first_open` 并存需要迁移；最终选择复用上游键，改其写入时机。
4. 在 `workspace.rs` 判断「setup 未完成就不恢复默认布局」：改动蔓延到通用 crate，被否决；改为 onboarding 侧在窗口建好后关闭 dock。

## 修改文件

- `crates/agent_panel/src/agent_panel.rs` — `hidden_until_onboarding` 改读工作区库（`db/0-dev`），去掉 OHOS 门控；`starts_open` / `starts_open_overrides_restore` 传入 `cx`。
- `crates/onboarding/src/onboarding.rs` — `first_open` 只在完成时写入；移除进入 setup 时的 `toggle_dock(Left)`；窗口建好后关闭该工作区的所有 dock（按 entity_id 定位，避免多窗口时关错）。
- `crates/workspace/src/workspace.rs`、`crates/zed/src/main.rs`、`crates/zed/src/zed/open_listener.rs` — 最终版本无改动（中间尝试已全部回退）。

## 验证

- 切换 Agentic → Classic：`left=GitPanel(显示)`、`right=Agent Panel(显示)`；切回后 `left=Agent Panel`、`right=GitPanel`。Agent Panel 两个方向都保持显示。
- 模拟首次启动（清空工作区记录）：onboarding 期间 `left/right/bottom` 三个 dock 的 `visible` 全为 0，Agent Panel 不出现。
- 未完成 setup 时 Agent Panel 隐藏；点「完成设置」后 `first_open` 写入，面板恢复可用。

> 注：上述实测在「新键 + 值判断」的中间版本上完成；最终版把标记换回上游 `first_open`、只用存在性判断，逻辑等价（首次缺失 → 隐藏/显示 setup，存在 → 显示面板/不再显示 setup），未再次装包实测。

## 已知行为与取舍

- setup 未完成就退出应用，下次启动会再次显示 setup（上游是「展示过就不再显示」）。这是单一标记方案的取舍。
- 老用户升级：其库里的 `first_open` 值为 `"false"`（旧版展示时写入），键存在 → 不重复显示 setup，Agent Panel 也正常显示。因为判断只看存在性，旧数据天然兼容。
- 读取的值 `"true"` 本身只用于存在性判断，值内容是冗余的。

[[ohos-debug-lessons]]
