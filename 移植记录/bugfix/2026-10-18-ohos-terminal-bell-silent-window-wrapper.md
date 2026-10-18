# OHOS 终端响铃（BEL）无声：默认静音叠加窗口包装类型漏实现

> 相关定位手段见 [[ohos-debug-lessons]]（hilog 按 tag 过滤、二进制字符串核对、用实际返回类型而非实现类型判断派发目标）。

## 问题描述

HiCodeer 的 terminal panel 中执行 `printf '\a'` 没有任何声音。ACP 完成通知音可以正常播放，说明音频管线和扬声器本身没有问题。

终端 BEL 的链路是：PTY 输出出现 `0x07` → alacritty 解析为 `AlacTermEvent::Bell` → `TerminalBackendEvent::Bell` → `TerminalEvent::Bell` → `TerminalView` 检查 `terminal.bell` 设置 → `window.play_system_bell()` → 平台层 → 音频管线。

最终确认是**两个独立原因叠加**：上游默认把 `terminal.bell` 设为 `off`，以及 `OhosWindowHandle` 漏实现 `PlatformWindow::play_system_bell`，导致调用落到 trait 的空默认实现。

## 问题表现

- 在 terminal panel 执行 `printf '\a'`，无任何声音；ACP 通知音正常。
- 加临时诊断日志后，第一次运行只能看到前两段：

```
[diag-bell] terminal backend emitted Bell
[diag-bell] terminal_view received Bell, setting=Off
```

- 把设置改为 `system` 后，第三条出现，但平台层与音频入口始终不打印：

```
[diag-bell] terminal backend emitted Bell
[diag-bell] terminal_view received Bell, setting=System
[diag-bell] play_system_bell dispatched
```

- 用 `strings` 检查产物 `hap/entry/libs/arm64-v8a/libcore.so`：

```
[diag-bell] bell handler registered            1
[diag-bell] play_system_bell entered           1
[diag-bell] play_system_bell dispatched        1
[diag-bell] platform play_system_bell          0
```

平台层那条日志字符串根本不在二进制里，说明 `OhosWindow::play_system_bell` 从未被链接/调用。

## 问题原因

### 原因一：上游默认静音

`assets/settings/default.json:2285` 写着：

```json
"bell": "off"
```

用户 `config/settings.json` 的 `terminal` 段只设置了 `shell`，没有覆盖 `bell`，因此运行时 `TerminalSettings::get_global(cx).bell` 解析为 `Off`。`crates/terminal_view/src/terminal_view.rs:1231` 的判断不成立，`play_system_bell()` 根本没有派发。

### 原因二：窗口包装类型漏实现

设置改成 `system` 后，`window.play_system_bell()` 已经执行，但仍然没有进入平台层。关键在 `OhosPlatform::open_window` 的返回类型：

```rust
// crates/gpui_ohos/src/ohos/platform.rs:600
Ok(Box::new(super::window::OhosWindowHandle::new(window)))
```

GPUI 拿到的是 `Box<dyn PlatformWindow>`，背后实际类型是 **`OhosWindowHandle`**，不是 `OhosWindow`。而 `crates/gpui_ohos/src/ohos/window.rs` 里有**两套** `PlatformWindow` 实现：

- `impl PlatformWindow for OhosWindowHandle`（约 2580 行）
- `impl PlatformWindow for OhosWindow`（约 2771 行）

移植时只给 `OhosWindow` 加了 `play_system_bell`，`OhosWindowHandle` 漏了。于是动态派发落到 `crates/gpui/src/platform.rs:1331` 的空默认实现：

```rust
fn play_system_bell(&self) {}
```

`OhosWindow` 里的那份实现虽然编译通过，但该类型从未以 `dyn PlatformWindow` 的形式交给 GPUI，属于死代码。这也是为什么二进制里连日志字符串都被链接器丢掉了。

### 排查中的死路

- 第一轮怀疑 handler 未注册，先在 `platform::play_system_bell` 加日志；结果该日志不出现，说明问题不在音频侧，而在更上游。
- 第二轮怀疑 `SYSTEM_BELL_HANDLER` 被两份 `gpui_ohos` 实例拆开，检查 HAP 只有单个 `libcore.so`、依赖也只有一条 path，排除。
- 最后回头看 `open_window` 的返回值，才注意到 `OhosWindowHandle` 包装层。

## 解决方案

### 1. 补上真正生效的实现

在 `OhosWindowHandle` 的 `PlatformWindow` 实现里转发：

```rust
fn play_system_bell(&self) {
    super::platform::play_system_bell();
}
```

`OhosWindow` 里的同名实现保留，避免两套 impl 行为不一致。

### 2. 打开用户设置

在 `config/settings.json` 的 `terminal` 段加：

```json
"bell": "system"
```

该文件会热加载，不需要重编。是否把 OHOS 的默认值改成 `system` 属于产品决策，本次没有改上游默认文件。

### 3. 铃声来源

OHOS 没有 `NSBeep()` / `MessageBeep()` / `xcb.bell()` 的对等物：

- `@ohos.multimedia.systemSoundManager` 的 `SystemSoundType` 只有拍照、录像开始、录像结束三种相机音。
- `SystemTonePlayer` / `RingtonePlayer` 是 System API，三方应用调用返回 `202 Caller is not a system application`，公开 SDK 26 里没有对应 `.d.ts`。
- `/system/etc/audio` 只有配置 xml/dat，`/system` 分区没有可读的 ogg/mp3/wav。

因此改为合成一记短音，按 macOS 默认提示音 Tink（新版界面显示为 Boop）的特征设计：

- 基频 1900 Hz，能量落在人耳敏感的 1–4 kHz
- 三个非谐分音（1.0 / 2.42 / 3.76 倍），模拟敲击体的金属感
- 2 ms 起音、指数衰减（时间常数 38 ms）、总长 160 ms、尾部 5 ms 淡出
- 峰值振幅 0.4

实现位于 `crates/audio/src/audio_pipeline_ohos.rs` 的 `render_bell_tone()`，渲染结果进入与通知音相同的 mixer/renderer。

### 4. 麦克风权限改为按需申请

初版在 `EntryAbility.onWindowStageCreate` 之后无条件调用 `requestPermissionsFromUser`，结果是：从不使用音频测试的用户也会在启动时看到麦克风弹窗。改为点击 Start Testing 时才申请：

- Rust 侧走 `openharmony-ability` 的权限桥插件（`PermissionBridgePlugin`，插件 ID `ohos.permission`），它把请求转成 ArkTS 的 `abilityAccessCtrl.requestPermissionsFromUser`。
- `audio::request_microphone_permission()` 必须在主线程构造 future：`global_app()` 是 thread_local，后台线程取不到；返回的 `Pin<Box<dyn Future + Send>>` 只捕获 bridge，可以交给 `cx.spawn` 在任意线程等待。
- 设置页点击后 `cx.spawn` 等待授权结果，再调用 `start_test_playback`；`AudioTestPageState.pending` 防止权限弹窗期间重复点击启动第二个采集流。
- HAP 侧注册 `PermissionPlugin`，并在 `hap/build-profile.json5` 的 modules 登记 `plugin_permission`、`hap/entry/oh-package.json5` 增加 HAR 依赖。漏登记模块会报 `00306055 Missing module configuration for external module`。

### 5. 音频测试的设备选择真正生效

初版 `open_test_output` 忽略传入的 `device_id`，选择设备没有实际效果。OHOS API 26 的 NDK 提供了增强路由接口，用它按 id 选择：

- `OH_AudioDeviceEnhanceManager_IsEnhancedRoutingSupported` 先判断机型是否支持，不支持则打一条 warn 后回退系统默认。
- `OH_AudioDeviceEnhanceManager_SelectOutputDeviceForAudioRenderer` / `SelectInputDeviceForAudioCapturer` 接受 `OH_AudioDeviceDescriptor*`，因此需要按设备 id 从 `OH_AudioRoutingManager_GetDevices` 的数组里找到对应 descriptor。
- `with_device_descriptor` 封装“枚举 → 按 id 匹配 → 回调 → `ReleaseDevices`”，避免 descriptor 指针在数组释放后继续使用。
- 注意 `OH_AudioRenderer_SetDefaultOutputDevice` 不是替代方案：它只接受 EARPIECE / SPEAKER / DEFAULT 三种类型，并且仅对通信类 StreamUsage 生效。

### 6. 签名 profile 的 ACL 导致安装失败

改动过程中安装一度固定失败：

```
install failed due to grant request permissions failed.
PermissionName: ohos.permission.kernel.ALLOW_WRITABLE_CODE_MEMORY
```

hilog 里的判据是 `[IsAclSatisfied] ... need acl` 与 `Acl of ... is invalid`。根因不在代码：

- `~/.ohos/config` 存在两套同 bundle（`com.hicodeer.edit`）的签名材料，`hap/build-profile.json5` 指向的那套 profile 里 `acls` 是空的。
- 之前能覆盖安装，是沿用了设备上旧的 access token；本次新增了外部 HAR 模块，BMS 判定 bundle 结构变化后重新 `InitHapToken`，重新校验 ACL 就失败了。
- 排查方法：`java -jar hap-sign-tool.jar verify-profile -inFile <p7b> -outFile <json>`，查看 `content.bundle-info.apl` 与 `content.acls.allowed-acls`。
- 口令问题：两套材料互不通用。`script/read-sign-pwd.js` 从 `material/` 解出的口令只能打开旧 p12，带 ACL 那套 p12 的口令密文已丢失。最终由 DevEco 重新自动签名生成新的密文后才构建通过。
- 经验：切换签名材料必须 cer + p7b + p12 + `material/` 整套一起换，只换文件会出现 `KEYSTORE_PASSWORD_ERROR`。

## 修改文件

- `crates/gpui_ohos/src/ohos/window.rs` — 给 `OhosWindowHandle` 补 `play_system_bell`；这是真正修复无声的一行
- `crates/gpui_ohos/src/ohos/platform.rs` — 新增 `SYSTEM_BELL_HANDLER`、`set_system_bell_handler`、`play_system_bell`；注册 `PermissionBridgePlugin`
- `crates/gpui_ohos/src/ohos.rs` — 导出 `set_system_bell_handler`
- `crates/gpui_ohos/src/gpui_ohos.rs` — 在 crate 根重新导出
- `crates/gpui_ohos/Cargo.toml` — 新增 `openharmony-ability-plugin-permission`
- `crates/gpui_ohos/depend/launch-zed/src/launch_app.rs` — 启动时注册 `audio::play_system_bell`
- `crates/gpui_ohos/depend/launch-zed/Cargo.toml` — 新增 `audio` 依赖
- `crates/audio/Cargo.toml` — OHOS 条件依赖增加 `openharmony-ability-plugin-permission`
- `crates/audio/src/audio.rs` — OHOS 条件导出 `play_system_bell` 与 `request_microphone_permission`
- `crates/audio/src/audio_pipeline_ohos.rs` — 实现铃声合成、mixer 入队、renderer 空闲释放；按 id 选择输入/输出设备；按需请求麦克风权限；顺带修复 renderer/capturer 启动失败泄漏
- `crates/audio/src/ohos_audio_sys.rs` — 新增 OHOS 音频 NDK FFI（renderer/capturer/设备枚举/设备增强路由）；DEFAULT 设备类型改为双向
- `crates/settings_ui/src/pages/audio_test_window.rs` — OHOS 单窗口下改为设置页内嵌音频测试；点击后异步申请权限；测试会话改由窗口持有
- `crates/settings_ui/src/settings_ui.rs` — 新增 `audio_test_stop` 字段，随 `SettingsWindow` 释放时停止测试
- `crates/settings_ui/src/pages.rs` — OHOS 条件导出 `render_audio_test_page`
- `crates/settings_ui/src/page_data.rs` — OHOS 用 `SubPageLink` 替代独立测试窗口的 `ActionLink`
- `hap/entry/src/main/module.json5` — 声明 `ohos.permission.MICROPHONE`
- `hap/entry/src/main/resources/base/element/string.json` — 权限用途说明
- `hap/entry/src/main/ets/entryability/EntryAbility.ets` — 删除启动时的权限请求；注册 `PermissionPlugin`
- `hap/entry/oh-package.json5` — 增加 `@ohos-rs/ability-plugin-permission` 依赖
- `hap/build-profile.json5` — modules 增加 `plugin_permission`

## 顺带修复（代码审阅）

- `ensure_renderer` 中 `OH_AudioRenderer_Start` 失败时未 `Release` 已生成的 renderer
- `InputStream::open` 中 `OH_AudioCapturer_Start` 失败时未 `Release` 已生成的 capturer
- `TestMixer` 入队时未递增 `PLAY_GENERATION`，音源短暂断流会让空闲释放线程误关 renderer
- `device_type_capabilities` 把 `DEFAULT`（1000）只标记为输出，输入下拉缺少“默认设备”

## 后续事项

- `terminal.bell` 上游默认 `off`；是否在 OHOS 上默认改成 `system` 需要产品决定
- mixer 当前串行播放而非混音：ACP 完成音（1.6 秒）期间触发的 BEL 会排队延迟
- 设备选择依赖系统的增强路由能力；部分机型 `IsEnhancedRoutingSupported` 返回 false，此时只能回退系统默认设备
- 相关经验：[[ohos-debug-lessons]]

