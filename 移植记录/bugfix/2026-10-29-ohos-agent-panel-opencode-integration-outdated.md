# Agent 面板里 opencode 集成永远显示"未安装"

## 问题描述 (Problem Description)

设置 → Panels → 智能体面板（Agent Panel）里有一个「立即检查」按钮，用来重新扫描各 AI CLI 的状态上报集成装没装、装到哪个版本。实测中 opencode 这一格始终没有 ✓，看起来像"自动安装没装上"；但磁盘上它的插件文件早就存在，而且每次检查（包括每 60 秒一次的自动检查）都会把它们重写一遍，状态却永远不变成"已安装"。

## 问题表现 (Symptoms)

- 面板网格里 claude、deveco 等 target 显示绿色 ✓，opencode 没有。
- 直接向 daemon 的 `agent.sock` 发 `integration.list`，opencode 返回 `state: "outdated"`。
- 与此同时磁盘上三份文件俱全且主插件版本正确：

```
~/.config/opencode/plugins/hicodeer-agent-state.js   HERDR_INTEGRATION_VERSION=14
~/.config/opencode/hicodeer-tui-session.js           HERDR_INTEGRATION_VERSION=14
~/.config/opencode/hicodeer-opencode/tui.js          HERDR_INTEGRATION_VERSION=13
```

- 每次检查都会重写 opencode 的 3 个插件文件 + 2 处配置（`tui.jsonc` 的 plugin 列表、`cli.json` 的 plugins 列表），属于无效写盘。
- 复现：opencode 命令在 PATH 上（本机是 `~/.harmonybrew/bin/opencode`）→ 打开设置 Panels 页或点「立即检查」→ 观察 opencode 单元格始终无勾。

## 问题原因 (Root Cause)

**同一个版本号存在两处权威**：Rust 常量，和资产文件里的一行注释。

- `OPENCODE_INTEGRATION_VERSION = 14`（`herdr/src/integration/mod.rs`）
- 每个资产文件自带 `// HERDR_INTEGRATION_VERSION=N`，安装时原样写到磁盘，检查时再把这一行解析回来跟常量比。

opencode 一个 target 挂三个资产，其中 `assets/opencode/tui.js` 是 V2 的入口垫片，内容只有三行：

```js
// installed by HiCodeer
// HERDR_INTEGRATION_ID=opencode-tui-v2
// HERDR_INTEGRATION_VERSION=13
export { default } from "../hicodeer-tui-session.js";
```

它自 2026-10-23 起就没有再改过。2026-10-24 的提交把整个 opencode 集成版本抬到 14（同时修正了 hook 读取的环境变量名），只改了带逻辑的两个资产，漏掉了这个 3 行转发垫片——它不含逻辑，不会被任何功能改动自然带动。

判定链因此断在最后一环：

```
磁盘主插件 14 >= 常量 14            → 初判 Current
  ↓ opencode 特判（registry.rs: opencode_tui_integration_is_valid）
tui-session 14 >= 14                → 通过
cli.json 里登记了 ./hicodeer-opencode → 通过
V2 垫片 13 >= 14                    → 失败 ⇒ 整项降级为 Outdated
  ↓
client: needs_install = (state == Outdated) → true，永远可装
  ↓
settings_ui: 只有 state == Current 才画 ✓ → 永远不画
```

后果是自洽的：状态永远 Outdated → 永远"需要安装" → 每 60 秒自动检查都重装一次 → 重装写入的垫片仍然是 13 → 状态仍是 Outdated。

补充事实：这套"常量 + 资产内 marker"的双权威并非只坑了 opencode，`claude`/`codex`/`zcode`/`atomcode`/`codewhale` 的 `.ps1` 资产版本比各自常量低 1，在 Windows 上同样会永远判过期。本次刻意只修 opencode 这一条，不扩大范围。

## 排查过程（死路记录）

1. **怀疑 opencode 命令不可见**：daemon 由系统拉起，不继承登录 shell 的 `PATH`，npm/bun 装的 agent 会因此被判 unavailable。查 `registry.rs` 的 `command_available` 有 `~/.npm/bin`、`~/.bun/bin`、`~/.local/bin`、`~/.harmonybrew/bin` 兜底，且 `integration.list` 返回 `available: true`，排除。
2. **怀疑根本没装上**：三个文件都在，mtime 与自动检查时间吻合，排除。
3. **怀疑 daemon 还在跑旧代码**：`hicodeerd` 不随 HAP 覆盖安装重启，旧进程跑旧 inode。核对 `/proc/<pid>/exe` 与文件 mtime 后确认（本次修复也依赖这一点做生效验证），但即便换成最新代码，状态仍是 outdated —— 说明问题不在新旧代码，而在版本数据本身。
4. 最终定位方式：把 `integration.list` 的 state 与资产文件里的 marker 摆在一起对比，13 与 14 的缺口一眼可见。

## 解决方案 (Solution)

核心思路：**让校验跟着资产走，而不是跟着 target 级常量走**。垫片只跟它自己的资产比版本，不再被主插件的 14 拖着。

新增一个通用读取函数（`herdr/src/integration/mod.rs`）：

```rust
/// The version an embedded asset declares for itself.
///
/// Checks use this when they must follow the asset rather than the target-wide
/// constant, so bumping the asset is the only edit a version change needs.
pub(crate) fn declared_version(asset: &str) -> u32 {
    registry::parse_integration_version(asset).unwrap_or(0)
}
```

opencode 的 V2 垫片比较改为（`herdr/src/integration/registry.rs`，`opencode_tui_integration_is_valid` 内）：

```rust
// 修改前：拿主集成版本要求垫片
.is_some_and(|version| version >= expected_version)      // 13 >= 14 → false

// 修改后：拿垫片自己声明的版本
.is_some_and(|version| {
    version >= super::declared_version(super::OPENCODE_V2_TUI_PLUGIN_ASSET)
})                                                       // 13 >= 13 → true
```

为什么不用另外两条更省事的路：

- **把常量 `14` 改成 `13`**：主插件、垫片都能过，但常量从此不再代表"当前集成版本"。将来主插件升到 15 而常量忘改时，`15 >= 13` 静默通过，用户永远拿不到新钩子——把眼前这个 bug 换成更难发现的版本。
- **在判定里直接写 `13`**：等于让"13"同时出现在资产文件与判定代码两处，垫片真升级时又要人肉同步，还是同一个坑。
- **改成内容比对**（`磁盘内容 == 内置资产`）：同样只改一处，但会把"用户手工改过钩子"判成过期并覆盖，语义比版本比较更激进，本次不动。

影响面：只改了 opencode 一个 target 的校验；不碰资产内容、不碰安装写入逻辑、不影响另外 21 个 target。

## 修改文件 (Modified Files)

- `herdr/src/integration/mod.rs` — 新增 `declared_version(asset)`，从内嵌资产里读出它自己声明的版本（解析失败返回 0，方向上偏保守：判过期会触发重装而不是静默放过）。
- `herdr/src/integration/registry.rs` — `opencode_tui_integration_is_valid` 里 V2 垫片的版本比较由 `expected_version`（target 级常量）改为 `declared_version(OPENCODE_V2_TUI_PLUGIN_ASSET)`。

## 验证

- `cargo check --lib --bins`（herdr）：EXIT=0，29.5s。
- `cargo build --release --bins`：`Finished release profile [optimized]`，26m58s，产物 92 MB。
- `bash script/bundle-ohos --release`：`=== HAP BUILD SUCCESSFUL ===`，56m21s。
- `./install-local.sh --no-start`：安装成功；`/data/service/hnp/hicodeerd.org/hicodeerd_0.1.0/bin/hicodeerd` 由 110 MB 换为 strip 后的 59 MB，mtime 更新（覆盖安装即替换 hnp 内容，只是进程不跟着重启）。
- 重启 daemon：`killall hicodeerd` → `setsid /data/service/hnp/bin/hicodeerd --log`，新 pid 起于 08:52。
- 直连 `/data/storage/el2/base/haps/entry/files/.hicodeer/agent/agent.sock` 发 `integration.list`：

```json
{"target":"opencode","label":"opencode","command":"opencode","available":true,"state":"current"}
```

  修复前同一查询为 `"state":"outdated"`。
- 面板：opencode 单元格出现绿色 ✓，且后续自动检查不再重写它的插件文件。

## 遗留

- 其他 target 仍是"常量 vs 资产内 marker"双权威（`.ps1` 那批落后 1 的问题仍在，鸿蒙侧不受影响，Windows 侧会误判过期）。本次按最小影响面处理，未推广 `declared_version`。
- 资产 `assets/opencode/tui.js` 里的 `HERDR_INTEGRATION_VERSION=13` 现在不参与比较，成为无害数据；保留是为了让"这份资产自称什么版本"仍然可读。若将来垫片内容真要改，按约定抬这个 marker 即可，校验会自然跟上。
