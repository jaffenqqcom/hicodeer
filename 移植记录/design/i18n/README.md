# HiCodeer 多语种 / 中文化落地全过程

本目录记录「给 HiCodeer（Zed 的 OpenHarmony 移植）落地多语种 / 中文界面」这一件事的**完整过程**，写到**命令与判据级别**，供以后复用（例如以后要加日语、韩语，照着走）。

## 文档状态

- **撰写时间**：2026-09-29。
- **最近更新**：2026-09-29 22:5x（补写 10.3 落地结果、修正第 11 章过时条目、新增第 15/16/17 章；数字均以当次实测为准，仓库当时仍被另一 worker 改写，读数为时刻快照）。
- **覆盖到的步骤**：调研选型 → 拉取汉化项目 → 对 1.23 树重新提取 → 提取质量核对 → 新增文案归类 → 补充翻译 112 条 → 编译可行性验证（三次试验）→ 骨架 + 语言包落地 → **全量回注已执行（9439 处 / 457→455 文件）** → **默认配置本地化与 release 构建（进行中）**。
- **当前真实待补**：release 编译结果（进行中）、装包、线上取证（`hdc hilog` 看 `resolved_locale`）、界面人工确认（禁止截图，须用户提供）。
- **本目录的写入者**：本轮只新增 markdown、只修改本目录内已有的 i18n 文档，未改动本目录之外的任何代码或配置文件。

## 路径口径

- **仓库根**：`/storage/Users/currentUser/workspace/hicodeer`（下称「仓库根」）。本目录位于仓库内，是 git 追踪范围内的新增目录（`移植记录/` 已被 git 追踪）。
- 文中「仓库内相对路径」一律以仓库根为基准；临时资产一律用绝对路径。
- **临时资产根目录**：`/storage/Users/currentUser/tmp/`（本机 `/tmp` 只读，不可用）。
- 代码位置引用统一写成 `路径:行号` 形式，便于跳转。

## 章节索引

- [第 1 章 背景与目标](01-background-and-goals.md)
- [第 2 章 调研：外部有没有现成方案](02-research-existing-solutions.md)
- [第 3 章 选定方案的机制](03-selected-solution-mechanism.md)
- [第 4 章 拉取与适配评估](04-fetch-and-compatibility.md)
- [第 5 章 对 1.23 树重新提取](05-reextract-on-1.23.md)
- [第 6 章 提取质量核对](06-extraction-quality-check.md)
- [第 7 章 新增文案归类](07-new-strings-classification.md)
- [第 8 章 补充翻译（112 条）](08-supplementary-translation.md)
- [第 9 章 编译可行性验证](09-build-feasibility.md)
- [第 10 章 落地（骨架 + 语言包 + 编译 + 装包）](10-landing.md)
- [第 11 章 遗留问题与下一步](11-open-issues-and-next-steps.md)
- [第 12 章 可复现步骤清单](12-reproducible-steps.md)
- [第 13 章 环境与工具坑清单](13-environment-pitfalls.md)
- [第 14 章 关键决策记录](14-decision-log.md)
- [第 15 章 全量回注实操与「四道闸」](15-mass-apply-in-practice.md)
- [第 16 章 默认配置本地化与 release 构建](16-default-settings-and-release.md)
- [第 17 章 语言切换与「实时生效」设计](17-locale-switch-and-live-reload.md)
- [附录 17a 实时切换设计原稿（全文）](17a-live-switch-design-full.md)

## 一句话结论

- Zed 官方**没有** i18n，社区也没有「丢个语言包就生效」的运行时方案，汉化只能**改源码重新编译**。
- 我们选了 `LI-NA/zed-i18n`（工具链式方案：提取 → 翻译 → 回注 → 编译），它自带一个独立 crate `localization`，用宏查表 + 字节级偏移回注。
- 它钉死上游 `v1.21.0`，而我们当时是 `1.23.0`，差两档；因为 manifest 用**字节偏移**定位，所以要**对我们的 1.23 树重新跑 extract**。
- 重提取已跑通（7977 键 / 10210 处 / 466 文件 / 199 kind，rc=0，250.40 秒），我们仓库全程只读。
- 编译可行性已验证（该 crate 的新引入依赖全部是本仓已有依赖，OHOS target 上三次试验通过）。
- **全量回注已完成**（9439 处 / 覆盖 90 个 crate；改动规模 455 文件 / +10301 / −5858，2026-09-29 22:48 实测）；回注后的 **debug 构建已成功**。
- **release 构建进行中**；装包、线上取证、界面人工确认**未做**。本目录记录方案、判据与实际执行过程。

## 后续维护提示

以后要给 HiCodeer **再加一种语言**（例如日语、韩语），需要动的地方：

1. **翻译词表**：在 `tmp/zed-i18n/translations/<新 locale>.json`（或合并后的源词表）里补齐该语言的译文；格式是扁平 dict「英文原句 → 译文」。
2. **运行时语言包**：跑 `generate-runtime-bundles` 重新产出 `assets/locales/<新 locale>.json`（结构见第 3 章），并更新 `assets/locales/index.json` 的 `locales` 数组。
3. **语言选择器**：确认 `crates/language_selector` 的 `localized_language_name.rs` 与新 locale 的显示名一致。
4. **初始化来源**：`user_preference` 现由 `ui_locale` 设置项驱动（设置项定义 `crates/settings_content/src/settings_content.rs:333`；读取点 `crates/zed/src/zed/ui_locale.rs:8-11`）——语言选择器已现成，无需新增设置项（见第 11.7、17 章）。
5. **验证**：重跑提取质量核对四类抽样（第 6 章），编译（第 10 章），装包后人工确认界面（禁止截图，见全局规则第 13 条）。

**三条最容易踩的维护铁律**：

1. **改动源码后必须重跑 `extract` 再回注**——字节偏移只对「提取那一刻的树」有效，源码一改即作废（第 15 章第三道闸）。
2. **回注用 `apply-universal`，不要用 `apply`**——后者单语言替换会破坏语言切换、且目标路径写死；机检判据是「新增行里中文字符数 = 0」（第 15.1 节、10.3(3)）。
3. **默认配置需显式写 `zh-CN`**——`sys-locale` 在 OHOS 上探测不到系统语言，默认 `"system"` 会回落 `en-US`（第 9 章、第 16.6 节）。

本目录位置说明：`移植记录/design/i18n/`。它挂在「移植记录」下，属于**只读区**的 `design/` 子目录——`移植记录/` 下已有内容一律只读、不许改删（见全局 CODEBUDDY.md 相关约束与该目录既有约定）；本 `i18n/` 子目录是本次新建的**新增**目录。
