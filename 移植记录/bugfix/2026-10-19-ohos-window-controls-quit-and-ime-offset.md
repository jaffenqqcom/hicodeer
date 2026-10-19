# OHOS 自绘窗口三键与退出链路：关闭无响应、最大化变沉浸、恢复失效、窗口黑框

> 相关定位手段见 [[ohos-debug-lessons]]：用 `snapshot_display -t png` + 自写裁剪脚本做像素级比对、用系统 App（hishell）当参考实现、`uitest uiinput` 与 `uinput -M -m` 的坐标系差异。

## 问题描述

HiCodeer（OHOS 2in1）原本使用系统标题栏。为了做成 mac 风格的自绘标题栏（关闭 / 最小化 / 最大化三键），在 `gpui_ohos` 里把窗口装饰切换成 `Decorations::Client`，并让 ArkTS 隐藏系统标题栏。改造后暴露出一串连锁问题：自绘关闭按钮点了没反应、最大化后系统状态栏和坞栏被隐藏、最大化/恢复按钮不切换图标也不生效、非最大化时窗口四周多出一圈黑框、菜单栏应用图标空占位。

同时，去掉系统标题栏后，IME 候选框的垂直定位多算了一段标题栏高度，候选框整体偏下。

## 问题表现

- 点自绘关闭按钮：弹不出确认框（早期版本直接无任何响应），窗口不关，进程继续跑。
- 点自绘最大化按钮：窗口铺满，但系统状态栏、Dock 一起消失，表现为全屏而不是最大化。
- 最大化后中间按钮仍是"最大化"图标；再点一次没有任何效果（应恢复成最大化前的尺寸）。
- 非最大化时窗口四周有一圈黑色边框/阴影（系统已经有一圈窗口阴影，应用又画了一层）。
- 菜单栏最左边预留了图标位置，但图标是空白的。
- 窗口恢复成浮动后，IME 候选框比光标低约 46vp（原代码按标题栏高度补的偏移）。

## 问题原因

### 1. `Platform::quit` 是空实现，窗口只在 gpui 侧被移除

`App::quit()` 最终调用 `Platform::quit()`，而 OHOS 平台层的实现只有一行日志：

```rust
fn quit(&self) {
    log::warn!("quit requested on OHOS; app exit is managed by the system");
}
```

`on_quit()` 同样是空实现，所以 gpui 在 `App::new` 里注册的 shutdown 回调永远不会被调用。整条链子变成：

```
关闭按钮 / CloseWindow
  → MultiWorkspace::close_window
  → prepare_window_to_close（只处理未保存文件）
  → window.remove_window()          // 只是把 gpui 的 Window 标记为 removed
  → window_closed_observers → cx.quit()
  → Platform::quit()                // 空实现，停在这里
```

结果：gpui 认为窗口没了，但原生窗口和进程都还在，屏幕上停留在最后一帧——用户看到的就是"点了没反应"。

### 2. `maximize()` 默认进入沉浸模式

SDK 里 `Window.maximize(presentation?: MaximizePresentation)` 的默认值是 `ENTER_IMMERSIVE`（见 `@ohos.window.d.ts`），而不是普通最大化。所以 `win.maximize()` 会把状态栏和 Dock 一起隐藏；此时 `getWindowStatus()` 也不是 `MAXIMIZE`，导致最大化状态镜像一直是 false。

### 3. 图标选择依赖一个"会撒谎的缓存"

标题栏渲染时同步读取 `PlatformWindow::is_maximized()` 来决定画最大化还是恢复图标，而 OHOS 这里返回的是 ArkTS 通过 NAPI 回写的镜像值。三个因素叠加让它不可靠：

- 窗口由 `module.json5` 的 `ohos.isMaximize` 元数据以最大化状态创建，Zed 之后又按会话恢复自己的 bounds；
- `windowStatusChange` 不一定在每次状态迁移时都到达；
- 镜像更新后没有任何机制触发重绘。

于是出现"最大化后图标不变"和"恢复按钮把窗口又最大化一次（因为镜像还是 false）"。

### 4. 客户端装饰的阴影/边框被重复绘制

切换到 `Decorations::Client` 后，Zed 会自己画一层"窗口阴影 + 1px 边框 + 圆角 + 阴影内边距"（`workspace.rs::client_side_decorations`）。OHOS 的系统窗口本身已经有阴影和圆角裁剪，应用再画一层，内边距区域用 `transparent_black()` 填充，在黑色背景上就成了一整圈黑框。标题栏和状态栏还会各自补圆角，在四角留下黑色碎片。

### 5. 应用图标没有真正打进二进制

`img("images/hicodeer.png")` 读取的是 `util::fs_embed!` 生成资源索引。新加的 PNG 没有触发 `assets` crate 重编译，RustEmbed 的索引里没有这条记录，`img()` 只保留了布局占位而画不出像素。

### 6. IME 候选框多算了一段标题栏

`plugins/ime/src/main/ets/ImePlugin.ets` 里用一个常量把标题栏高度写死成 46vp：

```ts
const TITLE_BAR_HEIGHT_VP = 46;
...
const top = this.lastCursor.y + winTop + titleBarHeightPx() + CURSOR_TRIM_Y_PX;
```

隐藏系统标题栏后，XComponent 的内容区已经与窗口顶边对齐，这段偏移就变成了纯粹的误差；而且 46vp 只是估计值，系统实际范围是 37~112vp。

## 解决方案

### 退出链路：延迟到下一轮主循环执行 shutdown，再终止 ability

`App::quit()` 是在 App 被可变借用期间调用的，`on_quit` 回调里的 `App::shutdown()` 不能同步跑（`try_borrow_mut` 会失败）。改为：

```rust
fn quit(&self) {
    let callback = self.quit_callback.borrow_mut().take();
    self.foreground_executor.spawn(async move {
        if let Some(mut callback) = callback {
            if !callback() { log::warn!("quit: gpui could not run the quit handlers synchronously"); }
        }
        if !openharmony_ability::terminate_ability() { log::warn!("quit: ..."); }
    }).detach();
}

fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
    *self.quit_callback.borrow_mut() = Some(callback);
}
```

最终链路：`remove_window → cx.quit → 延迟 shutdown（agent flush / LSP / 会话 / 窗口序列化）→ terminateSelf`。

### 关闭确认：只加在自绘关闭按钮上

在 `platform_linux.rs` 的 `WindowControlType::Close` 分支里，OHOS 下先 `window.prompt("Are you sure you want to quit?")`，确认后才 dispatch `CloseWindow`；取消则什么都不做。用 `#[cfg(target_env = "ohos")]` / `#[cfg(not(target_env = "ohos"))]` 两个互斥分支隔离，Linux 保持原行为。

### 最大化：显式退出沉浸 + 用实时状态切换

```ts
win.maximize(window.MaximizePresentation.EXIT_IMMERSIVE)
```

恢复时再补一次 `setImmersiveModeEnabledState(false)`（`recover()` 只恢复 bounds，不退出沉浸标记）。Rust 侧不再根据镜像二选一，而是调用单一的 toggle：

```rust
fn zoom(&self) {
    openharmony_ability::toggle_main_window_maximize();
}
```

ArkTS 用 `win.getWindowStatus() === MAXIMIZE` 决定 maximize 还是 recover，因此行为与缓存无关；镜像只用于图标，并在"窗口创建后 / 内容加载后 / windowStatusChange / maximize-recover 完成"四个时机回写。

### 黑框：OHOS 下不画客户端装饰框

`workspace.rs`（阴影内边距、1px 边框、box-shadow）、`status_bar.rs` 与 `platform_title_bar.rs` 的圆角分支都加了一条 OHOS 前置分支，直接返回未加工的 `div`，让系统负责窗口圆角与阴影；`set_client_inset` 也改为 0。

### 图标：OHOS 专用资源 + 条件选择

- 新增 `assets/icons/ohos_window_maximize.svg` / `ohos_window_restore.svg`，沿用系统那种"两段直角角标"字形（顺序：最大化/恢复、最小化、关闭）；
- `WindowControlType::icon()` 通过一对互斥常量选择图标，避免污染上游的 `generic_*.svg`；
- 菜单栏应用图标从 `hap/AppScope/resources/base/media/app_icon.png` 拷到 `assets/images/hicodeer.png`，缩到 64×64 并加 3px 圆角，插入标题栏 `children[0]`，用 `#[cfg(target_env = "ohos")]` 门控；
- 资源没被嵌入的坑：新增资源后必须让 `assets` crate 重编译（本次用 `touch crates/assets/src/assets.rs`），否则 RustEmbed 索引不会更新。

### IME 偏移：向系统查询，而不是假设

`titleBarHeightPx()` 改为：

```ts
const win = this.hookContext?.getWindow();
if (typeof win.getWindowDecorVisible === "function" && !win.getWindowDecorVisible()) return 0;
const heightVp = win.getWindowDecorHeight();   // API 12+，实际 37~112vp
return heightVp > 0 ? heightVp * display.getDefaultDisplaySync().densityPixels : 0;
```

这样"有系统标题栏的 app"用系统实测高度，"隐藏标题栏的 app"贡献 0，公共组件不需要为每个接入方加配置。

### 走过的弯路（供后续参考）

- 一开始把 `setWindowDecorVisible(false)` 放在 `getMainWindow()` 之后、`loadWindowStageContent()` 之前，调用静默无效（不报错、也不生效）；必须在内容加载后再调。
- 一度试图从 `/proc/<pid>/maps` 确认字体/资源是否加载，但窗口内容走 GPU 合成，映射里看不到；改用 `snapshot_display -t png` + 自写 PNG 解码/裁剪脚本做像素对比。
- `uitest uiinput click` 用的是物理像素，`uinput -M -m dx dy` 实际是绝对坐标，两者混用导致多次点错位置；最终用 "PNG 截图 → 裁剪 4 倍放大" 才准确定位按钮。
- 第一次只删掉 `TITLE_BAR_HEIGHT_VP` 常量，会让共享组件的其他接入方（有标题栏）候选框上移；改成查询系统状态后才对所有接入方成立。
- 图标方向（最大化/恢复）第一次做反了，最后以 hishell 最大化与浮动两张截图的像素比对为准。

## 修改文件

### HiCodeer

- `crates/gpui_ohos/src/ohos/platform.rs` — 新增 `quit_callback` 字段与 Clone；`quit()` 延迟执行 shutdown 并 `terminate_ability()`；`on_quit()` 保存回调；`button_layout()` 返回右侧三键（最大化/恢复、最小化、关闭）。
- `crates/gpui_ohos/src/ohos/window.rs` — `window_decorations()` 改为 Client；`window_controls()` 打开 minimize/maximize；`minimize()/zoom()` 接 ArkTS 动作；`is_maximized()` 读镜像；给 `OhosWindowHandle` 补 decorations/controls 转发。
- `crates/gpui_ohos/depend/launch-zed/src/launch_app.rs` — 删除启动诊断日志。
- `crates/platform_title_bar/src/platforms/platform_linux.rs` — 关闭按钮在 OHOS 下先弹确认框；新增 OHOS 图标常量对与选择逻辑。
- `crates/platform_title_bar/src/platform_title_bar.rs` — OHOS 下跳过客户端装饰边框与圆角。
- `crates/workspace/src/workspace.rs` — OHOS 下 `set_client_inset(0)`，跳过阴影内边距、边框和 box-shadow。
- `crates/workspace/src/status_bar.rs` — OHOS 下跳过状态栏底部圆角与补偿边距。
- `crates/title_bar/src/title_bar.rs` — 标题栏最左侧插入 HiCodeer 图标（OHOS 门控，16px、3px 圆角）；children 容量 5→6。
- `crates/icons/src/icons.rs` — 新增 `OhosWindowMaximize` / `OhosWindowRestore` 两个图标枚举项（无 cfg，避免非 OHOS 的图标一致性单测失败）。
- `assets/icons/ohos_window_maximize.svg`、`assets/icons/ohos_window_restore.svg` — 新增 OHOS 专用直角角标。
- `assets/images/hicodeer.png` — 新增 64×64 应用图标（由 hap 的 1024×1024 盒式滤波缩小）。
- `crates/gpui_ohos/Cargo.toml`、`crates/audio/Cargo.toml`、`crates/gpui_ohos/depend/launch-zed/Cargo.toml` — `openharmony-ability*` 临时改为相对 path 依赖（避免同一 crate 链接两份导致 `global_app` 各存一份），待 fork push 后切回 git。

### openharmony-ability-zed（fork）

- `crates/ability/src/window_control.rs` — 窗口动作扩为 minimize / show_and_focus / maximize / recover / toggle_maximize；新增 `set_main_window_maximized` / `is_main_window_maximized` 状态镜像；删除正常路径 info 日志，仅保留异常路径。
- `crates/derive/src/lib.rs` — `set_window_actions` 转发 5 个闭包；导出 `set_main_window_maximized`。
- `native_ability/src/main/ets/ability/NativeAbility.ets` — 注册 5 个窗口动作；`hideSystemWindowDecor()`（内容加载后调用）隐藏系统标题栏与系统三键并退出沉浸；`maximize(EXIT_IMMERSIVE)`；`recover()` 后恢复系统栏；`windowStatusChange` 与操作完成时回写最大化状态；`toggleMaximizeMainWindow()` 按实时状态切换。
- `native_ability/src/main/ets/ability/type.ets` — `Module` 接口补 5 参 `setWindowActions` 与 `setMainWindowMaximized`。
- `plugins/ime/src/main/ets/ImePlugin.ets` — 标题栏高度改为 `getWindowDecorVisible()` + `getWindowDecorHeight()` 查询，去掉写死的 46vp；同步更新注释。
