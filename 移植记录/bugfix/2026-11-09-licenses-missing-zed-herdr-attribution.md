# 开源许可申明缺失 Zed 主体与 herdr

## 问题描述

HiCodeer 的 `assets/licenses.md` 由 `script/generate-licenses` 自动生成，并在编译期被 rust-embed 嵌入应用，是「关于 → 开源许可」页面（`crates/zed/src/zed.rs:237`、`crates/settings_ui/src/settings_ui.rs:6889`）的唯一数据源。但生成结果里既没有说明本产品基于 Zed 源码，也没有出现 herdr，用户拿到的许可页无法体现这两项上游代码的使用。

## 问题表现

- 在全文中搜索 `herdr`（任意大小写）零命中。
- 与 Zed 相关的条目只有零散依赖：`gpui 0.2.2`、`html_to_markdown`、`zed_extension_api`、`zed-font-kit`、`zed-reqwest`，以及一批第三方 `zed-*` 扩展；没有任何一条指向 Zed 编辑器主体。
- `assets/licenses.md` 被 `.gitignore:40` 忽略，属于构建产物；`script/bundle-ohos` 在修复前完全没有调用 `script/generate-licenses`，因此新克隆的仓库或未手工生成过的环境，release 包里该页面读不到内容。
- 编译本身不会报错：rust-embed 只收集扫描到的文件，缺 `licenses.md` 时构建通过，直到用户打开许可页，`crates/util/src/util.rs:660` 的 `A::get(path).expect(path)` 才 panic。

## 问题原因

不是脚本漏执行，而是「依赖图扫描」这个模型天然覆盖不到两类对象，加上两处机制把线索进一步消除了。

第一层：`script/generate-licenses` 只做两件事——拼接静态资产段（主题、图标、字体、扩展、随包资产），以及跑两次 `cargo about generate`（根 workspace 和 `crates/gpui_ohos/depend/cmd-agent/hicodeerd`）。`cargo about` 的输出条目来自依赖图里每个 crate 的 `license` 与 `repository` 元数据，workspace 自身不作为被依赖方出现。

第二层：`script/licenses/hicodeer-licenses.toml:2` 配置了 `private = { ignore = true }`，而根 `Cargo.toml:228` 是 `[workspace.package] publish = false`，`crates/zed/Cargo.toml` 用 `publish.workspace = true` 继承它。于是 `zed`、`editor`、`terminal`、`workspace` 这一整棵 Zed 源码树的 crate 全被判为 private 并跳过，`crates/zed/Cargo.toml` 里那句 `license = "GPL-3.0-or-later"` 从未被渲染。同时 `accepted` 白名单里没有 `GPL-3.0-or-later`，即使手动放开 private，cargo-about 也会因「未接受的许可证」失败。反证是 `crates/gpui/Cargo.toml` 显式写了 `publish = true` 与 `license = "Apache-2.0"`，所以 `gpui` 反而出现在清单里。

第三层：herdr 并不是没扫到。`crates/gpui_ohos/depend/cmd-agent/hicodeerd/Cargo.toml:22` 以 `agent_server = { package = "hicodeer-agent", path = "../../../../../herdr" }` 引入它，herdr 没有 `publish = false`，所以确实进了报告——落成 `* [hicodeer-agent 0.9.5]( https://crates.io/crates/hicodeer-agent )`。但 package 被改名，该条又拿不到 `repository`，`script/licenses/template.md.hbs` 在 repository 为空时回落到 `https://crates.io/crates/<name>`。结果是从这一条既看不出上游叫 herdr，也看不到 `github.com/herdrdev/herdr`。

第四层：`script/licenses/check-coverage.py` 的检查项只有扩展 id、字体目录、CA bundle、两次 cargo 是否落地、daemon 是否覆盖 herdr 依赖集。herdr 那段只比较 `herdr/Cargo.lock` 与 daemon `Cargo.lock` 的 crate 名字差集，不问报告里是否存在 herdr 条目、更不问能否识别。因此上述缺失全程绿灯。herdr 自己的 workspace 成员 `crates/ghostty-vt` 也因 private 规则不出现，进一步说明覆盖判定只看 lock 名字。

## 解决方案

不去改 cargo-about 的 private 与 accepted 规则——那会把上百个本地 crate 拉进清单，或直接触发 GPL 白名单错误，改动面大且语义混乱。改为在既有的「多报告合并」流程里补一份上游来源报告：

- `script/generate-licenses` 在两次 `cargo about` 之后构造第三份报告，格式与 `merge-about.py` 解析的块完全一致：
  - `#### Apache License 2.0` + `##### Used by:` + `* [herdr 0.9.5]( https://github.com/herdrdev/herdr )`，不附文本，让已有的 Apache 全文继续由 cargo-about 提供；
  - `#### GNU General Public License v3.0 or later` + `* [zed 1.26.0]( https://github.com/zed-industries/zed )` + 仓库根 `LICENSE-GPL` 的全文。
- 版本与仓库地址从 `herdr/Cargo.toml`、`crates/zed/Cargo.toml` 解析，不硬编码，避免升级后失配。
- 三份报告一起交给 `merge-about.py`，由它把 herdr 条目按字母序并入 Apache 段、把 GPL 段插到对应位置，并重算 Overview 与 Total。
- `script/licenses/check-coverage.py` 增加两条断言：报告里必须存在 `* [zed ` 与 `* [herdr `，否则失败。这样以后改生成流程不会再无声丢掉。
- `script/bundle-ohos` 在 release 编译前调用 `script/generate-licenses`（`--hap-only` 与 debug 构建不调用），使随包分发的产物自带最新申明。

关键取舍：只加标准条目（许可证名、crate 名、版本、仓库链接、许可证全文），不写上游地址说明段、基线 commit、修改说明等非标准内容。

实际重跑 `bash script/generate-licenses`，退出码 0，`check-coverage` 通过；产物中 `* [herdr 0.9.5]( ... )` 落在 Apache 段 heck 与 hermit-abi 之间，`#### GNU General Public License v3.0 or later` 段落在 CC0 与 ISC 之间。

曾考虑但放弃的路径：在 `accepted` 里加 `GPL-3.0-or-later` 并单独放开 Zed 树——cargo-about 没有 per-crate 的 private 覆盖，放开会连带列出全部本地 crate；把 herdr 的 package 名改回去——会波及 daemon 的编译与命名约束，代价远大于收益。

## 修改文件

- `script/generate-licenses` — 新增 UPSTREAM_REPORT：从 Cargo.toml 解析 herdr/zed 版本与仓库，生成 Apache 与 GPL-3.0-or-later 两个标准块，作为第三份报告交给 merge-about.py。
- `script/licenses/check-coverage.py` — 新增 `* [zed ` / `* [herdr ` 覆盖率断言，并在文件头说明补列第六项检查。
- `script/bundle-ohos` — 在 release 编译前调用 `script/generate-licenses`；`--hap-only` 与 debug 构建不触发。
- `assets/licenses.md` — 重新生成的产物，新增 herdr 条目与 GPL-3.0-or-later 段；该文件被 .gitignore 忽略，不入库。

[[ohos-debug-lessons]]
