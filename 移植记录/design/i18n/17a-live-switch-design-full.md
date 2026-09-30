> **附录说明（收录时补注）**
>
> 本文是 `2026-09-29` 一次**只读设计调研**的**原稿全文**，收录自 `/storage/Users/currentUser/tmp/i18n-live-switch-design.md`（原文 657 行），正文**逐字节保留**，未做任何改写。
>
> 阅读须知：
> - 正文里的 **`file:line`、行号、统计数字都是 2026-09-29 当时的快照**；此后仓库完成了全量本地化回注与骨架接入，目标文件内容可能已变。**落手前必须重读目标文件当前内容**（原稿第 8 节「风险 7」已提示这一点）。
> - 正文**第 0 节「观察到的仓库改写现象」**记录的是当时另一进程正在全量回注的临时状态，**该现象已结束**，不代表仓库现状。
> - 结论级摘要见 [`17-locale-switch-and-live-reload.md`](17-locale-switch-and-live-reload.md)；**本文是代码级施工图**（含逐文件改动清单 A/B/C、7 条风险与回退手段、验证判据）。
---

# 设置里选语言 + 切换后实时生效：可执行技术方案

只读调研，全程未写/改/删仓库与 zed-i18n 任何文件，未跑任何构建，未碰设备。
证据均来自直读代码，附 file:line 与原文。

## 0. 观察到的仓库改写现象（不干预，仅记录）

- git status --short 计数：465 个条目（M/??）为快照
- 最近 30 分钟内被修改的 .rs：363 个；最新为 crates/zed/src/zed/migrate.rs（约 5.2 分钟前）
- 与「另一个 worker 正在全量回注」一致。以上数字随 worker 推进而变，非稳定事实

---

## 1. localization 运行时模型（决定实时切换可行性）

### 1.1 registry 的确切类型与初始化方式

crates/localization/src/localization.rs:103（与 tmp/zed-i18n/tools/zed_i18n/runtime_overlay/crates/localization/src/localization.rs 逐字节相同，diff -q 返回 IDENTICAL）：

    static REGISTRY: OnceLock<Registry> = OnceLock::new();

- 是 std::sync::OnceLock<Registry>（第 4 行 use std::{..., sync::OnceLock}），一次性，写入后永不变更
- 初始化入口 initialize（269-320）用 REGISTRY.get_or_init(|| { ... })，第二次调用直接复用首次结果

    pub fn initialize(request: InitRequest<'_>) -> &'static InitOutcome {
        let registry = REGISTRY.get_or_init(|| {
            let system_locales = sys_locale::get_locales().collect::<Vec<_>>();
            ...
        });
        log::info!(
            "localization initialized: requested={:?}, system={:?}, resolved={}, source={:?}, fallback={:?}",
            ...
        );
        &registry.outcome
    }

- 系统语言来源硬编码在 crate 内部：sys_locale::get_locales()（271 行）。InitRequest 只能传 user_preference 与 legacy_locale，没有外部注入 system_locales 的口子

Registry 结构（95-101）：

    struct Registry {
        outcome: InitOutcome,
        locales: Box<[LocaleMetadata]>,
        messages: HashMap<Box<str>, Box<str>>,
        formats: HashMap<Box<str>, Box<[FormatSegment]>>,
        source_formats: HashMap<Box<str>, Box<[FormatSegment]>>,
    }

### 1.2 宏展开形态（最关键）

crates/localization/src/localization.rs:590-600：

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

结论（必须准）：

- 宏不是编译期常量，展开后是运行期查表调用 translate_static(...)
- 但每个调用点展开出一个独立的 static VALUE: OnceLock<&'static str>，首次求值后把结果永久固化在该 static 里
- 因此：注册表可变后，那些已经求值过的调用点仍返回旧值 —— 固化发生在宏这一层，不在 registry 这一层
- 唯一例外窗口：is_initialized() 为 false 时不缓存（597-598 行返回 $source）。测试 preinitialization_macro_result_is_not_cached（752-760）正面验证：初始化前返回 "Open Settings"，初始化后同一调用点返回 "설정 열기"

推论：实时切换在原理上可行（查表是运行期的），但当前被两层缓存合围——宏的 per-call-site OnceLock + REGISTRY 的 OnceLock。两者都要动。

### 1.3 公开 API 清单与签名

- initialize(request: InitRequest<'_>) -> &'static InitOutcome（269）
- is_initialized() -> bool（322）
- translate_static(source: &'static str) -> &'static str（326）
- lookup(source: &str) -> Option<&'static str>（333）
- format_message(source: &'static str, args: &[(&'static str, String)]) -> String（337）
- available_locales() -> &'static [LocaleMetadata]（344）
- resolved_locale() -> &'static str（351）
- system_locale() -> Option<&'static str>（358）
- preview_resolution(preference: &str) -> LocaleResolution（364）

InitRequest（61-65）：

    #[derive(Clone, Copy)]
    pub struct InitRequest<'a> {
        pub user_preference: Option<&'a str>,
        pub legacy_locale: Option<&'a str>,
    }

内部实现（242-253）：

    fn translate_static<'a>(&'a self, source: &'static str) -> &'a str {
        self.messages
            .get(source)
            .map(|translation| translation.as_ref())
            .unwrap_or(source)
    }

    fn lookup(&self, source: &str) -> Option<&str> { ... }

公开版能返回 &'static str 的原因：REGISTRY.get() 给出 &'static Registry，于是 &'a str 中 'a 提升为 'static。这条依赖「registry 永不改变」，一旦改成可变锁就失效——见第 3 节。

### 1.4 InitOutcome 字段

crates/localization/src/localization.rs:52-59：

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct InitOutcome {
        pub resolved_locale: String,
        pub system_locale: Option<String>,
        pub source: LocaleSource,
        pub persist_legacy_locale: Option<String>,
        pub fallback_reason: Option<FallbackReason>,
    }

LocaleSource（17-23）：User / LegacyMarker / System / EnglishFallback。
FallbackReason（25-32）：UnsupportedPreference / InvalidLegacyMarker / NoSupportedSystemLocale / CorruptIndex / CorruptBundle。

### 1.5 有无现成的「重载/切换」入口

没有任何。全 crate 搜索只有 initialize 一处 get_or_init，无 set_locale、无 reload、无 RwLock。语言切换被设计成「重启进程」。

### 1.6 最小改动方案（实时切换）

推荐 A 方案：RwLock<Option<&'static Registry>> + 泄漏（Box::leak）+ 宏去掉缓存。理由是它让全部 4766 处调用点零改动（返回类型仍是 &'static str）。

改动点：

1. REGISTRY 改为可替换的指针槽：

    static REGISTRY: std::sync::RwLock<Option<&'static Registry>> = std::sync::RwLock::new(None);

2. 新增 pub fn set_locale(user_preference: &str) -> bool：
   - 复用 embedded_text("index.json") + serde_json::from_str::<LocaleIndex> + embedded_text(bundle) + Registry::from_index(...)（这些是既有私有 fn，125-215、582-588）
   - 把新 Registry 用 Box::leak(Box::new(registry)) 变 &'static Registry
   - *REGISTRY.write().unwrap() = Some(leaked)，返回 true；失败（索引/包损坏）保留旧表并返回 false
3. 读路径改为：

    pub fn translate_static(source: &'static str) -> &'static str {
        REGISTRY
            .read()
            .ok()
            .and_then(|guard| *guard)
            .map(|registry| registry.translate_static(source))
            .unwrap_or(source)
    }

   其余 lookup/available_locales/resolved_locale/system_locale/preview_resolution/initialize 同法改造（都返回 &'static ...，leak 方案下签名不变）。
4. 宏去掉 per-call-site 缓存（否则改了 registry 也不生效）：

    macro_rules! localized_str {
        ($source:literal) => {{
            if $crate::is_initialized() { $crate::translate_static($source) } else { $source }
        }};
    }

影响面与代价：

- 调用点改动：0（宏签名、translate_static 签名都不变）
- 每次调用从「读 static」变成「读 RwLock + HashMap::get」。读锁无争用是 atomic 级别，HashMap::get 为 O(1)。UI 文本多为短串，量级可接受（未实测，列为未验证项）
- 内存只增不减：每次 set_locale 泄漏一份完整 Registry（约等于一个语言包解析后的 HashMap<Box<str>,Box<str>>，源 JSON 约 0.75–1.16 MB，见 assets/locales/*.json 大小）。人工低频切换可接受；若要彻底无泄漏，须换 B 方案

B 方案（无泄漏，但改动面大，不推荐）：RwLock<Registry> + 返回 Cow<'static, str> 或 String/Arc<str>。
代价：translate_static 返回类型一变，所有把结果当 &'static str 用的地方全部报错。已知直接调用点 16 处，其中 crates/settings/src/base_keymap_setting.rs:137 直接做 == 比较：

    (name == option || localization::translate_static(name) == option).then_some(value)

再叠加宏的 4766 处展开点，波及数百文件。不建议。

并发/生命周期陷阱（A 方案）：

- 必须 leak：RwLock<Registry>（不 leak）时读守卫借出的 &Registry 生命周期不可能到 'static，translate_static -> &'static str、available_locales -> &'static [LocaleMetadata] 全部无法成立。leak 后引用与锁解耦，读锁释放后返回值仍有效（这是安全的，不是悬垂）
- set_locale 写锁期间替换指针；已持有旧 &'static 的渲染帧继续指向旧泄漏表（安全，因只泄漏不释放），下一帧（配合第 4 节重绘）读到新表
- 后台线程调用 format_message/日志类翻译与 set_locale 并发：RwLock 覆盖（GPUI 前台线程之外也存在调用者）
- initialize 需保持幂等：if is_initialized() { return 现有 outcome }，否则第二次会重建并再泄漏一份

---

## 2. 工具自带的语言选择器做了什么

### 2.1 设置页选择器（UI）

crates/settings_ui/src/components/locale_picker.rs:12-27：

    pub(crate) fn locale_setting_item() -> SettingsPageItem {
        SettingsPageItem::SettingItem(SettingItem {
            title: localization::localized_str!("Display Language"),
            description: localization::localized_str!(
                "Select the language used by the Zed interface. Changes apply after restarting Zed."
            ),
            field: Box::new(SettingField {
                organization_override: None,
                json_path: Some("ui_locale"),
                pick: |settings| settings.ui_locale.as_ref(),
                write: |settings, value, _| settings.ui_locale = value,
            }),
            metadata: None,
            files: USER,
        })
    }

- 读写的设置键：ui_locale（json_path），类型 settings::UiLocale，默认 files: USER（写用户设置文件）
- UI 形态：DropdownMenu + ContextMenu（render_locale_picker，29-137）。选项 = "system" 串接 localization::available_locales()（57-61）
- 写入只做「写设置文件」（72-88），没有任何重载调用：

    let value = settings::UiLocale(tag.clone());
    let completion = settings::SettingsStore::global(cx)
        .update_settings_file_with_completion(
            <dyn Fs>::global(cx),
            move |settings, app| (write)(settings, Some(value), app),
        );

- 明确按「重启生效」设计（108-135）：

    let selected_locale = if selected == "system" {
        localization::system_locale().unwrap_or("en-US").to_owned()
    } else {
        localization::preview_resolution(&selected).resolved_locale
    };
    let restart_required = selected_locale != localization::resolved_locale();
    ...
    .when(restart_required, |row| {
        row.child(
            Button::new("restart-zed-for-ui-locale", localization::localized_str!("Restart Zed"))
                ...
                .on_click(|_, _, cx| workspace::reload(cx)),
        )
    })

workspace::reload（crates/workspace/src/workspace.rs:11786）是完整重启应用（会弹「Are you sure you want to restart?」，11804-11808），不是热切换。

### 2.2 初始化包装

crates/zed/src/zed/ui_locale.rs:7-37：

    pub fn initialize_localization(fs: Arc<dyn Fs>, cx: &mut App) {
        let user_preference = SettingsStore::global(cx)
            .raw_user_settings()
            .and_then(|user| user.content.ui_locale.as_ref())
            .map(|locale| locale.0.clone());
        let legacy_locale = legacy_marker_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|locale| locale.trim().to_owned())
            .filter(|locale| !locale.is_empty());

        let outcome = localization::initialize(localization::InitRequest {
            user_preference: user_preference.as_deref(),
            legacy_locale: legacy_locale.as_deref(),
        });
        ...persist_legacy_locale 回写...
        #[cfg(debug_assertions)]
        gpui::set_text_observer(observe_untranslated_text);
    }

- 启动时读一次 ui_locale → initialize 一次；legacy_locale 来自可执行文件旁 locales/legacy-locale 文件（39-62），用于把旧 zed-i18n 标记（sidecar 文件）迁移成设置项 ui_locale（outcome.persist_legacy_locale 回写，22-33）
- 没有监听设置变更、没有重初始化路径

### 2.3 是否已具备「设置变更 → 重新初始化」

没有。选择器只有「重启」按钮；initialize_localization 只在启动时调用一次（注入点见下）。

### 2.4 两个注册点（证明「设置里的语言选择」已现成接好）

工具注入（tmp/zed-i18n/tools/zed_i18n/apply_universal.py）：

- apply_universal.py:668-672 把 crate::components::locale_setting_item(), 插进 general_page 的 general settings section
- apply_universal.py:673-677 把 .add_basic_renderer::<settings::UiLocale>(crate::components::render_locale_picker) 插进 settings_ui.rs
- apply_universal.py:651-655 把 zed::initialize_localization(fs.clone(), cx); 插到 main.rs 的 watch_settings_files 与 handle_keymap_file_changes 之间

仓库内实际落地位置：

- crates/settings_ui/src/page_data.rs:141：crate::components::locale_setting_item(),（在 general_settings_section 内）
- crates/settings_ui/src/settings_ui.rs:556：.add_basic_renderer::<settings::UiLocale>(crate::components::render_locale_picker)
- crates/zed/src/main.rs:516：zed::initialize_localization(fs.clone(), cx);（前一行为 zed::watch_settings_files(...)，513 行为 settings::init(cx)）

结论：设置里的语言选择器是现成的、已注册的（工具自带 overlay 提供），缺的只有「实时生效」这一环。

---

## 3. ui_locale 设置项现状

### 3.1 已存在（不是要新增）

字段定义 crates/settings_content/src/settings_content.rs:329-333：

    /// The locale used by the Zed UI. `system` follows the operating system language.
    /// Changes apply after restarting Zed.
    ///
    /// Default: system
    pub ui_locale: Option<UiLocale>,

类型定义 crates/settings_content/src/settings_content.rs:366-384：

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, MergeFrom)]
    #[serde(transparent)]
    pub struct UiLocale(pub String);

    impl Default for UiLocale {
        fn default() -> Self {
            Self("system".into())
        }
    }

    impl JsonSchema for UiLocale {
        fn schema_name() -> std::borrow::Cow<'static, str> { "UiLocale".into() }
        fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
            schemars::json_schema!({ "type": "string" })
        }
    }

- 字段名：ui_locale
- 类型：Option<UiLocale>（UiLocale 是 String 的 transparent newtype）
- 默认值：字段缺省 = None；UiLocale::default() = "system"（语义上的「跟随系统」由 "system" 字符串承载）
- 已登记进 flattened_deserialize! 的 options 列表（settings_content.rs:437：title_bar, vim_mode, ..., ui_locale, feature_flags,）
- JSON Schema 手写为 {"type":"string"}，不产生枚举约束，所以下拉项完全由 locale_picker.rs 自己渲染（不依赖 schema）

所以不需要新增设置项。3.2/3.3 仅作参考对照。

### 3.2 同类设置项定义范例（若将来要加新项）

- 简单布尔范例：crates/settings_content/src/title_bar.rs:121 pub show_menus: Option<bool>,
- 带默认值的解析：crates/title_bar/src/title_bar_settings.rs:30 show_menus: content.show_menus.unwrap()
- 选新值的 UI 字段注册范例：crates/settings_ui/src/settings_ui.rs:556（.add_basic_renderer::<T>(render_fn)）

### 3.3 legacy_locale 是给谁用的

迁移用。给「旧 zed-i18n 版本把语言写在可执行文件旁的 locales/legacy-locale sidecar 文件」这一历史机制兜底：

- 读取：crates/zed/src/zed/ui_locale.rs:39-62（按 Windows/macOS/Linux 拼路径；Linux 为 <prefix>/share/zed-i18n/locales/legacy-locale）
- 优先级与回写：crates/localization/src/localization.rs:404-416（user_preference 为空时才看 legacy_locale，命中则 source = LegacyMarker 且返回 persist_legacy_locale = Some(id)）
- 回写设置项：crates/zed/src/zed/ui_locale.rs:22-33（settings.ui_locale = Some(settings::UiLocale(locale))）

即：老用户第一次跑新版本时，把 sidecar 里的语言搬到 ui_locale 设置里，之后 sidecar 不再需要。

---

## 4. 实时生效的「重绘」路径

### 4.1 既有范例

通用设置变更后的全局刷新（这是最相关的一条）——crates/settings/src/settings_store.rs:392-401：

    while let Some((settings_file, content)) = settings_streams.next().await {
        cx.update_global(|store: &mut SettingsStore, cx| {
            let result = match settings_file {
                SettingsFile::User => store.set_user_settings(&content, cx),
                SettingsFile::Global => store.set_global_settings(&content, cx),
                _ => return,
            };
            settings_changed(settings_file, result, cx);
            cx.refresh_windows();
        });
    }

即：任何用户/全局设置文件被写入并 watcher 捕获后，都会 cx.refresh_windows()。语言设置写入也在其列。

refresh_windows 语义（crates/gpui/src/app.rs:1162-1166）：

    /// Schedules all windows in the application to be redrawn. This can be called
    /// multiple times in an update cycle and still result in a single redraw.
    pub fn refresh_windows(&mut self) {
        self.pending_effects.push_back(Effect::RefreshWindows);
    }

其它范例：crates/theme_settings/src/settings.rs:575、605、612、623 等一串 cx.refresh_windows();（字体/主题类全局设置改动后刷新）。

关键补充：observe_global::<SettingsStore> 可用——crates/gpui/src/app.rs:2153-2160：

    pub fn global_mut<G: Global>(&mut self) -> &mut G {
        let global_type = TypeId::of::<G>();
        self.push_effect(Effect::NotifyGlobalObservers { global_type });
        ...
    }

settings_store.rs:393 的 cx.update_global(...) 内部会走 global_mut，于是 NotifyGlobalObservers { SettingsStore } 被排队；crates/gpui/src/app.rs:1812-1814 在 flush_effects 里派发：

    Effect::NotifyGlobalObservers { global_type } => {
        self.apply_notify_global_observers_effect(global_type);
    }

而 SettingsStore 已 impl Global（crates/settings/src/settings_store.rs:245）。所以注册 cx.observe_global::<SettingsStore>(...) 能在每次设置变更时收到回调（回调签名 impl FnMut(&mut App)，crates/gpui/src/app.rs:2200-2213）。

时序（决定挂点是否正确）：update_global 进入时先 push NotifyGlobalObservers，闭包体内 cx.refresh_windows() 后 push RefreshWindows；effects 为 FIFO，所以观察者回调先于刷新执行——在回调里 set_locale 后，紧接着的刷新就能读到新表。这正是我们要的顺序。

注意一个反直觉点：recompute_values（1365-1486）结尾不发通知，set_user_settings（959-982）也不发。通知来自 update_global 的外层包装，不是设置内容本身的变化。因此观察者回调会被任何设置变更触发，回调里必须自行比对 ui_locale 是否真的变了，避免无谓重载。

### 4.2 推荐挂点

在 crates/zed/src/zed/ui_locale.rs::initialize_localization 内追加（该文件是工具 overlay 文件，改它属「工具改造」范畴）：

1. 用 RwLock<Option<String>>（或 OnceLock 无法重置，需用 Mutex/RwLock）记住「上次生效的 user_preference」
2. 注册：

    cx.observe_global::<SettingsStore>(move |cx| {
        let pref = SettingsStore::global(cx)
            .raw_user_settings()
            .and_then(|user| user.content.ui_locale.as_ref())
            .map(|locale| locale.0.clone());
        if *last.borrow() != pref {
            *last.borrow_mut() = pref.clone();
            if let Some(pref) = pref.as_deref() {
                localization::set_locale(pref);
            }
            cx.refresh_windows();
        }
    });

优点：覆盖「在设置 UI 里改」与「用户手改 settings.json」两条路径。缺点：每次任意设置变更都进回调（比对成本可忽略）。

替代（更局部）：改 locale_picker.rs 的 dropdown 回调（70-89），在写入成功后调 set_locale + refresh_windows。缺点：只在从设置 UI 改时生效，手改 JSON 不生效；且 SettingField.write 第三参是 &App（不可变，见 crates/settings_ui/src/settings_ui.rs:116 write: fn(&mut SettingsContent, Option<T>, &App)），想在 write 里刷新不可行，得改在 toggleable_entry 的 move |_window, cx| 回调里（那里 cx 可 spawn、可刷新）。

结论：推荐 observe_global 挂点（全局正确、覆盖两条路径），locale_picker 那条作为备选。

---

## 5. 系统语言在 OHOS 上怎么来

### 5.1 已查实的通道（比预期完整）

链条已在仓库与 fork 中打通，只是最后一环没接：

1. ArkTS 启动时把系统语言作为 init context 传入——openharmony-ability-zed/native_ability/src/main/ets/ability/NativeAbility.ets:147：

    preferredLocales: context?.config?.language ?? "",

（同处 148 行 colorMode: context?.config?.colorMode ?? -1）

2. Rust 侧 init context 接收——openharmony-ability-zed/crates/ability/src/app.rs:33：

    pub preferred_locales: Option<String>,

访问器 app.rs:457-459 pub fn preferred_locales(&self) -> Option<String>。

3. 运行期配置变更也在传 language——openharmony-ability-zed/crates/ability/src/lifecycle.rs:132-170：

    let on_configuration_updated =
        env.create_function_from_closure("configuration_updated", move |ctx| {
            let configuration = ctx.first_arg::<Object>()?;
            let language = configuration.get_named_property::<String>("language")?;
            let color_mode = configuration.get_named_property::<i32>("colorMode")?;
            ...
            let configuration = crate::Configuration { language, color_mode: ..., ... };
            ...
            h(Event::ConfigChanged(conf))
    ...

Configuration 结构（openharmony-ability-zed/crates/ability/src/configuration/config.rs:3-15）：

    pub struct Configuration {
        pub language: String,
        pub color_mode: ColorMode,
        pub direction: Direction,
        pub screen_density: ScreenDensity,
        pub display_id: i32,
        pub has_pointer_device: bool,
        pub font_size_scale: f64,
        pub font_weight_scale: f64,
        pub mcc: String,
        pub mnc: String,
    }

ets 侧运行期回调：NativeAbility.ets:660 onConfigurationUpdate(newConfig)（转发给 lifecycle）。

4. 断点：gpui_ohos 只消费了 color_mode，完全没读 language。全 crates/gpui_ohos/src 搜索 config()/language/preferred_locales 仅命中两处，都是 color_mode：

- crates/gpui_ohos/src/ohos/platform.rs:556-562（appearance_from_mode）
- crates/gpui_ohos/src/ohos/window.rs:1640-1647（Event::ConfigChanged 更新 appearance）

Event::ConfigChanged 的消费点 crates/gpui_ohos/src/ohos/window.rs:1621-1658 只处理缩放/键盘重叠/color_mode，未处理语言。

### 5.2 能否复用现有配置通道传系统语言

能，且比预想更近：Configuration.language 已经在 Rust 侧（每次 ConfigChanged 都带 Event::ConfigChanged(conf)，可读 conf.language），启动期还有 preferred_locales。

要做的最小改动（仅判断可行性，本轮不改）：

- gpui_ohos 在 Event::ConfigChanged 分支里读 conf.language，并在启动时读 preferred_locales/Configuration.language，通过某个入口交给 localization
- 但 localization 目前没有接收系统语言的接口：initialize 内部硬编码 sys_locale::get_locales()（localization.rs:271），InitRequest 无 system_locales 字段
- 两条接法：
  - 接法甲（小改）：给 InitRequest 加 system_locales: Option<&str>（或 &[String]），initialize/set_locale 优先用它，缺省才回落 sys_locale::get_locales()
  - 接法乙（不改 crate）：把 OHOS 系统语言当 user_preference 传。语义会标成 LocaleSource::User（不准），且会覆盖用户「system」选择，不推荐
- 补充：sys_locale 在 OHOS 只读环境变量（LANGUAGE→LC_ALL→LC_MESSAGES→LANG），不读 HarmonyOS 系统设置。OHOS 应用进程里是否设置了这些环境变量，本轮未验证（禁设备）。若为空，get_locales() 返回空 → resolve_locale 落到 NoSupportedSystemLocale → 英文兜底（localization.rs:429-441）

结论：系统语言两条来源的改动量与风险

- ① 复用配置通道（接法甲）：改动集中在 gpui_ohos（消费 language）+ localization（新增注入字段）+ 一个把 language 交给 localization 的桥。风险：格式匹配（见 5.3）、需设备验证、须保持其他平台编译不受影响（gpui_ohos 是 OHOS 专属 crate，风险可控）
- ② 让用户在设置里手动选：改动量最小（选择器已现成），风险最低，且不依赖系统语言格式

### 5.3 一个已定位的潜在缺口（与系统语言格式相关）

crates/localization/src/localization.rs:557-580 的 match_locale：先精确匹配 id/alias，再按语言前缀匹配，且前缀匹配要求唯一命中：

    let language = normalized.split('-').next()?;
    let mut matching = locales.iter().filter(|locale| {
        normalize_locale_tag(&locale.id).split('-').next()
            .is_some_and(|locale_language| locale_language.eq_ignore_ascii_case(language))
    });
    let first = matching.next()?;
    matching.next().is_none().then_some(first)

assets/locales/index.json 里 zh-CN 的 aliases 为 ["zh","zh-Hans","zh-SG"]，zh-TW 为 ["zh-Hant","zh-HK","zh-MO"]（共 14 个 locale，en-US 无 bundle；除 locales 外 source_formats 485 条）。

- 若 OHOS 报 zh-Hans → alias 命中 → 解析为 zh-CN（OK）
- 若 OHOS 报 zh-Hans-CN → 精确匹配失败，前缀 zh 命中 zh-CN 与 zh-TW 两个 → matching.next().is_none() 为 false → 返回 None，落英文兜底（缺口）

OHOS 实际格式未验证。

---

## 6. 与其他设置的交互：菜单栏默认关闭

- 设置项定义：crates/settings_content/src/title_bar.rs:121 pub show_menus: Option<bool>,
- 默认值：assets/settings/default.json:628-629：

        // Whether to show the menus in the titlebar.
        "show_menus": false,

（同节 default.json:611 起为 "title_bar": { ... }，624-625 "show_user_menu": true）

- 运行时读取：crates/title_bar/src/title_bar_settings.rs:14 pub show_menus: bool,（30 行 show_menus: content.show_menus.unwrap()）；crates/title_bar/src/application_menu.rs:315-317：

    pub(crate) fn show_menus(cx: &mut App) -> bool {
        TitleBarSettings::get_global(cx).show_menus
    }

- 设置 UI 的键路径：crates/settings_ui/src/page_data.rs:4474 json_path: Some("title_bar.show_menus")

结论：键名 title_bar.show_menus，默认 false。用户切到中文后，标题栏菜单默认不显示；要看到中文菜单项必须先打开该开关。语言选择器应一并提示用户这一点（属 B 类体验建议）。

---

## 7. 改动清单（按文件）

### A. 必须改（不做就不生效）

- crates/localization/src/localization.rs:103
  - 改：static REGISTRY: OnceLock<Registry> → static REGISTRY: RwLock<Option<&'static Registry>>（或等价可替换槽）
  - 为什么：OnceLock 一次性，无法换表
- crates/localization/src/localization.rs:590-600（localized_str!）
  - 改：删掉 per-call-site static VALUE: OnceLock<&'static str> 缓存，直接走 $crate::translate_static($source)
  - 为什么：这是实时切换能否生效的决定性一处；不清缓存则切换后所有已渲染调用点仍返回旧值
- crates/localization/src/localization.rs:242-247（内部 translate_static）/ 326-331（公开版）
  - 改：读路径经 RwLock read + *guard 取 &'static Registry
  - 为什么：返回类型靠 leaked &'static Registry 维持，translate_static -> &'static str 签名不变，从而调用点零改动
- crates/localization/src/localization.rs（新增）
  - 改：新增 pub fn set_locale(user_preference: &str) -> bool（复用 embedded_text+LocaleIndex+Registry::from_index，Box::leak 后写 REGISTRY）
  - 为什么：提供唯一的切换入口；无它则只能重启
- crates/localization/src/localization.rs:344-373（available_locales/resolved_locale/system_locale/preview_resolution）
  - 改：同上改走 RwLock 读
  - 为什么：这些是选择器与状态查询的依赖；不改则锁改造不完整（编译期即报错）
- crates/zed/src/zed/ui_locale.rs:7-37
  - 改：在 initialize_localization 内注册 cx.observe_global::<SettingsStore>，比对 ui_locale 变化 → 调 localization::set_locale + cx.refresh_windows()
  - 为什么：唯一能把「设置变更」接到「换表 + 重绘」的既有全局钩子（4.1/4.2 已证可用）

### B. 建议改（体验相关）

- crates/settings_ui/src/components/locale_picker.rs:15-17
  - 改：描述文案由「Changes apply after restarting Zed.」改为「立即生效」类表述
  - 为什么：实时生效后原文案会误导
- crates/settings_ui/src/components/locale_picker.rs:113-135
  - 改：restart_required 判定与「Restart Zed」按钮改为只在真正需要重启的场景出现（或整段移除）
  - 为什么：实时生效后残留的「重启」按钮会让用户以为没生效
- crates/settings_ui/src/components/locale_picker.rs（或同页说明）
  - 改：若切到非英文，提示「标题栏菜单需开启 title_bar.show_menus 才可见」
  - 为什么：该开关默认 false（第 6 节），否则用户看不到中文菜单项
- crates/localization/src/localization.rs:557-580（match_locale）
  - 改：前缀匹配命中多候选时，对 zh-Hans-* / zh-Hant-* 之类做脚本子标签消歧，或放宽为「首个候选」
  - 为什么：系统语言若为 zh-Hans-CN 会解析失败（5.3）；仅在接入系统语言后才有感

### C. 可选（系统语言接入）

- crates/gpui_ohos/src/ohos/window.rs:1621-1658（Event::ConfigChanged）
  - 改：在 color_mode 之后追加读取 conf.language，经桥交给 localization
  - 为什么：这是系统语言在运行期到达 Rust 侧的唯一事件
- crates/gpui_ohos/src/ohos/platform.rs（启动期）
  - 改：读 preferred_locales() / Configuration.language，作为初始系统语言
  - 为什么：启动期配置变更事件尚未到达，需 init 路径兜底
- crates/localization/src/localization.rs:61-65（InitRequest）
  - 改：加 system_locales: Option<&str>（或 &[String]），initialize/set_locale 优先使用，缺省回落 sys_locale::get_locales()
  - 为什么：initialize 现在硬编码 sys_locale（271），OHOS 上读不到系统设置；不开口子就接不进系统语言

---

## 8. 风险清单

每条给「会发生什么 + 怎么发现 + 怎么回退」。

- 风险 1：set_locale 泄漏内存累积
  - 会发生什么：每次切换泄漏一份 Registry（约一个语言包解析结果，源 JSON 0.75–1.16 MB 量级），反复切换内存只增不减
  - 怎么发现：切换 N 次后观察进程 RSS 增幅 ≈ N × 单包解析量；或统计 set_locale 调用次数与各包大小
  - 怎么回退：切到 B 方案（RwLock<Registry> + 返回 Cow/String），代价是调用点大范围改动；或加「同一 preference 不重复 set_locale」的去重（已在 4.2 的 last 比对里体现）
- 风险 2：去掉宏缓存后渲染路径性能下降
  - 会发生什么：每次 localized_str! 求值由「读 static」变为「RwLock 读 + HashMap::get」；高频渲染点（列表/弹窗）可能变慢
  - 怎么发现：对比改造前后同场景帧耗时/CPU（profiler，见 instrumentation 设置）；或对最高频调用点计数
  - 怎么回退：保留 per-call-site 缓存但加「代际号（epoch）校验」：static CELL: (AtomicU64, RwLock<&'static str>)，epoch 变化时重算。成本是每次一次 atomic load
- 风险 3：observe_global::<SettingsStore> 回调被任意设置变更触发，误重载
  - 会发生什么：改任何设置都可能触发一次语言重载（若比对逻辑写错）
  - 怎么发现：日志里 set_locale 调用次数与「非语言设置变更次数」相关（可在 set_locale 入口加 info 日志观察）
  - 怎么回退：回调内严格比对规范化后的 preference（None/"system"/具体 id 三态），不相等才重载
- 风险 4：系统语言格式不匹配（5.3 缺口）
  - 会发生什么：OHOS 报 zh-Hans-CN 时中文系统落英文兜底
  - 怎么发现：启动日志 localization initialized: system=..., resolved=en-US, fallback=NoSupportedSystemLocale（localization.rs:311-318 已打印这些字段）
  - 怎么回退：改 match_locale 消歧；或临时让用户手动选
- 风险 5：set_locale 与渲染并发读
  - 会发生什么：换表瞬间后台线程仍在读旧表
  - 怎么发现：RwLock 旨在消除该风险；若出现异常应表现为文本混排（不得截图，改用日志/用户反馈）
  - 怎么回退：leaked Registry 保证旧引用永不悬垂（只泄漏不释放），天然安全；若怀疑锁误用则回到「单线程内切换」约束
- 风险 6：改动 gpui_ohos 影响其他平台编译
  - 会发生什么：gpui_ohos 是 OHOS 专属 crate（Cargo.toml:99-103 workspace members，gpui_ohos/Cargo.toml:17 [target.'cfg(target_env = "ohos")'.dependencies]），改它只影响 OHOS 目标
  - 怎么发现：bundle-ohos 编译（非 OHOS 目标不编译该 crate）
  - 怎么回退：[OHOS PORT BEGIN/END] 注释对包裹新增段，便于整体撤除
- 风险 7：仓库正被另一个 worker 全量改写
  - 会发生什么：本方案给出的行号（如 localization.rs:103/590）可能因 overlay 更新而位移；git status 465 项、localized_str! 4766 次等统计是时刻快照
  - 怎么发现：git status、文件 mtime、localized_str! 计数变化
  - 怎么回退：落手前先重读目标文件当前内容，以 file:line + 原文为准（不要按旧行号盲改）

---

## 9. 验证判据（禁截图）

编译判据（本轮未跑，属既定授权范围，落手后即跑 debug）：

- script/bundle-ohos（debug，不是 --release，见全局规则 17）
- 判据：命令退出码 0，且日志出现 HAP BUILD SUCCESSFUL
- 编译期反向判据：若 translate_static 返回类型与调用点不兼容，会在 cargo check/bundle-ohos 阶段报错（A 方案下应无此类错误——这正是选择它的原因）

运行时判据（禁止截图，用日志与状态查询）：

- 启动：crates/localization/src/localization.rs:311-318 的 localization initialized: requested=..., system=..., resolved=..., source=..., fallback=... 与设置值一致
- 切换：在 set_locale 成功路径加 info 日志（打印 user_preference 与 new resolved_locale），切换后应出现一条 resolved 变化的新日志，且同一进程内（无重启）
- 交叉验证：切换后触发一次设置文件写入 → settings_store.rs:400 的 refresh_windows 生效
- 未译文本观察（debug 版）：crates/zed/src/zed/ui_locale.rs:64-85 的 set_text_observer，切换后若出现 untranslated accepted UI text reached GPUI layout，说明该文本未进入语言包
- 菜单项：确认 title_bar.show_menus（page_data.rs:4474，默认 false）打开后菜单项显示为目标语言
- 如需界面确认：向用户索取截图，不自行截图（全局规则 13）

---

## 10. 未验证项（逐条如实列出）

- OHOS 应用进程内是否设置了 LANG/LC_ALL/LANGUAGE（决定 sys_locale::get_locales() 是否返回非空）——本轮禁设备，未验证
- OHOS Configuration.language 的实际字符串（是 zh-Hans 还是 zh-Hans-CN 等）——未验证；影响 5.3 缺口是否真实触发
- preferredLocales 在启动时是否确实被 ets 填值（源码为 context?.config?.language ?? ""，NativeAbility.ets:147）——源码证据在，运行时取值未在设备验证
- OHOS 上报的 language 与 assets/locales/index.json 的 aliases 是否覆盖（14 个 locale；zh-CN aliases zh,zh-Hans,zh-SG；zh-TW aliases zh-Hant,zh-HK,zh-MO）——未验证
- set_locale 泄漏方案的单次内存增量的实测数值——未实测
- 去掉宏缓存后 4766 处（当前快照）调用点的实际性能影响——未实测
- 我没有在当前仓库编译或运行任何东西；未执行 apply/extract/generate-runtime-bundles；未用 hdc；未写仓库任何文件
- 仓库正被另一 worker 改写中：git status=465、近 30 分钟 363 个 .rs 被改、最新 crates/zed/src/zed/migrate.rs（约 5.2 分钟前）。所有涉及「当前规模」的数字均为快照

---

## 11. 总结论

一句话：「设置里选语言」是现成的（工具自带 overlay 的 locale_setting_item 已注册进 General 设置页，ui_locale 设置项也已存在），「实时生效」原理可行但当前被两层缓存设计成「重启生效」（宏的 per-call-site OnceLock<&'static str> + REGISTRY 的 OnceLock），最省事的实现路径是：把 REGISTRY 换成 RwLock<Option<&'static Registry>>（新表 Box::leak 成 &'static）、宏去掉 per-call-site 缓存、新增 set_locale、在 initialize_localization 里用 observe_global::<SettingsStore> 比对 ui_locale 变化后调 set_locale + cx.refresh_windows()——这样 4766 处调用点零改动（返回类型仍是 &'static str），代价是每次切换泄漏一份 registry（人工低频切换可接受）。系统语言接入列为可选（C 类）：通道其实已在（Configuration.language + Event::ConfigChanged），只是 gpui_ohos 尚未消费它。
