# 第 17 章 语言切换与「实时生效」设计

本章把「设置里选语言」的现状与「切换后实时生效」的设计方案并进仓库。**用户已决定：本阶段先做「重启生效」，不做实时切换**——省掉大半工作量。

> 素材来源：只读设计产出 `/storage/Users/currentUser/tmp/i18n-live-switch-design.md`（实测 **657 行**）。本章是**提炼**（机制 / 改动点 / 坑），不是原文搬运。该设计文档本身只读调研，未写/改任何仓库文件、未跑构建、未碰设备。

> **完整施工图**：本章是结论级提炼；代码级原稿（逐文件改动清单、7 条风险与回退手段、验证判据）见 [17a-live-switch-design-full.md](17a-live-switch-design-full.md)。

## 17.1 结论先行

- **设置里的语言选择是现成的**（工具自带 `locale_picker.rs`，已注册进设置页；`ui_locale` 设置项已存在）。
- 它**被设计成「重启生效」**：描述文案是「Changes apply after restarting…」，旁边就是一个 **Restart** 按钮。
- **本轮只做「重启生效」**——因为宏展开是**运行期查表**，重启后重新初始化即可生效；实时切换的复杂度（两层缓存都要改）**不值得在此时做**。
- **实时切换的方案已记录在案**（第 17.4 节），但**明确标注「当前不实施」**，供将来按需启用。

## 17.2 设置里的语言选择（现成，已注册）

- **选择器组件**：`crates/settings_ui/src/components/locale_picker.rs`（实测 163 行）。
  - `locale_setting_item()`（约 `:12-27`）产出一个 `SettingsPageItem`：
    - `title` = `localized_str!("Display Language")`；
    - `description` = `localized_str!("Select the language used by the Zed interface. Changes apply after restarting Zed.")`；
    - `field.json_path = Some("ui_locale")`，读写 `settings.ui_locale`。
- **注册点**（实测本机存在）：
  - `crates/settings_ui/src/page_data.rs:141`：`crate::components::locale_setting_item(),`（在 General 设置页的 general section 内）。
  - `crates/settings_ui/src/settings_ui.rs:556`：`.add_basic_renderer::<settings::UiLocale>(crate::components::render_locale_picker)`。
- **设置项 `ui_locale`**（已存在，不是新增）：
  - 字段 `pub ui_locale: Option<UiLocale>,`（`crates/settings_content/src/settings_content.rs:333`）。
  - 类型 `UiLocale(pub String)`（`settings_content.rs:368`），默认 `"system"`。
  - 已登记进 `flattened_deserialize!` 的 options 列表（`settings_content.rs:437`）。
- **写入行为**：`locale_picker` 的 dropdown 回调**只做「写设置文件」**（`update_settings_file_with_completion`），**没有任何重载调用**；`restart_required` 时显示一个 **Restart** 按钮（走 `workspace::reload`，即**完整重启应用**，不是热切换）。

**结论**：设置里的语言选择**无需新增**，本轮要做的是让 `ui_locale` 的默认值为 `"zh-CN"`（见第 16 章）。

## 17.3 初始化链路与「重启生效」为什么够

- **初始化入口**：`crates/zed/src/zed/ui_locale.rs:7-37` 的 `initialize_localization(fs, cx)`：
  - 读 `SettingsStore::global(cx).raw_user_settings().content.ui_locale` 作为 `user_preference`（`:8-11`）；
  - 调 `localization::initialize(InitRequest { user_preference, legacy_locale })`（`:17-20`）；
  - `legacy_locale` 来自可执行文件旁的 sidecar 文件（`:39-62`，迁移旧机制用）。
- **调用点**：`crates/zed/src/main.rs:516`：`zed::initialize_localization(fs.clone(), cx);`（上一行 515 是 `watch_settings_files`）。
- **为什么「重启生效」就够——宏是运行期查表，不是编译期常量**：
  - 宏定义原文（`crates/localization/src/localization.rs:590-600`）：

```rust
#[macro_export]
macro_rules! localized_str {
    ($source:literal) => {{
        static VALUE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
        if $crate::is_initialized() {
            *VALUE.get_or_init(|| $crate::translate_static($source))
        } else {
            $source
        }
    }};
}
```

  - 展开后是**运行期调用 `translate_static($source)` 查表**（不是把译文烤进二进制）。所以**只要重启后重新初始化 registry，读到的就是新语言**——重启生效成立。

## 17.4 实时切换方案（记录在案，**当前不实施**）

若将来要「设置里改完立即生效」，方案要点（提炼自只读设计产出）：

- **必须动两处缓存**（缺一不可）：
  1. **registry 是 `OnceLock`**：`static REGISTRY: OnceLock<Registry> = OnceLock::new();`（`crates/localization/src/localization.rs:103`）——**首次初始化定终身**。要改成可替换结构，例如 `RwLock<Option<&'static Registry>>`（新表用 `Box::leak` 提升为 `&'static`，从而 `translate_static` 等仍返回 `&'static str`，**调用点零改动**）。
  2. **真正的固化元凶：宏里的 per-call-site 缓存**——`static VALUE: std::sync::OnceLock<&'static str>`（`localization.rs:593`）**必须删掉**。它在每个调用点把「首次查到的译文」永久固化；registry 改成可替换后，**已求值过的调用点仍会返回旧值**。删除后宏直接走 `$crate::translate_static($source)` 即可。
- **新增 `set_locale(user_preference) -> bool`**：复用既有私有构建路径重建一份 registry，`Box::leak` 后写入可替换槽；失败（索引/包损坏）保留旧表并返回 false。
- **切换后触发全局重绘**：用现有范例——`crates/settings/src/settings_store.rs:392-401` 的 settings 变更处理里会 `cx.refresh_windows();`（任何设置文件写入并被 watcher 捕获后都会刷新所有窗口）。更稳的挂点是 `cx.observe_global::<SettingsStore>(...)` 里比对 `ui_locale` 是否真变了，变了才 `set_locale` + `refresh_windows()`。
- **代价与收益**：
  - **4766 处调用点零改动**（宏与 `translate_static` 的签名都不变）；
  - **每次切换泄漏一份 registry**（只泄漏不释放；源语言包约 0.75–1.16 MB/份）。人工低频切换**可接受**。

## 17.5 系统语言自动跟随（可选增强）

- **通道早已铺好，只是没被消费**：
  - ArkTS 侧启动就把系统语言塞进 init context——**本机实测** `hap/entry/oh_modules/@ohos-rs/ability/src/main/ets/ability/NativeAbility.ets:146`：
    `preferredLocales: context?.config?.language ?? "",`
    （紧随 `:147` 是 `colorMode: context?.config?.colorMode ?? -1,`）。
    > 行号差异如实标注：只读设计产出引用 `NativeAbility.ets:147`；本机实测为 **:146**，且路径形态不同（本机是 `oh_modules` 依赖，不是设计产出引用的 `openharmony-ability-zed/...` 源仓库路径）。
  - 运行期配置变更也在读 `language`——只读设计产出引用 `openharmony-ability-zed/crates/ability/src/lifecycle.rs:132-170`。
    > **未核实**：本机**不存在** `openharmony-ability-zed/**`、`crates/ability/**`、`**/lifecycle.rs`（实测 glob 命中 0）。设计产出引用的是上游 fork 的相对路径，本机核不到。
  - **断点**：`crates/gpui_ohos/` **只消费了 colorMode，从来没消费语言**——实测：
    - `crates/gpui_ohos/src/ohos/platform.rs:556-562`（`appearance_from_mode(color_mode)`，只看 color_mode）；
    - `crates/gpui_ohos/src/ohos/window.rs:1621-1657`（`Event::ConfigChanged` 分支只更新 color_mode / 缩放 / 键盘重叠）；全 `crates/gpui_ohos/src` 搜 `language` / `preferred_locales` 无命中。
- **一个已定位的坑：`match_locale` 前缀歧义**。
  - `crates/localization/src/localization.rs` 的 `match_locale` 先精确匹配 id/alias，再按**语言前缀**匹配，且**前缀匹配要求唯一命中**。
  - `assets/locales/index.json` 里 `zh-CN` 的 aliases 含 `zh` 等；若 OHOS 上报 **`zh-Hans-CN`**：精确匹配失败 → 前缀 `zh` **同时命中 `zh-CN` 与 `zh-TW` 两个** → 「唯一命中」条件不成立 → **匹配失败、回落英文**。
  - 若上报 `zh-Hans`：alias 命中 → 解析为 `zh-CN`（正常）。
  - **OHOS 实际上报格式未验证**（禁设备），故该坑是否真实触发**未核实**。

## 17.6 未验证项（逐条如实列出）

- OHOS 应用进程内是否设置了 `LANG` / `LC_ALL` / `LANGUAGE`（决定 `sys_locale::get_locales()` 是否非空）——未验证。
- OHOS `Configuration.language` 的实际字符串格式（`zh-Hans` 还是 `zh-Hans-CN`）——未验证；影响 17.5 的歧义坑是否触发。
- 本机 `hap/entry/oh_modules/.../NativeAbility.ets` 与上游 fork 源仓库的对应关系——只按同名文件比对，**未核实**两者是否同版本。
- 实时切换方案的「单次切换内存增量」「去掉宏缓存后 4766 处调用点的性能影响」——均**未实测**（当前不实施，不需要实测）。
