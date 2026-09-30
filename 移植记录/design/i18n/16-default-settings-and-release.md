# 第 16 章 默认配置本地化与 release 构建

用户本轮要求：**把默认配置改成中文，并加一组默认项**，且**「格式要合规，对着代码确认」**。本章记录「怎么确认格式合规」这套可复用的做法，以及 release 构建与装包环节。

> 时间戳说明：`assets/settings/default.json` 在 **2026-09-29 22:47:17**（实测 mtime）仍被另一个 worker 改动（与本轮的 release 构建并行）。本章所有对它的读数均为**该时刻快照**；若数值与预期不同，以「文件正被另一 worker 改动、读数可能为中间态」为准，不下判断。

## 16.1 用户要求的默认项（原样列出 + 实测状态）

用户原文列出的赋值项如下。**注意：原文列了 7 个赋值项，但称为「8 项」——本文如实按原文列出 7 项，差异（第 8 项未在原文出现）标注为「未核实」。**

- `ui_locale = "zh-CN"`
- `agent.limit_content_width = false`
- `agent.favorite_models = []`
- `agent.model_parameters = []`
- `ui_font_size = 16`
- `buffer_font_size = 15`
- `theme = { mode: "system", light: "One Light", dark: "One Dark" }`

实测状态（2026-09-29 22:48 读 `assets/settings/default.json`，mtime 22:47:17）：

- `"ui_locale": "zh-CN",` —— 在 **第 3 行**（紧跟 `$schema` 之后）。
- `"buffer_font_size": 15,` —— **第 42 行**。
- `"ui_font_size": 16,` —— **第 72 行**。
- `"agent.limit_content_width": false,` —— **第 1183 行**（`agent` 段内；第 1180-1182 行是它的注释）。
- `"favorite_models": [],` —— **第 1197 行**（`agent` 段内）。
- `"model_parameters": [ ... ]` —— **第 1201-1219 行**；方括号内**全部是注释**，即语义上的**空数组**。
- `"theme": { "mode": "system", "light": "One Light", "dark": "One Dark" },` —— **第 10-14 行**。
- 另：`"show_menus": false,` —— **第 629 行**（本次未要求改，但见 16.7 的误判点）。

**结论**：截至上述时刻，用户要求的 **7 项赋值全部出现在 `default.json` 中**；但这是**另一个 worker 正在写入**的文件，**「已随 release 包生效」尚待装包后确认**（进行中）。

## 16.2 默认配置文件的路径与加载链路（file:line）

- **文件**：`assets/settings/default.json`（仓库内相对路径）。
- **加载入口**：`crates/settings/src/settings.rs:135-137`

```rust
pub fn default_settings() -> Cow<'static, str> {
    asset_str::<SettingsAssets>("settings/default.json")
}
```

- **解析点**：`crates/settings/src/settings_file.rs:106`

```rust
crate::parse_json_with_comments::<serde_json::Value>(crate::default_settings().as_ref())
```

- **初始化挂点**：`crates/settings/src/settings.rs:129-132`（`init` 里用 `SettingsStore::new(cx, &default_settings())` 注册全局）。

## 16.3 解析器是什么（决定「尾随逗号要不要去」）

解析器是 **`serde_json_lenient`**，不是严格的 `serde_json`：

- 定义在 `crates/settings_json/src/settings_json.rs:784-789`：

```rust
pub fn parse_json_with_comments<T: DeserializeOwned>(content: &str) -> Result<T> {
    let mut deserializer = serde_json_lenient::Deserializer::from_str(content);
    let value = serde_path_to_error::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(value)
}
```

- **结论**：
  - **允许注释**（`//`）——所以 `default.json` 里满篇注释合法。
  - **允许尾随逗号**——`serde_json_lenient` 的设计目标就是「宽松 JSON」；实测文件本身就大量使用尾随逗号（例如第 13 行 `"dark": "One Dark",` 后紧跟 `}`；第 1194 行 `"enable_thinking": false,` 后紧跟 `}`）。
  - **所以用户给的片段里那个尾随逗号不必去掉**——与文件既有风格一致即可。
- **核对方法（可复用）**：先在 `crates/settings*/` 里搜 `default_settings()` 找到加载函数，再顺藤摸到 `parse_json_with_comments`，最后定位到它用的是哪个 JSON 解析 crate（`serde_json_lenient` vs `serde_json`）。**解析器决定语法的宽松度**，这一步不能跳。

## 16.4 未知字段是静默忽略还是报错

- 实测：在 `crates/settings_content/src/` 全目录搜 `deny_unknown_fields`，**出现 0 次**。
- 结论：serde 的默认行为生效——**未知字段被静默忽略，不报错**。
- 含义：往 `default.json` 里写错一个键名，**不会在启动时报错**，只会「该项没生效」。所以**一定要对着 Rust 结构体核字段名**（见 16.5），不能指望解析器替你喊。

## 16.5 逐项核对：字段名 / 类型 / 合法取值

**核对方法（可复用）**：对每个设置项，先按「项目名 → Rust 结构体」定位到 `crates/settings_content/src/` 里的字段声明（多为 `Option<T>`），再看 `T` 的定义（newtype？enum？`Vec<...>`？），确认：
1. **字段名**与 JSON 键**逐字相同**（注意 `#[serde(rename = "...")]`）；
2. **类型**能接受该 JSON 字面量（整数能否进 `f32`？空数组元素类型？）；
3. **枚举值**是合法变体（注意 `#[serde(rename_all = "snake_case")]`）。

逐项结论（均给出 `file:line`）：

- **`ui_locale`**：
  - 字段 `pub ui_locale: Option<UiLocale>,`（`crates/settings_content/src/settings_content.rs:333`）。
  - `UiLocale` 是 `String` 的透明 newtype（`settings_content.rs:368`），默认值 `"system"`。
  - → `"zh-CN"` 合法（任意字符串都合法，是否为受支持 locale 由运行时 `match_locale` 判定）。
- **`agent.limit_content_width`**：
  - `pub limit_content_width: Option<bool>,`（`crates/settings_content/src/agent.rs:271`）。
  - → `false` 合法。
- **`agent.favorite_models`**：
  - `pub favorite_models: Vec<LanguageModelSelection>,`（`agent.rs:283`）；`LanguageModelSelection` 定义在 `agent.rs:655-662`。
  - → `[]` 合法（空数组不涉及元素类型构造）。
- **`agent.model_parameters`**：
  - `pub model_parameters: Vec<LanguageModelParameters>,`（`agent.rs:339`）；`LanguageModelParameters` 定义在 `agent.rs:666-671`。
  - → `[]` 合法。
- **`ui_font_size` / `buffer_font_size`**：
  - `pub ui_font_size: Option<FontSize>,`（`crates/settings_content/src/theme.rs:180`）。
  - `pub buffer_font_size: Option<FontSize>,`（`theme.rs:199`）。
  - `FontSize` 是 `pub f32` 的透明 newtype（`theme.rs:259`，`#[serde(transparent)]`）。
  - → JSON 整数 `16`/`15` 可反序列化为 `f32`，合法。
- **`theme`**：
  - 字段 `pub theme: Option<ThemeSelection>,`（`theme.rs:218`）。
  - `ThemeSelection` 是 enum（`theme.rs:326-339`）：`Static(ThemeName)` 或 `Dynamic { mode: ThemeAppearanceMode, light: ThemeName, dark: ThemeName }`。
  - `ThemeAppearanceMode`（`theme.rs:402-413`，带 `#[serde(rename_all = "snake_case")]`）只有三个变体：`Light` / `Dark` / `System`（`System` 带 `#[default]`）。
  - 主题名合法：`pub const DEFAULT_LIGHT_THEME: &'static str = "One Light";`（`theme.rs:341`）、`pub const DEFAULT_DARK_THEME: &'static str = "One Dark";`（`theme.rs:342`）。
  - → `{ mode: "system", light: "One Light", dark: "One Dark" }` **完全匹配** `Dynamic` 变体：字段名 `mode`/`light`/`dark` 逐字对应，`"system"` 命中 `System` 变体，两个主题名是内置常量。**合法**。

**未核实项**：`ThemeName` 类型（`ThemeSelection.light/dark` 的元素类型）本轮**未逐字核对**其定义位置——但主题名字符串 `"One Light"`/`"One Dark"` 已由 `theme.rs:341-342` 的常量证实存在，故取值合法性不受影响。

## 16.6 为什么必须显式写 `zh-CN`（不能依赖 `"system"`）

- 依据第 9 章结论：`sys-locale` 在 OHOS 上走 `unix` 分支，**只读四个环境变量**（`LANGUAGE` / `LC_ALL` / `LC_MESSAGES` / `LANG`），**不读 HarmonyOS 系统设置**。
- OHOS 应用进程若**没有**这些环境变量，`sys_locale::get_locales()` 返回**空** → 系统语言为 `None` → **回落 `en-US`**。
- 因此默认值若为 `"system"`，会落回英文；**必须显式写 `"zh-CN"`** 才能让中文默认生效。

## 16.7 两条必须记住的误判点

1. **用户级 `settings.json` 会覆盖默认值。** `default.json` 只是**默认**——只对「**没设过该项**的用户」生效。任何在用户级 `settings.json` 里写过 `ui_locale`（或其它项）的用户，其值**优先**。所以「装了新版却还是英文」的常见原因是**用户自己那份 settings 里覆盖了**，不是默认项没写进去。
2. **`title_bar.show_menus` 默认 `false`。**
   - 定义：`pub show_menus: Option<bool>,`（`crates/settings_content/src/title_bar.rs:121`）。
   - 默认值：`assets/settings/default.json:629` 为 `"show_menus": false,`。
   - 含义：**切到中文后，标题栏默认不显示带文字的菜单栏**——要点开 **☰** 看弹出菜单，或先把该设置打开。这是「装完以为汉化没生效」最常见的误判点。
   - 相关设置键路径：`title_bar.show_menus`（设置页 `crates/settings_ui/src/page_data.rs:4474`）。

## 16.8 release 构建与装包

### 16.8.1 本轮要求编译 release

- 用户本轮**明确要求**编译 release，因此本轮的 `--release` 属**已授权**动作（对照全局规则第 17 条：默认禁止，但用户明确允许时不在此列）。
- **判据**：命令**退出码 0** **且**日志出现**构建成功标志** `=== HAP BUILD SUCCESSFUL ===`。
- **clean 仍然禁止**（全局规则第 12 条）：release 构建里**不得**掺入任何 `clean` / 清缓存动作。
- **当前状态（2026-09-29 22:49，只读观察）**：release 构建**正在跑**：
  - 进程 `bash ./script/bundle-ohos --release`（PID 54146）；
  - 子进程 `cargo.real build --release --lib -p launch-zed --target aarch64-unknown-linux-ohos`（PID 54216）；
  - 多个 `rustc` 正在并行编译各 crate。
  - **耗时、退出码、是否出现成功标志——全部「进行中，结果待补」。**
- 对照旁证：本轮回注后的 **debug** 构建已成功（`tmp/bundle-ohos-after-i18n.log`，20:11，`HAP BUILD SUCCESSFUL` + `EXITCODE=0`）。

### 16.8.2 装包要核实的点（`install-local.sh` 是否只认 debug 产物）

- **实测结论：不是只认 debug 产物。**
  - `install-local.sh:44` 的默认产物路径写死为 `hap/entry/build/default/outputs/default/entry-default-signed.hap`（**稳定名**）。
  - `script/bundle-ohos:1010` 签名产物也写死到**同一路径** `$HAP_DIR/entry/build/default/outputs/default/entry-default-signed.hap`（注释明确写着「Sign straight into the stable name install-local.sh expects」）。
  - `script/bundle-ohos:204-208` 用 `BUILD_MODE`（`debug` / `release`）区分构建模式，但**最终 HAP 落点不随模式变化**——release 构建会**覆盖**同名文件。
- **所以**：release 构建成功后，直接 `./install-local.sh`（不带任何卸载参数）即可覆盖安装；若有路径不符，可用 `./install-local.sh --hap <path>` 或 `hdc install -r <hap>` 覆盖安装（**两者都不许卸载**，见全局规则第 21 条）。
- **当前状态**：**未装包**（进行中）。

> 全局规则提醒：装包**严禁** `--reinstall` 或任何卸载动作（会清空沙箱、丢失用户手工配置）。
