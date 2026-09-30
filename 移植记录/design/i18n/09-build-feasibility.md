# 第 9 章 编译可行性验证（这是「能不能做成」的分水岭）

落地前必须先回答一个问题：**这个 `localization` crate 在我们的 OHOS 构建里到底编不编得过？** 本章记录验证过程与结论。

## 9.1 目标 crate

- crate 名：`localization`。
- 来源：`tmp/zed-i18n/tools/zed_i18n/runtime_overlay/crates/localization/`（**注意路径用的是下划线 `zed_i18n`，且嵌在 Python 包里**）。
- 落地：仓库内 `crates/localization/`（`Cargo.toml` + `src/localization.rs`）。
- 实测：仓库内 `crates/localization/src/localization.rs` 与 overlay 源**逐字节一致**（`diff` 无差异，761 行）。

## 9.2 依赖面

- `Cargo.toml` 里 **5 个依赖全部走 workspace**：
  - `log` / `rust-embed` / `serde` / `serde_json` / `sys-locale`
- → **不引入任何新外部 crate**。
- license：`GPL-3.0-or-later`（实测）。

## 9.3 代码特征

- **零平台 `cfg`**：全文只有两处 `#[cfg(test)]`（实测 `crates/localization/src/localization.rs:106` 与 `:602`），没有任何针对平台的条件编译。
- **无 `std::env`**、**无直接系统调用**（实测：全文未出现 `std::env` / `libc` / `std::process` / `setlocale`）。

## 9.4 我们仓库的依赖现状（为什么低风险）

- `sys-locale`：已在 `crates/time_format/Cargo.toml:16` 使用（`sys-locale.workspace = true`），workspace 定义在根 `Cargo.toml:868`（`sys-locale = "0.3.1"`）。
- `rust-embed`：已在 `crates/grammars/Cargo.toml:18`、`crates/util/Cargo.toml:34` 使用；其 `compression` features 会间接拉起 `include-flate` / `globset` / `walkdir` / `sha2` 链。
- `serde` / `serde_json` / `log`：全树大量使用。
- 结论：这 5 个依赖我们**本来就在用**，没有新引入任何未知依赖。

## 9.5 OHOS 构建配置

- target triple：**`aarch64-unknown-linux-ohos`**（来自 `script/bundle-ohos:200`，`OHOS_TARGET="aarch64-unknown-linux-ohos"`）。
- 工具链：**`RUSTUP_TOOLCHAIN=1.97.1`**（来自 `script/bundle-ohos:221`）。
- 本机 `rustc -vV` 的 `host` 实测就是 **`aarch64-unknown-linux-ohos`** → **不是交叉编译**（本机即设备；`rustc 1.97.1`，`host: aarch64-unknown-linux-ohos`）。

## 9.6 三次试验（最有说服力的证据）

命令形态（三次通用）：

```sh
cargo check -p localization --target aarch64-unknown-linux-ohos --offline
```

环境：`PATH=$HOME/.harmonybrew/opt/rustup/bin:...`、`RUSTUP_TOOLCHAIN=1.97.1`、`RUSTFLAGS="-A warnings"`。

### 试验①：只加 crate、无 `assets/locales`

- 结果：**EXIT=101**。
- 报错：`#[derive(RustEmbed)] folder '.../assets/locales' does not exist`，外加一个因宏展开失败带出的 `E0599`。
- **原因分类：缺资源**（非平台问题、非 Rust 代码错误）。
- 在此之前，`serde` / `log` / `rust-embed` / `include-flate` / `globset` / `sys-locale` **全部 check 通过**。

### 试验②：补一个**空**的 `assets/locales/` 后同命令

- 结果：**EXIT=0，5.02 秒**。

### 试验③：host target

```sh
cargo check -p localization --offline
```

- 结果：**EXIT=0，1m22s**。

## 9.7 旁证：历史编译指纹

- OHOS target 的 `target/aarch64-unknown-linux-ohos/{debug,release}/.fingerprint/` 下**早就有** `sys-locale`、`rust-embed`、`include-flate`、`globset`、`walkdir`、`sha2` 等的历史编译指纹（实测存在）。
- → 它们在这个 target 上**本来就编得过**，不是本次新引入的未知风险。

## 9.8 最关键的运行时结论（决定要不要新增通道）

`sys-locale` 在 OHOS 上走的是 **`unix` 分支**（条件编译 `#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]`），其 `src/unix.rs`：

- **只读四个环境变量**，按顺序取：`LANGUAGE` → `LC_ALL` → `LC_MESSAGES` → `LANG`。
- 然后做 **POSIX → BCP47** 转换。
- **没有任何系统 API、没有 `setlocale`、不读 HarmonyOS 系统设置**。

→ **推论**：

- 在 OHOS 上，若应用进程**没有**这些环境变量，`get_locales()` 返回**空** → `system_locale()` 为 **`None`** → 最终**回落 `en-US`**。
- **HarmonyOS 系统设置里的语言不会自动传进来。**
- 要中文生效，**必须由调用方通过 `initialize(InitRequest { user_preference, legacy_locale })` 显式传入**（或由 OHOS 侧把语言塞进进程环境）。
- 这与我们已知的 colorMode「**必须由 ArkTS 侧经配置通道主动带入**」的模式一致。

补充：

- `initialize()` 是**一次性**的——基于 `static REGISTRY: OnceLock<Registry>`（`crates/localization/src/localization.rs:103`），首次调用定终身（实现见 `:269`）。

## 9.9 本轮的改动清单（精确到行）

- 新增 `crates/localization/Cargo.toml`。
- 新增 `crates/localization/src/localization.rs`。
- 根 `Cargo.toml`：
  - members 加一行 `"crates/localization",`——位于 `crates/lmstudio` 与 `crates/lsp` 之间（保持字母序），实测在 `Cargo.toml:141`。
  - workspace 依赖声明加一行 `localization = { path = "crates/localization" }`，实测在 `Cargo.toml:415`。
  - 实测 `git diff --stat Cargo.toml` = **2 insertions**。
- `Cargo.lock`：自动多若干行（**未引入新外部 crate**）。
  - 实测：`git diff --stat Cargo.lock` = **13 insertions**（对话记录为 11 行，标注差异，以实测 13 为准）。
- 临时建**空**目录 `assets/locales/`。

## 9.10 回退命令

```sh
rm -rf crates/localization && \
sed -i '/^    "crates\/localization",$/d' Cargo.toml && \
git show HEAD:Cargo.lock > Cargo.lock && \
rmdir assets/locales
```

> 注意：`rmdir assets/locales` **仅适用于验证阶段那个「空目录」**。若 `assets/locales/` 里已有语言包（落地阶段会放入，见第 10 章），不要执行这一句（`rmdir` 对非空目录会失败，且不应删除语言包）。此外 `sed` 只删 members 那一行；若还要回退 workspace 依赖声明那一行，请手工处理，不要用破坏性命令批量改。
