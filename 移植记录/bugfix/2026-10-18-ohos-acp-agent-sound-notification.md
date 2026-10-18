# OHOS 上 ACP agent 完成通知音不生效

## 问题描述

HiCodeer（Zed 的 OHOS 移植）中，ACP agent 执行长命令或完成一轮回复后没有声音提示。上层逻辑本身正常：`AcpThreadEvent::Stopped` 会调用 `notify_with_sound`，用户设置 `play_sound_when_agent_done` 也已设为 `always`。问题出在 OHOS 的音频后端 `crates/audio/src/audio_pipeline_ohos.rs`：移植时只保留了编译接口，`play_sound` 是空实现，只打印一条 warn，从未真正调用 OHOS 音频 API。

## 问题表现

- 应用日志出现：

```
audio_pipeline_ohos: play_sound(AgentDone) is not implemented on OHOS yet, no sound will be played
```

- 即使把 `play_sound_when_agent_done` 设为 `always`，ACP agent 完成一轮后仍然没有声音
- 第一次实现 SoundPool 播放后，日志变为：

```
SoundBridgePlugin: loaded sound 1 from rawfile
SoundPool: #212 SoundPool::Play, soundID(1) has not been loaded completely
SoundBridgePlugin: action play failed: BusinessError: Failed to play sound, soundID not loaded
SoundPoolCallBackNapi: #179 can not find loadcompleted callback!
```

- 修复加载时序后，日志显示播放成功：

```
audio_pipeline_ohos: play_sound(AgentDone)
SoundBridgePlugin: load complete for sound 1
SoundBridgePlugin: loaded sound 1 from rawfile
SoundBridgePlugin: play finished stream 1
```

但耳朵仍然听不到声音
- 查询音频策略服务：

```
hidumper -s AudioPolicyService -a -m
Ringer Mode: SILENT
```

## 问题原因

问题由四层原因叠加而成，前一层修好后才会暴露下一层。

**1. 移植层未实现**

`crates/audio/src/audio_pipeline_ohos.rs` 在移植时是空壳，`play_sound`、`end_call`、`init` 都只记录日志，没有接入 OHOS 音频能力。因此上层所有 `Audio::play_sound` 调用都不会产生声音。

**2. SoundPool 的加载时序误用**

`media.SoundPool.load(fd, offset, length)` 返回的 Promise 只表示「加载请求已提交」并给出 soundId，**不代表资源已经加载完成**。首次实现直接 `await load()` 后立刻 `play()`，系统报：

```
SoundPool::Play, soundID(1) has not been loaded completely
```

同时因为插件没有注册 `loadComplete` 监听，系统加载完成时找不到回调：

```
can not find loadcompleted callback!
```

正确的时序是注册 `on('loadComplete')`，等到对应 soundId 的加载完成事件后再调用 `play()`。

**3. 通知流在静音模式下被系统抑制**

第二次修复后，SoundPool 播放完整成功（收到 `play finished stream 1`），但设备处于：

```
Ringer Mode: SILENT
```

音频流使用的是 `STREAM_USAGE_NOTIFICATION`（7）。鸿蒙在静音/铃声静音模式下会抑制通知流，因此 API 调用成功、播放事件正常，但扬声器没有输出。这不是权限问题，SoundPool 播放不需要额外权限。

**4. 音效资源重复打包**

最初把 `assets/sounds/*.wav` 额外复制到 HAP 的 `resources/rawfile/sounds/`，通过 `resourceManager.getRawFd` 加载。但 Zed 的 `crates/assets/src/assets.rs` 已经通过 `util::fs_embed!` 把 `sounds/**/*` 编译期嵌入 `libcore.so`，OHOS 构建脚本也固定启用 `--features util/debug-embed`。于是 HAP 内同时存在两份音效（约 684KB 重复）。SoundPool 只能从文件描述符或 URI 加载，不能直接读内存字节，这是当时选择 rawfile 的原因。

## 解决方案

**1. 新增 play-notification 插件（openharmony-ability）**

- 新增 `crates/plugin-play-notification` 与 `plugins/play-notification` HAR
- 插件 ID 为 `ohos.play-notification`，类型名统一为 `ohos.play-notification.*`
- ArkTS 侧管理单例 SoundPool，支持 rawfile、file、fd 三种资源来源，以及 play、stop、set-volume、set-loop、set-rate、set-priority、set-interrupt-mode、unload、release
- 插件创建时注册 `playFinishedWithStreamId`、`loadComplete`、`errorOccurred` 事件

**2. 等待 loadComplete 后再播放**

ArkTS 插件维护两个状态：

- `loadedSounds: Set<number>`：已经收到 loadComplete 的 soundId
- `pendingLoads: Map<number, callback>`：仍在等待 loadComplete 的 soundId

`load` 动作在 `pool.load()` 返回 soundId 后调用 `awaitSoundLoaded(soundId)`，只有收到对应 soundId 的 `loadComplete` 才返回成功。事件先于 Promise resolve 时会缓存到 `loadedSounds`；`errorOccurred` 会 reject 等待者；等待有 10 秒超时。

**3. 音效改为 Zed 已有嵌入方案 + 运行时落盘**

- 删除 `script/bundle-ohos` 中复制 `assets/sounds` 到 rawfile 的步骤
- 删除 `.gitignore` 的 rawfile 规则和已生成的 rawfile 目录
- `audio_pipeline_ohos.rs` 新增 `ensure_sound_file`：
  - 首次播放时用 `cx.asset_source().load("sounds/<name>.wav")` 读取 libcore.so 内嵌的字节
  - 写入 `paths::data_dir()/cache/sounds/<name>.wav`
  - 之后复用缓存文件，通过 `SoundSource::File` 交给 SoundPool 加载
- `Audio` 全局仍按 `Sound` 缓存 soundId；SoundPool 懒创建（`maxStreams = 4`、`STREAM_USAGE_NOTIFICATION`）

**4. 并发与缓存写入的健壮性修复**

- ArkTS 插件用一个共享的 `creatingPool` Promise 串行化首次创建：多个并发首次播放不会各自创建 SoundPool，避免后完成者覆盖前者导致池泄漏
- `ensure_sound_file` 用全局 `SOUND_CACHE_LOCK` 保护检查与写入，避免并发首次播放同时写同一文件
- 缓存文件只接受非空文件；写入采用「临时文件 + rename」，避免中断写入留下半截文件
- 加载失败时删除对应的缓存文件，下一次播放会从内嵌资源重新生成，避免坏缓存永久卡住

**5. 静音模式的结论**

- 播放链路和插件均已验证正常
- 设备 `Ringer Mode: SILENT` 时通知流不发声，符合系统设计
- 如果希望静音模式下也发声，需要把 usage 改为 `STREAM_USAGE_MUSIC`（1），以媒体音量播放；这是产品取舍，默认保持通知流语义

**6. 已删除的临时调试代码**

- `EntryAbility.ets` 中直接调用 SoundPool 的自检方法 `playSoundSelfCheck` 及其 import、3 秒延时调用均已删除
- 临时日志目录 `.workbuddy/tmp/hilog-*` 已删除
- 验证日志确认干净版已重新打包并覆盖安装

## 验证结果

- `cargo check -p audio`：通过
- `cargo check -p gpui_ohos`：通过
- `script/bundle-ohos --hap-only`：HAP 打包成功，`ArkTS:ERROR` 为 0
- HAP 的 `ets/modules.abc` 中已包含 `PlayNotificationPlugin` 与 `ohos.play-notification`
- HAP 中已无 `resources/rawfile/sounds`
- 设备日志确认 SoundPool 创建、加载完成、播放完成事件全部到达
- 设备静音模式下听不到声音，退出静音后确认有声音
- 待验证：运行时落盘版本的完整 Rust 构建（当前已安装的 libcore.so 仍是旧构建，未包含 `ensure_sound_file`）

## 修改文件

openharmony-ability 工作副本：

- `crates/plugin-play-notification/Cargo.toml` — 新增插件 crate 依赖声明
- `crates/plugin-play-notification/src/lib.rs` — 插件 facade：`ohos.play-notification`、稳定 typeName、参数校验、`PlayNotificationExt`
- `plugins/play-notification/oh-package.json5` — HAR 包名 `@ohos-rs/ability-plugin-play-notification`
- `plugins/play-notification/build-profile.json5` — HAR 构建配置
- `plugins/play-notification/hvigorfile.ts` — HAR 构建入口
- `plugins/play-notification/index.ets` — 导出 `PlayNotificationPlugin`
- `plugins/play-notification/src/main/module.json5` — 模块名 `plugin_play_notification`
- `plugins/play-notification/src/main/ets/PlayNotificationPlugin.ets` — SoundPool 实现、loadComplete 等待、生命周期清理
- `Cargo.toml` — workspace 依赖从 plugin-sound 更新为 plugin-play-notification
- `AGENTS.md` — 插件列表更新
- `Cargo.lock` — 依赖解析更新

HiCodeer：

- `crates/audio/src/audio_pipeline_ohos.rs` — 实现 play_sound/end_call/init；新增 `ensure_sound_file` 运行时落盘；改为 `SoundSource::File`
- `crates/audio/Cargo.toml` — OHOS 条件依赖增加 openharmony-ability、plugin-play-notification、paths
- `crates/gpui_ohos/Cargo.toml` — openharmony-ability 相关依赖改为本地 path（联调状态）
- `crates/gpui_ohos/depend/launch-zed/Cargo.toml` — openharmony-ability 与 derive 改为本地 path（联调状态）
- `crates/gpui_ohos/src/ohos/platform.rs` — 注册 `PlayNotificationBridgePlugin`
- `hap/entry/src/main/ets/entryability/EntryAbility.ets` — 注册 `PlayNotificationPlugin`；临时自检代码已加入后删除
- `hap/build-profile.json5` — 增加 `plugin_play_notification` 模块
- `hap/entry/oh-package.json5` — 增加 `@ohos-rs/ability-plugin-play-notification` 依赖
- `hap/entry/oh-package-lock.json5` — ohpm 依赖解析更新
- `script/bundle-ohos` — 曾加入 rawfile 复制，最终移除
- `.gitignore` — 曾加入 rawfile 规则，最终移除
- `Cargo.lock` — 依赖来源从 git 改为本地 path（联调状态）

## 后续事项

- 完整 Rust 构建后，验证运行时落盘路径 `paths::data_dir()/cache/sounds/` 的写权限与播放
- 后台播放是否受鸿蒙后台音频策略限制，需要实测
- 视觉通知尚未实现；当前讨论的方案是应用内 `StatusToast` + 「打开」按钮，或新开发右上角 overlay
- openharmony-ability 上传 GitHub 后，需要把 HiCodeer 的本地 path 依赖切回 git 并更新 `Cargo.lock`
- 相关经验：[[ohos-debug-lessons]]
