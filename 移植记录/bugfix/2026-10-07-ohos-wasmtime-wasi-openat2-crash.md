# wasmtime-wasi 内嵌 WASI primitives 触发 openat2 被 seccomp SIGSYS 杀进程（附：hvigor ArkTS 大小写检查构建卡点）

## 问题描述 (Problem Description)

用户在设备上使用 HiCodeer（`com.hicodeer.edit`）时应用崩溃，cppcrash 日志（`cppcrash-com.hicodeer.edit-20020232-20261007135829908.log`）关键行：`Reason:Signal:SIGSYS(SYS_SECCOMP) syscall number is 437`。进程存活 115 秒，进入前台约 45 秒后被杀；崩溃线程为 `tokio-rt-worker`。

根因：**wasmtime-wasi 内嵌的 WASI 文件系统 primitives 在 `target_os = "linux"` 上默认走 `openat2` 快速路径**，而 OHOS 应用 seccomp 白名单不含 `openat2`（437），内核在 syscall 入口直接发 `SIGSYS` 终止进程。

这是 2026-08-26 修复过的同类问题（cap-primitives 调用 openat2）在 **wasmtime-wasi 内嵌的第二份同源代码**中的再现：当时只修了 cap-primitives；wasmtime-wasi 48 自带了一份从 cap-std 移植的 primitives 副本（`src/filesystem/primitives/rustix/linux/fs/*`），未随同修复，成为漏网之鱼。

排查过程中还牵出并解决了第二个问题：**验证修复需要重新打包 HAP，但本机构建必失败**（hvigor 的 ArkTS 编译器报 50 个 `Cannot resolve import statement ... Please check the case`），根因是目录名大小写的"双源真名"不一致（见下文），一并记录。

## 问题表现 (Symptoms)

### 故障一：运行时 SIGSYS 崩溃（主问题）

- 崩溃日志关键行：`Reason:Signal:SIGSYS(SYS_SECCOMP) syscall number is 437`。
- 崩溃线程 `tokio-rt-worker`（wasmtime 的 tokio blocking pool）。
- 寄存器佐证（日志 dump）：
  - `x8 = 0x1B5 = 437`（openat2 的 syscall 号）；
  - `x1` 指向栈上 WASI 沙箱内相对路径 `servers/0.3.0/aarch64-unknown-linux-gnu/rust-glancer`（小端解码）；
  - `x3 = 0x18 = 24 = sizeof(struct open_how)`，与 openat2 调用约定完全吻合。
- 符号化调用栈（用 `target/libcore-symbols/libcore.so` + llvm-addr2line）：

  ```
  wasmtime_wasi::filesystem::Dir::stat_at
  └─ wasmtime_wasi::filesystem::unix::stat_at
     └─ primitives::stat::stat
        └─ primitives::rustix::linux::fs::stat_impl::stat_impl
           └─ ...open_impl::open_beneath
              └─ rustix::fs::openat2  (linux_raw backend)
                 └─ syscall(SYS_openat2) → SIGSYS
  ```

- 反汇编铁证（崩溃 pc = `0xec4faa4`，模块基址 `0x5decac0000`）：

  ```
  ec4fa98: mov  w8, #437      ← syscall 号
  ec4fa9c: mov  w3, #24       ← sizeof(open_how)
  ec4faa0: svc  #0            ← 内核陷入，seccomp 在此投递 SIGSYS
  ec4faa4: tbz  x0, #63, ...  ← 崩溃 pc（svc 下一条）
  ```

- 触发条件：wasm 扩展（WASI 沙箱）执行文件 stat 类操作时即触发；旧包前台 45 秒内必崩。

### 故障二：hvigor 打包必失败（验证修复时暴露）

- `script/bundle-ohos` 在 Rust 编译/链接全部成功后，卡在 hvigor 的 ArkTS 编译：

  ```
  > hvigor ERROR: Failed :entry:default@CompileArkTS...
  ArkTS Compiler Error
  1 ERROR: Cannot resolve import statement /storage/.../HiCodeer/hap/entry/src/main/ets/entryability/EntryAbility.ets. Please check the case.
  ...（共 50 个同类 ERROR，连同目录 ./Setup、生成文件 ResourceTable.ts、@ohos-rs/* 依赖全部报错）
  ```

- 连跑两次均失败（29s / 26s），非偶发。
- 同机另一个项目 warp-ohos 的 HAP 却在 2026-10-06 刚成功产出——说明本机工具链可用，是 HiCodeer 侧特有的问题。

## 问题原因 (Root Cause)

### 故障一根因（openat2）

链条如下：

1. **OHOS target 的 cfg 是"Linux 外观"**：`rustc --print cfg --target aarch64-unknown-linux-ohos` 实测 `target_os="linux"`、`target_env="ohos"`。因此上游所有 `#[cfg(target_os = "linux")]` 的优化路径在 OHOS 上都会被选中。
2. **wasmtime-wasi 的 Linux 特化实现**：`src/filesystem/primitives/rustix/linux/fs/stat_impl.rs` 用 `open_beneath`（`open_impl.rs`）实现沙箱内 stat——这正是崩溃路径。上游为 Android 做了规避（`#[cfg(target_os = "android")]` 用 `manually::stat`），但 OHOS 不在其列。
3. **rustix 走 linux_raw backend**：构建指纹显示 rustix 编译输出 `cargo:rustc-cfg=linux_raw`、依赖 `linux_raw_sys`（而非 libc）；`RUSTFLAGS` 中并无 `--cfg rustix_use_libc`。因此 `rustix::fs::openat2` 由**内联汇编 `svc #0`** 发出。
4. **seccomp 硬杀**：OHOS 应用 seccomp（libapp_filter）对白名单外 syscall 直接发 SIGSYS；`openat2` 不在白名单且无任何应用权限可放行（2026-08-26 已调研确认）。

**为什么 shim 垫片拦不住（排查中的关键结论）**：

- 项目现存的 `ohos-libc-shim` 手段是 `-Wl,--wrap=<symbol>`（**链接期符号重定向**），只对"经过符号的调用"有效。
- 本崩溃的 openat2 由 rustix linux_raw 内联 `svc #0` 发出，**不经过任何符号**——既不经 `openat2` 符号，也不经 libc 的 `syscall` 函数——`--wrap` 物理上无法拦截。
- shim 文档注释设想的通路（用 `rustix_use_libc` cfg 让 rustix 走 libc backend，其 openat2 会经 `syscall(SYS_openat2, ...)` 符号从而被 `--wrap=syscall` 拦下）**从未接进构建**：`script/bundle-ohos` 只导出 `RUSTFLAGS="${RUSTFLAGS:-} -A warnings"`，`.cargo/config.toml` 无该 cfg，git 历史里 `rustix_use_libc` 字样只出现在 shim 注释与一篇移植记录中；构建产物指纹（`cargo:rustc-cfg=linux_raw` + deps 含 `linux_raw_sys`）证实 rustix 一直是 linux_raw。
- 另外两套 shim（`musllib-shim` = TLS key 虚拟化；`ohos-meta-shim` = virtiofs 元数据放行）与 openat2 无关。

### 故障二根因（hvigor 大小写检查 + dentry 名污染）

排查中先出现两个误导，均被后续实验推翻：

- **误导 1**：日志首行的 `load native resolver failed: Failed to load native binding ... libgcc_s.so.1` 看起来像根因。实际上它是 `node-resolve/index.js` 里 **try/catch 捕获后的非致命警告**（resolver 加载失败会 fallback 到 JS 路径）；目录里两个 arm64 `.node`（2026-09-15 补入）是 glibc 二进制（NEEDED `libc.so.6`/`libgcc_s.so.1`），在 OHOS（musl 用户态、工具链 node 自身 NEEDED `libc.so`）上本就无法加载，但**真正导致编译失败的不是它**，而是 ArkTS 编译器的大小写检查。
- **误导 2**：先怀疑"从大写路径启动构建"是原因 → cd 到小写路径重跑 → **仍然失败**（错误信息里仍是 `/storage/.../HiCodeer/...`）→ 该假设被推翻，并引出真正的机制。

真因（三步实测定位）：

1. **磁盘目录项真名 = 小写 `hicodeer`**：`python3 os.listdir('/storage/Users/currentUser/workspace')` 返回 `'hicodeer'`；大小写保留探针（创建 `AbC.tmp` 后 readdir 原样返回）证明该文件系统**不折叠大小写**。
2. **内核 dentry 名 = 大写 `HiCodeer`**：`pwd -P`、`readlink /proc/self/cwd`、node `process.cwd()`（三者都走 getcwd）**一致返回大写**，且用小写拼写 cd 进去后仍返回大写（dentry 名不会因小写访问改回）。**大写的出处（后续定点调查）**：
   - **应用代码里硬编码**：`hap/entry/src/main/ets/entryability/Setup.ets:15` `const HOME_DIR_NAME: string = 'HiCodeer'` —— 应用把用户选定根目录下的"数据主目录"定义为 `<root>/HiCodeer`（大写），首次设置时 `mkdirSync` 创建并把该大写路径写入记录文件；
   - **运行时记录**：app 私有目录 `files/custom_data_dir`（`Setup.ets:13` 的 `RECORD_FILE`），设备实读内容为 `/storage/Users/currentUser/HiCodeer`；此后每次启动都按该大写路径访问。注意：这是**主目录下**的 app 数据目录（磁盘真名即大写），与 `workspace/` 下的项目目录是两个不同路径；
   - **工作区侧**：`workspace/hicodeer` 是**小写真名的历史目录**（用户侧创建）；应用侧存在按大写拼写访问该项目内文件的历史记录（app hilog 可见 `workspace/HiCodeer/crates/node_runtime/src/lib.rs` 的 worktree 条目），内核 dentry 名因此被"大写拼写访问"写为 `HiCodeer` 且不再回落。
   - 即：大小写分叉不是偶发"污染"，而是"应用侧惯用大写 `HiCodeer` 拼写 + 工作区磁盘真名为小写"这一组合的稳定结果——**只要该组合存在，`caseSensitiveCheck: true` 就会持续失败**。
3. **两侧字符串碰撞点**：hvigor 从 `process.cwd()`（= getcwd = 大写）取项目路径；ArkTS 编译器 `strictMode.caseSensitiveCheck: true` 以磁盘真名（小写）做**逐字符比对** → 所有 import（含同目录文件、生成文件、依赖包）全部判为"大小写不匹配"而失败。
4. **对照组**：warp-ohos 的目录名全小写，getcwd 与 readdir 两侧一致 → 其构建（同样 `caseSensitiveCheck: true`）一直成功。

附带观察：HiCodeer 工程的 `outputs` 目录从未留下过成功 HAP（对照 warp-ohos 有 10-06 产物），即**该项目在本机历史上从未成功打包过**，与"dentry 名被污染后构建必失败"的行为一致。

## 解决方案 (Solution)

### 故障一修复：wasmtime-wasi OHOS patch（规避 openat2）

照抄 2026-08-26 cap-primitives 的成熟模式，新建本地补丁副本 `patches/wasmtime-wasi/`（wasmtime-wasi 48.0.3 全量源码复制自 crates.io），**只改 1 个文件**：

`src/filesystem/primitives/rustix/linux/fs/open_impl.rs`：

- 3 处 `#[cfg(target_os = "linux")]` → `#[cfg(all(target_os = "linux", not(target_env = "ohos")))]`（顶部 import 块、`open_impl` 内 openat2 尝试块、`open_beneath` 定义）；
- 新增 OHOS 专用桩，保留符号、直接报 ENOSYS：

  ```rust
  #[cfg(target_env = "ohos")]
  pub(crate) fn open_beneath(
      _start: &fs::File,
      _path: &Path,
      _options: &OpenOptions,
  ) -> io::Result<fs::File> {
      Err(rustix::io::Errno::NOSYS.into())
  }
  ```

- **调用方零改动**：`linux/fs/mod.rs` 的 re-export 在 OHOS 下仍指向桩（符号不缺失）；`stat_impl.rs` 拿到 `ENOSYS` 后按既有逻辑自动回退 `manually::stat`（用户态逐组件解析 + `openat`，与内核 `RESOLVE_BENEATH` 语义等价）。

`Cargo.toml` 的 `[patch.crates-io]`（OHOS 移植段）注册 `wasmtime-wasi = { path = "patches/wasmtime-wasi" }`。

**方案对比**：

- 方案 A（采用）：patch crate，OHOS cfg 排除 openat2 + ENOSYS 桩。改动局部、与既有 5 份补丁（cap-primitives / wasmtime / which / virtiofsd / rustix-openpty）同构、静态自洽。
- 方案 B（否决）：shim 垫片拦截。内联 `svc` 不经过任何符号，`--wrap` 拦不到；且其设想的 `rustix_use_libc` 通路从未接线，接线还是全局性改动。**不可行**。
- 方案 C（否决）：SIGSYS handler 改写 ucontext 模拟 syscall。重型、依赖 seccomp TRAP/KILL 策略（未证）、信号安全风险高。

### 故障二修复：关闭 ArkTS 大小写检查

`hap/build-profile.json5`：`strictMode.caseSensitiveCheck: true` → `false`。

理由：该检查的语义是"路径字符串必须与磁盘真名逐字符一致"，而本机环境里**磁盘真名（小写）与内核 dentry 名（大写）天然分叉**，任何一侧的拼写都无法同时满足，检查在此环境下不可靠。改后同一条命令 **39 秒打包成功**（此前必失败），产出 165MB 签名 HAP。

遗留说明：dentry 名不会自愈；若将来想恢复该检查，需在**设备重启**后验证 `pwd -P` 是否回落到小写真名再决定。

**已试验排除项（2026-10-07）**：把 `Setup.ets` 的 `HOME_DIR_NAME` 改为小写 `'hicodeer'`（试图消除"应用侧大写拼写"的源头）并恢复 `caseSensitiveCheck: true` 重新打包——**仍然失败**（同样的 49 个 `Please check the case`，报错路径依旧是大写）。该实验证明：

- 构建报错中的大写来自**项目目录**（`workspace/hicodeer`）的 dentry 名，与 app 的**数据主目录**常量 `HOME_DIR_NAME` 无关（两者是两个目录、两套机制）；
- 即使想通过改 app 消除大写惯例也不够：本机 app 的旧记录文件 `custom_data_dir` 已固化大写路径（`resolveExisting()` 直接读记录值、不经过常量），且 dentry 名是内核态、改任何源码/配置都不会复位它（唯一复位途径是重启设备重建 dentry，且之后需保证无任何大写拼写访问，不可控）。

该实验的两处改动已回滚（`caseSensitiveCheck` 恢复 `false`、`HOME_DIR_NAME` 恢复 `'HiCodeer'`）。

**补充排查（hap 全目录含生成物的大写分布）**：对 `hap/` 全目录（跟随符号链接、含所有生成物）搜索大写 `HiCodeer`，结果分两类：

- **源码/资源共 9 处，均与路径行为无关**：`Setup.ets`（常量与注释，已实验排除）、两处 `string.json` 的"应用显示名"、`oh-package.json5` 描述文字、`EntryAbility.ets` 注释——**源码里不存在"以大写拼写访问项目目录"的代码**。
- **生成物大量命中，且全部记录了【大写】项目绝对路径**：ArkTS 编译缓存（`hap/entry/build/default/cache/.../esmodule/.ts_checker_cache`、`modules.cache`、`filesInfo.txt`，数百处）、oh_modules 内 `@ohos-rs` 插件的 CMake/ninja 还原文件（`CMakeConfigureLog.yaml`、`build.ninja`）、`.hvigor` 的日志/report/sync output。
- 机制：这些路径是 hvigor/CMake **运行时从 getcwd（内核 dentry 名）取到的项目路径**的固化记录；每次构建读写这些缓存/中间件都在以大写拼写访问项目目录，**反过来持续把 dentry 名"钉"在大写**——自成闭环。即大写不存在于任何源码中，而是"内核 dentry 名 ↔ 生成物缓存"互相维持。
- 因此若要让 `caseSensitiveCheck: true` 可用，需同时满足：重启设备复位 dentry 名 + 清理上述生成物（清缓存属人工操作）+ 之后全程以**小写拼写**启动构建与会话（含 Claude/codebuddy 会话的 cwd）；任一条件被打破即复发。

**dentry 名固定机制（小目录实验，2026-10-07）**：在 `~/tmp/caseprobe` 上实测——小写拼写创建目录后，用大写拼写进入（`cd .../CASEPROBE`）再 `pwd -P` **仍返回小写**；大小写交替进入均不改名。结论：**dentry 名在"目录创建/首次被访问"时固定，之后的访问拼写不会改写它**。据此推断：`workspace/hicodeer` 的大写 dentry 名 = 历史上**第一次访问它的进程用了大写拼写**；且**重启设备（dcache 清空）后，只要"第一次访问"用小写拼写，dentry 名即可固定回小写**——这是恢复 `caseSensitiveCheck: true` 的可行路径，前置条件是先清掉所有记录大写路径的生成物（防构建时被读回大写）。

**生成物清理记录（2026-10-07，用户授权）**：已删除 `hap/entry/build/`（含 ArkTS 编译缓存与旧 HAP）、`hap/.hvigor/` 内容、`hap/entry/.cxx/` 内容、`hap/entry/oh_modules/**/.cxx`（9 个）、`hap/entry/oh_modules/**/@ohos-rs/ability/build`（9 个）——复查 `grep -Rl "workspace/HiCodeer" hap/` 为空，即所有含大写路径的生成物已清空；未执行任何构建，缓存交由编译器下次构建时自行重建。

**状态更新（2026-10-07，用户决定）**：`caseSensitiveCheck` 已改回 `true` 并保持至今。最初计划是「重启设备复位 dentry 名」实验（约定流程：重启后第一次访问项目必须用小写拼写、由用户手跑构建）；**该实验未按重启路线走**——同日 16:00 前后查明了完整注入链并找到**确定性修复**（见下节），**无需重启**。

**根因收口（2026-10-07 16:00，最终版）**：把"大写 HiCodeer"的来路钉死为**两条实测事实的串联**：

1. **源头 = 内核 dcache 里该目录项的名字**：磁盘真名（readdir 视角）= `hicodeer`（小写）；内核缓存名（`getcwd` 视角）= `HiCodeer`（大写）——同一实体（三种拼写 `stat` 的 inode 完全一致）、两个名字视图（hmdfs 上的名字分叉；`/storage/Users/currentUser` 挂载为 `hmdfs` 类型）。该分叉**无法用改名修复**：`mv`、`rename(2)`、`renameat2(RENAME_NOREPLACE)` 对它是**一致 ENOENT**（同文件系统新建目录的同类操作全部正常；`RENAME_EXCHANGE` 经小目录验证可用，但未对其执行——属高风险操作被权限层拦截）；`/proc/sys/vm/drop_caches` 无权限；`chdir` 不会改写 dcache 名（实测 `PWD` 变量=小写而 `pwd -P`=大写并存）。
2. **进入构建的唯一通道 = hvigor 的 `process.cwd()`**：`script/bundle-ohos` 调用 hvigor **不传项目路径参数**（`--mode module … assembleHap`，见脚本 1022 行附近），hvigorw.js（`commandline-ohos/hvigor.org/hvigor_1.0.0/bin/hvigorw.js`）中 **`HVIGOR_PROJECT_ROOT_DIR = process.cwd()` 为无条件赋值**（环境变量不可覆盖）→ Node 的 `getcwd()` = dcache 名 = **大写** → hvigor/ArkTS 编译器全链路径大写 → 与磁盘真名逐字符比对失败（50 个 "Please check the case"）。**与启动 shell 的拼写无关**——`getcwd` 读的是 dcache，小写 cd 重跑同样失败。
3. **实测日志对比（hvigor build.log）**：15:46 失败构建 `workspace/HiCodeer` 命中 869 次（100% 大写）；16:03 修复后构建 `workspace/hicodeer` 命中 505 次（100% 小写）、case 报错 0。

**修复方案（已验证；2026-10-07 用户决定"先不落地"）**：Node 层 `process.cwd()` 修正补丁——以 `NODE_OPTIONS="--require <脚本路径>"` 预载，把 cwd 逐段替换为父目录 readdir 报告的真实名字（`HiCodeer`→`hicodeer`；对正常文件系统为 no-op，零副作用；不碰目录、不碰 dcache、不需重启）。**验证结果**：debug `--hap-only` **41 秒 BUILD SUCCESSFUL**（165,834,032 字节 HAP）；**release 全量构建成功**（`Mode: release`、hvigor 41s 589ms、签名 verify 通过、`entry-default-signed.hap` 139,831,125 字节、staged release 版 libcore.so 272,205,160 字节）。**不落地时的使用形态**：手动 `NODE_OPTIONS="--require <脚本路径>" script/bundle-ohos [--release]`。补丁脚本全文存档（原临时文件按规则已清理）：

```js
// Patch process.cwd() to return each path segment spelled the way readdir
// reports it. On this device the user storage is hmdfs: the kernel dcache may
// keep a stale case variant for a directory (getcwd -> ".../HiCodeer") while
// readdir reports the real on-disk name ("hicodeer"). Tools such as hvigor
// derive the project root from process.cwd(), so ArkTS caseSensitiveCheck
// then rejects every path. Rewriting each segment with the readdir spelling
// makes those consumers see the real names. On case-sensitive or consistent
// filesystems every lookup already matches and this is a no-op.
'use strict';
const fs = require('fs');
const originalCwd = process.cwd.bind(process);

function readdirNameOf(parent, name) {
  let entries;
  try {
    entries = fs.readdirSync(parent);
  } catch (error) {
    return name;
  }
  for (const entry of entries) {
    if (entry.length === name.length && entry.toLowerCase() === name.toLowerCase()) {
      return entry;
    }
  }
  return name;
}

function withReaddirSpelling(absolutePath) {
  const segments = absolutePath.split('/').filter((segment) => segment.length > 0);
  let current = '/';
  for (const segment of segments) {
    const realName = readdirNameOf(current, segment);
    current = current === '/' ? '/' + realName : current + '/' + realName;
  }
  return current;
}

process.cwd = function patchedCwd() {
  try {
    return withReaddirSpelling(originalCwd());
  } catch (error) {
    return originalCwd();
  }
};
```

**拟议落地（未执行）**：① 新增 `script/hvigor-working-directory-case-fix.js`（上述全文）；② `script/bundle-ohos` 在 hvigor 调用行前注入 `NODE_OPTIONS`（一行）。两处均在项目内、不涉其它平台。

**备选路线（重启）的可行性备注**：重启可清空 dcache、给出把名字复位为小写的窗口；但成败取决于"重启后第一次访问该目录的拼写"（若被大写访问抢先则前功尽弃），且本机存在多个常驻进程（claude/zsh/rust-analyzer/node 等，实测 18 个持 cwd 于项目内）可能抢先，**非确定性**；用户已知悉，暂未执行。

## 修改文件 (Modified Files)

- `patches/wasmtime-wasi/`（新增）— 从 crates.io 复制 wasmtime-wasi 48.0.3 全量源码；与上游逐文件 diff 后**仅 `src/filesystem/primitives/rustix/linux/fs/open_impl.rs` 有内容改动**（3 处 cfg 加 `not(target_env = "ohos")` + OHOS ENOSYS 桩 + 说明注释），其余 218 个文件与上游逐字节一致。
- `Cargo.toml` — `[patch.crates-io]` OHOS 段新增 `wasmtime-wasi = { path = "patches/wasmtime-wasi" }` 及 4 行说明注释。
- `Cargo.lock` — wasmtime-wasi 来源由 registry 变为本地 path（cargo 自动更新）。
- `hap/build-profile.json5` — `strictMode.caseSensitiveCheck`：原 `true` 一度改为 `false`（规避 dentry 名与磁盘真名大小写分叉导致的 ArkTS import 解析全灭）；2026-10-07 用户决定改回 `true` 并保持至今（配合下述 `process.cwd()` 修正补丁，`true` 下 debug/release 构建均已实测通过）。
- （拟议、未落地——2026-10-07 用户决定"先不落地"）`script/hvigor-working-directory-case-fix.js`（新增）与 `script/bundle-ohos`（hvigor 调用行前注入一行 `NODE_OPTIONS`）——见上「根因收口」节；补丁全文已存档于本报告。
- `README.md` — 按项目 CLAUDE.md 的 HARD RULE 要求在文件头添加两行 PR review 标记。
- （验证用临时日志均落在 `~/tmp/`，验证结束后已清理。）

## 验证 (Verification)

- `cargo check -p wasmtime-wasi --target aarch64-unknown-linux-ohos`（`RUSTFLAGS="-A warnings"`，与正式构建一致）通过，6m24s。
- 全量 debug 构建：`script/bundle-ohos` → `=== HAP BUILD SUCCESSFUL ===`，签名产物 `hap/entry/build/default/outputs/default/entry-default-signed.hap`（165,834,032 字节）。
- **修复进产物的直接证据**：构建日志 `Compiling wasmtime-wasi v48.0.3 (/storage/Users/currentUser/workspace/HiCodeer/patches/wasmtime-wasi)`（两次全量构建均出现）。
- 覆盖安装（`./install-local.sh`，未卸载、沙箱数据保留）后实机运行：工作区扫描（6000+ 文件）、rust-analyzer / JSON language server / package-version-server 正常拉起，内存正常，**无 SIGSYS、无崩溃**（旧包前台 45 秒必崩）。
- **用户已确认问题解决**。

## 关联 (Related)

- `2026-08-26-ohos-wasm-runtime-permission-and-openat2-crash.md` — cap-primitives 的**同源代码第一份** openat2 修复（含 OHOS seccomp 白名单调研结论）；本报告是同类问题在 wasmtime-wasi 内嵌第二份副本中的再现与收口。两者此后应一并检查：上游生成新副本（如 wasmtime 升级）时需重新核对。
- `[[ohos-debug-lessons]]`
