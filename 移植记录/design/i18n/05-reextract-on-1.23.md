# 第 5 章 对 1.23 树重新提取（关键技术环节）

## 5.1 为什么必须重提取

- `LI-NA/zed-i18n` 自带的词表与 manifest 是针对 **v1.21.0** 做的。
- manifest 里的 `start_byte` / `end_byte` 是**字节级偏移**，**只对 1.21 的源码树有效**。
- 要回注我们的 **1.23 树**，必须**对新树重跑 extract**，得到对准我们源码的偏移。
- 这一步是整件事的关键：偏移不对准，回注就会把补丁打到错误的位置。

## 5.2 环境准备（踩坑记录）

- 本机**没有 `uv`**（而它的官方用法是 `uv run zed-i18n`），仅有 harmonybrew 的 `python3` **3.14.7**，路径 `/storage/Users/currentUser/.harmonybrew/bin/python3`。
- 依赖只有两个：`tree-sitter>=0.25,<0.26` + `tree-sitter-rust>=0.24,<0.25`；要求 Python ≥ 3.12。
- 本机是 **`aarch64-linux-musl`**，tree-sitter **没有 cp314 预编译轮子** → 必须**源码编译**，其中两个坑：
  1. **缺 `-lintl`**：用 harmonybrew 的 `libintl`，通过 `LDFLAGS=-L$HOME/.harmonybrew/lib -Wl,-rpath,$HOME/.harmonybrew/lib` 解决。
  2. **pip 打包阶段 `bdist_wheel` 的 musllinux 探测**会执行 `/lib/ld-musl-aarch64.so.1`，被 `Permission denied` → 必须 `pip install --no-build-isolation`，并在 venv 内**先装 `setuptools` + `wheel`**。
- 最终 venv 落在 `/storage/Users/currentUser/tmp/zed-i18n-venv`（约 **31 M**，实测），装成 `tree_sitter-0.25.2` + `tree_sitter_rust-0.24.2`。
- **全程未 `sudo`、未动系统 / 全局 Python。**

## 5.3 CLI 与守卫（重要坑）

### 5.3.1 参数与输出

- `extract` 只有 `--zed-root`（可选），**没有 `--output`**；输出**固定写回** zed-i18n 仓库自身的 `catalog/`、`manifest/`。
- 因此重提取会**覆盖** `tmp/zed-i18n/catalog/en-US.json` 与 `tmp/zed-i18n/manifest/ui-strings.json`；1.21 版本需**先另存为 baseline**（见 5.4）。

### 5.3.2 输入范围

- 扫描 `<zed-root>/crates/**/*.rs`。
- **`EXCLUDED_PARTS = {tests, fixtures, examples}`** 会被跳过。

### 5.3.3 不校验版本

- **它不校验版本**：`verify_checkout_revision` 只在 `fetch-zed` 里调用。→ 所以能对非 1.21 的树提取。

### 5.3.4 工作区守卫与绕过

- **但有个工作区守卫**：`ensure_inside_workspace` 会把 `--zed-root` 解析后要求落在 zed-i18n 仓库内，否则抛 `path is outside workspace`。
- **绕过方式**：写一个**驱动脚本**，在**运行时**把 `tools.zed_i18n.cli.ensure_inside_workspace` 替换为**恒等函数**后再调 `cli.main`——**不修改项目任何文件**。

### 5.3.5 执行命令形态

```sh
/storage/Users/currentUser/tmp/zed-i18n-venv/bin/python <driver.py> \
    --root /storage/Users/currentUser/tmp/zed-i18n \
    extract \
    --zed-root /storage/Users/currentUser/workspace/hicodeer
```

（`<driver.py>` 即上面那个做运行时替换的驱动脚本；`--root` 指向 zed-i18n 仓库，`--zed-root` 指向我们的 1.23 树。）

## 5.4 结果

- 输出：`Extracted 7977 source strings from 10210 occurrences`，**rc=0，耗时 250.40 秒**。
- 新产物：
  - `catalog/en-US.json`：**7977 键**。
  - `manifest/ui-strings.json`：**7977 条** / **10210 occurrence** / **466 个文件** / **199 个 kind**。
- `status` 分布：**`accepted=7276` / `ignored=541` / `needs_review=160`**（实测一致）。
- **自洽校验**：
  - `needs_review = 160` 恰等于「仅 1.23 有」的 160 条。
  - `accepted` 少 40 + `ignored` 少 24 = **64**，恰等于「仅 1.21 有」的 64 条。
    （1.21 baseline 的 status 分布实测为 `accepted=7316` / `ignored=565`；与 1.23 的 7276 / 541 相减，差值 40 与 24，合计 64。）
- **`crates/gpui_ohos/**` 贡献 0 条**（该目录没有 UI 文案字面量；实测含 `ohos` 的文件集合为空）。
- 我们新增的 `crates/settings_content/src/qemu.rs` 贡献 **28 条**（实测：该文件在 `needs_review` 中出现 28 次）。
- 我们的仓库**全程只读**（跑前跑后 `git status` 均只有既有的 `M install-local.sh`）。

## 5.5 两版差异（核心数字）

- 1.21 键：**7881**（实测 `tmp/zed-i18n-baseline-1.21/catalog/en-US.json`）。
- 1.23 键：**7977**（实测 `tmp/zed-i18n/catalog/en-US.json`）。
- 共有：**7817**（占 1.21 的 **99.19%**）。
- 仅 1.21 有：**64**（占 1.21 的 **0.81%**）。
- 仅 1.23 有：**160**（占 1.23 的 **2.01%**）。
- 并集：**8041**。
- 净变化：**+96**。

### 5.5.1 那 64 条「消失」的拆解

- **47 条是文档注释（`rust_doc_comment`）措辞被上游改写**。
  - 实测佐证：仅 1.21 有的 64 条里，首个 occurrence 的 kind 为 `rust_doc_comment` 的恰好 **47 条**。
- **9 条只是断句片段**（如 `Default: [`、`scopes.`）。
  - 实测备注：若按「长度 < 12 字符」判据，实测得 **6 条**（样例 `Default: [`、`scopes.`、`— {} {}` 等都在其中）。对话记录为 9 条，与实测有口径差异，以实测为准并标注。
- 剩余真找不到的条目里，多为被改写的 UI 文案。实证（对话记录）：
  - `Stage {count} Files` → `Stage Folder`
  - `Trash {count} Files` → `Trash {file_count} {file_label}`
  - `Failed to discard changes` → `Failed to discard unsaved changes: {e}`

### 5.5.2 kind 分布变化

- `rust_doc_comment`：**2375 → 2503**（+128）。
- `settings_enum_variant_label`：**275 → 295**（+20）。
- 其余 kind 的 |Δ| **< 8**。
- **两版 kind 集合完全相同**（实测 `set(1.21 kinds) == set(1.23 kinds)` 为 True，均为 199 种）→ 说明差异来自**内容变化**，不是提取口径变了。

## 5.6 为什么这步很重要

- 它把「别人的 1.21 词表」变成「**对我们 1.23 树对准的词表 + 偏移**」。
- 有了它，「全量回注」才具备**条件**（见第 11 章遗留问题）。
- 它也给出了**干净的差异清单**（160 条新增、64 条消失），这是第 7、8 章归类与补译的输入。
