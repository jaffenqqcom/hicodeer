# 开启坚盾守护模式后 HiCodeer 闪退

## 问题描述 (Problem Description)

用户在 HarmonyOS 的「设置 > 隐私和安全 > 坚盾守护模式」中开启坚盾模式后启动 HiCodeer，应用只运行约 8 秒即退出，无任何界面提示。cppcrash 日志 `cppcrash-com.hicodeer.edit-20020243-20261104144514864.log` 记录崩溃类型为 `Reason:Signal:SIGABRT(SI_TKILL)`，崩溃线程为主线程。根因是坚盾模式禁止应用申请匿名可执行内存，wasmtime 无法创建 WASM 编译所需的内存页，tree-sitter 的 `WasmStore::new` 返回错误后被 `unwrap()` 升级为 panic，panic 跨 NAPI 的 C 回调边界无法 unwind，最终触发 abort。

## 问题表现 (Symptoms)

- 开启坚盾模式 → 冷启动应用 → 约 8 秒后进程消失，无对话框、无错误提示。
- cppcrash 日志中的关键行：

```
thread '<unnamed>' panicked at crates/language/src/language.rs:142:58:
called `Result::unwrap()` on an `Err` value: WasmError { kind: Compile,
message: "failed to compile Wasm stdlib: unable to make memory executable
Caused by: 0: failed to make memory executable 1: Invalid argument (os error 22)" }
```

- 同一进程更早处（加载 html 扩展时）已出现同样错误，但只记 error 日志、未崩溃：

```
Failed to load extension: html ... failed to compile wasm component:
unable to make memory executable: Invalid argument (os error 22)
```

- 崩溃时机是进程扫描完工作区文件、进入语言解析阶段时，不是打开开关的瞬间。开关切换会重启应用，所以表现为「下次启动必崩」。
- 复现步骤：开启坚盾模式 → 启动 HiCodeer → 打开任一含代码文件的工作区 → 必现闪退。
- 关闭坚盾模式后一切正常，可稳定复现与规避。

## 问题原因 (Root Cause)

### 1. 坚盾模式的内核约束

坚盾模式（Secure Shield，`DSM_SECURE_SHIELD_MODE`）在内核/沙箱层拒绝把匿名内存页标记为可执行。实测探针结果：

```
mmap(PROT_READ|PROT_WRITE) 成功
mprotect(PROT_READ|PROT_EXEC) 返回 -1，errno=22 (EINVAL)
```

HAP 里已声明 `ohos.permission.kernel.ALLOW_WRITABLE_CODE_MEMORY`（`hap/entry/src/main/module.json5`），`bm dump` 显示该权限授予状态为 `0`（已授权），但坚盾模式在权限之上再收一层限制，应用侧无法通过申请权限恢复 JIT 能力。

### 2. 崩溃链路

```
NAPI 回调（libuv 主循环）
  └─ TSFN → libhicodeer.so
       └─ with_parser()                      crates/language/src/language.rs:139
            ├─ Parser::new()
            ├─ WasmStore::new(&WASM_ENGINE)  ← 内部 mmap + mprotect(PROT_EXEC)
            │    └─ Err(WasmError: unable to make memory executable)
            └─ .unwrap()                     ← 上游代码，panic
       └─ panic 跨 extern "C" 边界无法 unwind → abort → SIGABRT
```

### 3. 为什么是「必崩」而不是「功能缺失」

上游把 WASM 语法能力当成无条件可用：`with_parser` 在语法解析器池为空时新建 Parser，立即 `WasmStore::new(...).unwrap()`。坚盾模式下这一句必然失败，没有降级路径，所以任何触发语法解析的操作都会崩。

扩展宿主走的是另一套 `wasmtime::Engine`（`crates/extension_host/src/wasm_host.rs`），它本身返回 `Result`、失败只记日志，所以只有扩展加载失败而没有崩溃——但它掩盖了同一根因，直到语言层用 `unwrap` 把错误升级成崩溃。

### 4. 一条被排除的思路：改 WasmStore 本身

曾考虑把降级逻辑放进 `WasmStore`，不可行：

- `WasmStore::new` 内部调 `ts_wasm_store_new`，失败就返回 `Err`，行为已经正确，崩溃来自调用方的 `unwrap`。
- 让它「不返回 Err」只能伪造一个句柄，C 侧 `TSWasmStore` 没有降级实现，后续 `load_language` 会解引用非法指针，把「可预测的 Err」变成「不可预测的崩溃」，等同桩函数。
- 它覆盖不到扩展链路：扩展组件走 `Component::from_binary`，与 tree-sitter 的 store 无关。

## 解决方案 (Solution)

### 核心思路

坚盾模式禁用的是「可执行内存」，应用侧无法恢复 JIT，因此兼容目标是「不崩溃 + 原生功能可用」，而不是「在坚盾下继续跑 wasm」。wasmtime 只有 Cranelift JIT，没有解释器；AOT 预编译的 cwasm 同样需要可执行映射；第三方 wasm 扩展也无法预编译。所以 wasm 能力在坚盾模式下主动降级。

`catch_unwind` 也不可用：panic 发生在 NAPI 的 `extern "C"` 回调里，跨 FFI 展开不可靠，且 panic 后 parser 池与 engine 状态不可信。必须做到「不进入失败路径」。

### 判定点集中到 language.rs

探测用「真实构造一次 WasmStore」实现，不额外写 mmap/mprotect；结果缓存到 `OnceLock`，失败只试一次，整个进程生命周期不再重试（进出坚盾模式会重启应用，缓存不会过期）。

```rust
#[cfg(target_env = "ohos")]
pub fn wasm_runtime_available() -> bool {
    if let Some(available) = WASM_AVAILABLE.get() {
        return *available;
    }
    match WasmStore::new(&WASM_ENGINE) {
        Ok(store) => { drop(store); let _ = WASM_AVAILABLE.set(true); true }
        Err(error) => { let _ = WASM_AVAILABLE.set(false); log::warn!(...); false }
    }
}
```

`with_parser` 改为 cfg 互斥的两条分支，非 OHOS 分支与上游逐字一致：

```rust
// 改前（无条件）
parser.set_wasm_store(WasmStore::new(&WASM_ENGINE).unwrap()).unwrap();

// 改后
#[cfg(target_env = "ohos")]
if let Some(store) = new_wasm_store() {
    parser.set_wasm_store(store).unwrap();
}
#[cfg(not(target_env = "ohos"))]
parser.set_wasm_store(WasmStore::new(&WASM_ENGINE).unwrap()).unwrap();
```

语法加载路径原先在 `language_registry.rs` 里对 `take_wasm_store()` 又一次 `unwrap()`，也一并收口为 `language.rs` 的 `load_wasm_grammar()`：store 缺失返回 `Err`，走已有的 `AvailableGrammar::LoadFailed` 分支，不再 panic；正常路径保证 store 用完放回 parser。这样「判定、创建、取用失败处理」全部集中在一个文件。

### 扩展侧短路

`wasm_host.rs::load_extension` 在编译前判断 `language::wasm_runtime_available()`，不可用直接返回错误，避免 4 个内置扩展各跑一遍 Cranelift 失败刷日志。日志用 `OnceLock` 保证全局只记一条。

### 坚盾模式查询接口

SDK 里没有这个库的头文件和导入库，只能运行时解析：

- 库：`/system/lib64/ndk/libdevice_security_mode.z.so`（14 KB，设备实测 EXISTS）
- 符号：`HMS_DSM_GetDeviceSecurityMode`，返回位掩码，`DSM_NORMAL_MODE=0`、`DSM_SECURE_SHIELD_MODE=1`
- 系统能力 `SystemCapability.Security.SafetyDetect`，起始 5.0.1(13)

接口实现在 openharmony-ability 的 `crates/ability/src/device_security_mode.rs`：按绝对路径优先的候选表 `dlopen`，`dlsym` 成功即 `transmute` 成 `unsafe extern "C" fn() -> c_int` 并用 `LazyLock` 缓存；库或符号缺失一律返回 `false`（老镜像按普通模式处理）。句柄有意不 `dlclose`，保证符号所在映像不被卸载；仅「库打开成功但缺符号」的分支关闭句柄再继续。

设备侧独立验证通过：

```
query ok via /system/lib64/ndk/libdevice_security_mode.z.so -> mode=1
```

### 扩展页提示

`extensions_ui` 的 `render()` 在标题下方插入 `Banner`（`Severity::Warning`），条件为 OHOS 且 `is_secure_shield_mode_on()`。非 OHOS 分支显式置 `None`，依赖只在 `[target.'cfg(target_env = "ohos")'.dependencies]` 下声明。

文案走 localization 的 catalog，英文原文：

```
Secure Shield mode is on. Some extension plugins cannot run.
You can turn it off in Settings > Privacy and security > Secure Shield mode.
```

14 个语言包（含中文简繁）各补一条译文，`python3 script/check-i18n` 通过，`catalog_sha256` 无需改动。

## 验证

- 坚盾仍开启（系统接口实测 `mode=1`）下启动应用：进程正常存活，无新的 cppcrash 记录；重启应用后仍正常。
- `cargo check -p language -p extension_host -p extensions_ui` 通过，且是在依赖切回 GitHub 版本后执行的，证明新依赖可用。
- 未签名包安装被拒（`code:9568320 error: no signature file`），确认签名是必需步骤。

### 构建环境的一个副作用（与本次修复无关，但值得记录）

坚盾模式下构建工具链也会被打断：

- `node` 启动即崩（`Failed to mark memory page as executable`），需加 `NODE_OPTIONS=--jitless` 才能跑 hvigor/ohpm。
- JVM 无法初始化，`-Xint`、`-Xshare:off`、`-XX:TieredStopAtLevel=1` 全部无效，因此 hvigor 的签名步骤走不通。
- 可行解法：用 SDK 自带的原生签名工具 `command-line-tools/sdk/default/openharmony/toolchains/lib/hap-sign-tool`（ELF，不依赖 Java），参数与 hvigor 的 signingConfig 一致（`sign-app -keyAlias debugKey -signAlg SHA256withECDSA -mode localSign ...`）。

这条只影响开发机构建，不进入产品代码。

### 遗留与边界

- 坚盾模式下丢失的能力：内置 wasm 扩展（html/glsl/proto 等）、第三方 wasm 扩展、wasm 分发的语法高亮。原生语法、LSP、终端、文件编辑不受影响。
- 同类但不在本次链路上的：agent panel 里的 node/bun（V8/JSC 也要 JIT）、QEMU TCG。它们各自有独立入口，需要时复用同一探测结论。
- 若日后要恢复扩展功能，方向是把内置扩展从 wasm grammar 改成原生 grammar，而不是试图绕过坚盾。

相关经验条目：[[ohos-debug-lessons]]

## 修改文件 (Modified Files)

openharmony-ability-zed（已 commit 02fa617 并推送 origin/main）：

- `crates/ability/src/device_security_mode.rs` — 新增：dlopen + dlsym 查询坚盾模式，结果缓存，库缺失降级为普通模式
- `crates/ability/src/lib.rs` — 新增 `mod device_security_mode;` 与 `pub use device_security_mode::*;`

hicodeer（已 commit c3b437f）：

- `crates/language/src/language.rs` — `with_parser` 改为 cfg 互斥的降级分支；新增 `WASM_AVAILABLE`、`wasm_runtime_available()`、`new_wasm_store()`、`load_wasm_grammar()`
- `crates/language/src/language_registry.rs` — wasm 语法加载由 5 行 `take_wasm_store().unwrap()` 闭包改为调用 `load_wasm_grammar()`
- `crates/extension_host/src/wasm_host.rs` — `load_extension` 开头按 `wasm_runtime_available()` 短路，日志全局只记一次
- `crates/extensions_ui/src/extensions_ui.rs` — 扩展页顶部新增坚盾模式提示 `Banner`，仅 OHOS 编译
- `crates/extensions_ui/Cargo.toml` — 新增 OHOS 条件依赖 `openharmony-ability`，并在 cargo-shear 忽略表登记
- `assets/locales/` 14 个 bundle（cs-CZ / de-DE / es-ES / fr-FR / it-IT / ja-JP / ko-KR / pl-PL / pt-BR / ru-RU / tr-TR / zh-CN / zh-TW，英文原文在源码侧）— 各新增一条坚盾模式提示文案
- `Cargo.lock` — openharmony-ability 系列依赖升到 `02fa617c17f99391093c51b2dd4d3090b687eadb`
