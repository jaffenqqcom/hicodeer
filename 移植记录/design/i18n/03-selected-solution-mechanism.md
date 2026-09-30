# 第 3 章 选定方案的机制

这一章要把 `LI-NA/zed-i18n` 的机制讲透，因为后面所有步骤（重提取、回注、生成语言包）都围绕它。

## 3.1 机制三件套

1. **新增一个独立 crate `localization`**（不是满篇改字面量）。
   - 它来自 `zed-i18n` 的 `tools/zed_i18n/runtime_overlay/crates/localization/`（**注意路径是下划线 `zed_i18n`，且嵌在 Python 包里**）。
   - 仓库内落地为 `crates/localization/`（`Cargo.toml` + `src/localization.rs`）。
2. **回注时把英文字面量包成 `localization::localized_str!("...")`**（宏查表）。
   - `localized_str!` 是一个宏（`crates/localization/src/localization.rs:591`），运行时按当前 locale 从内嵌语言包里查译文。
3. **用 manifest 里的 `(file, start_byte, end_byte)` 字节级偏移精确定位打补丁**。
   - 这是「回注」能精确写入的原因，也是为什么**偏移必须对准目标源码树**（见第 5 章）。

## 3.2 运行时

- **语言包嵌入二进制**：用 `rust-embed` 把 `assets/locales/*.json` 在编译期嵌进二进制（不依赖运行时文件系统）。
- **系统语言探测**：用 `sys-locale` 探测系统语言（它在 OHOS 上的行为有重要限制，见第 9 章）。
- **启动初始化**：`localization::initialize(InitRequest { user_preference, legacy_locale })`（`crates/localization/src/localization.rs:269`）决定最终生效语言。
  - 内部状态是 `static REGISTRY: OnceLock<Registry>`（`crates/localization/src/localization.rs:103`）→ **一次性**，首次调用定终身。

## 3.3 数据结构（字段级）

### 3.3.1 `catalog/en-US.json`

- 是**扁平 dict**：`英文原句 → 英文原句`（值等于键）。
- 实测核对（2026-09-29）：`tmp/zed-i18n/catalog/en-US.json` **7977 键**，且 `all(k == v)` 为 **True**（恒等）。

### 3.3.2 `manifest/ui-strings.json`

- 是 dict：键 = 英文原句，值 = 一个 dict。
- 每个值里至少含：
  - `status`：取值 **`accepted`** / **`ignored`** / **`needs_review`** 三种。
  - `occurrences[]`：每条含
    - `file`：源码文件（仓库内相对路径）。
    - `line`：行号。
    - `kind`：字符串类别（见 3.3.4）。
    - `start_byte` / `end_byte`：**字节级偏移**，回注时据此定位并替换。
    - `call`：调用形态（如 `localized_str!` 之类）。
- 实测核对：`tmp/zed-i18n/manifest/ui-strings.json` 共 **7977 条**，occurrence 合计 **10210 处**，覆盖 **466 个文件**，**199 种 kind**；`status` 分布为 `accepted=7276` / `ignored=541` / `needs_review=160`。
  - 旁证：`tmp/zed-i18n/reports/extract-summary.json` 亦记为 `{"occurrence_count": 10210, "source_count": 7977}`，与实测一致。

### 3.3.3 `translations/<locale>.json`

- 是**扁平 dict**：`英文原句 → 译文`。
- 实测核对：`tmp/zed-i18n/translations/zh-CN.json` 当前 **7428** 条（撰写时的现状见第 4 章差异说明）。
- 共 **13 种语言**：`cs-CZ` / `de-DE` / `es-ES` / `fr-FR` / `it-IT` / `ja-JP` / `ko-KR` / `pl-PL` / `pt-BR` / `ru-RU` / `tr-TR` / `zh-CN` / `zh-TW`（实测 `translations/` 目录）。

### 3.3.4 `kind`（类别）

- 实测共 **199 种**。
- 按 occurrence 数排序，头部依次是：
  - `rust_doc_comment`（2503）
  - `action_description`（1415）
  - `label`（781）
  - `tooltip`（615）
  - `setting_title`（494）
  - `setting_description`（493）
  - `button`（429）
  - 其下还有 `settings_enum_variant_label`（295）等。

### 3.3.5 运行时语言包（`assets/locales/*.json`）

由 `generate-runtime-bundles` 产出，结构与上面三种都不同。实测 `assets/locales/zh-CN.json` 顶层键为：

- `catalog_sha256`：字符串，语言包与 catalog 的绑定校验值。
- `locale`：字符串（如 `zh-CN`）。
- `messages`：dict，`英文原句 → 译文`（扁平，实测 **7434** 条）——**这是运行时真正查的扁平表**。
- `formats`：dict，`英文原句 → 分段结构`（实测 **477** 条），用于含占位符的句子，每个元素形如 `{"text": "..."}` 或 `{"arg": "占位符名"}`。
- `schema_version`：整数（实测 `1`）。
- 另有 `assets/locales/index.json`：顶层含 `catalog_sha256` / `locales`（数组，每项含 `id` / `native_name` / `english_name` / `aliases` / `direction` / `bundle`）/ `schema_version`。`en-US` 的 `bundle` 为 `null`（英文走源码原文，不需要包）。

## 3.4 许可证

- 内容侧：**GPL-3.0**。
- 工具侧：**MIT**。
- 其 `localization` crate 声明 `license = "GPL-3.0-or-later"`（实测 `crates/localization/Cargo.toml`）。
- 结论：**与本项目（GPL-3.0-or-later）相容**。

## 3.5 工具链子命令

实测 `tools/zed_i18n/cli.py` 注册了 **12 个子命令**：

- `fetch-zed`（拉取上游 Zed 源码，内部会调 `verify_checkout_revision` 校验版本）
- `extract`（**提取**：从源码抽 UI 字符串，写回 catalog/manifest）
- `audit-candidates`（审计候选）
- `validate`（校验）
- `apply`（**回注**：按语言逐个回注）
- `prepare-translation`（**翻译流水线·产出待译模板**）
- `merge-translation`（**翻译流水线·合回译文**）
- `extract-context-groups`（抽取上下文分组）
- `generate-version-diff`（生成版本差异）
- `generate-runtime-bundles`（**生成运行时语言包**）
- `apply-universal`（**通用回注**：一次回注即支持多语言，运行时查表）
- `generate-packaging`（生成打包产物）

其中本任务重点用到 4 组：`extract`、`prepare-translation` + `merge-translation`、`apply` / `apply-universal`、`generate-runtime-bundles`。
