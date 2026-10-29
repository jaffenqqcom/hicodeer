# 去 herdr 商标：把上游品牌从交付物里剥离

- 日期：2026-10-29
- 范围：`herdr/`（内嵌的 agent server）、`crates/agent_panel/`、`agent-tests/e2e/`
- 结论：四类对外面已全部换成 HiCodeer 品牌；源码标识符、目录名与第三方标识按约定保留原样。

## 1. 目标与边界

HiCodeer 的交付物里不得出现上游产品名 `herdr`（与「去 Zed 化」并列的两条品牌线）。
品牌只从**用户能看见、能读到、能落到盘上**的四类面剥离：

| 面 | 判据 | 处理方式 |
|---|---|---|
| 二进制内的字符串 | `strings libcore.so / hicodeerd / hicodeer-agent` 可读出 | 全部由品牌常量派生 |
| 运行时落盘产物 | 目录名、socket 名、日志文件名、hook 文件名 | 同上，改名后重新生成 |
| 日志内容 | tracing target、env-filter、启动/退出横幅、错误前缀 | target 与文案同步改 |
| 用户可见文案 | 帮助、报错、onboarding、右键菜单、toast、SKILL.md、API schema title | 文案改写为新品牌 |

**不在范围内**（有意保留）：Rust 模块名 / 变量名 / 结构体名里带 `herdr` 的标识符、`herdr/`
这个目录名本身、`docs/next/api/herdr-api.schema.json` 这个文件名、`herdr/skills/herdr/`
目录。判据是它们不进用户配置、不进协议字符串、不出现在界面上。

版本尚未发布，因此**不做老机器兼容**：不写「把旧的 herdr 目录/配置迁移过来」的代码，
只保证新装机器上从第一次启动起就只有新品牌。

## 2. 品牌的单一来源

新增 `herdr/src/brand.rs`，五个常量，其余全部派生：

```rust
pub(crate) const NAME: &str = "HiCodeer";                    // 界面与横幅
pub(crate) const DIR: &str = "hicodeer/agenttui";            // $XDG_CONFIG_HOME / $XDG_STATE_HOME 下
pub(crate) const FILE_PREFIX: &str = "hicodeer";             // socket / 日志 / 临时文件 / 状态文件
pub(crate) const COMMAND: &str = "hicodeer-agent";           // 帮助文本与诊断信息
pub(crate) const SESSION_KEY_PREFIX: &str = "hicodeer";      // API 上报的 <prefix>:<agent>
```

派生出来的固定值：

- 环境变量族：`HICODEER_AGENT_*`（pane id、socket 路径、二进制路径、声音文件、集成版本标记……）
- socket：`<XDG_CONFIG_HOME>/.hicodeer/agent/agent.sock`（`hicodeerd` 侧同名，两侧必须一致）
- 会话来源：`hicodeer:<agent>`（`official_source_for()` 由 `SESSION_KEY_PREFIX` 拼出）
- hook 文件名：`hicodeer-agent-state.sh / .js / .ts / .ps1`
- 更新与分发：`HICODEER_AGENT_UPDATE_MANIFEST_URL`、`HICODEER_AGENT_PREVIEW_MANIFEST_URL`、
  `HICODEER_AGENT_HOMEBREW_FORMULA_URL`（不再硬编码上游域名，未配置时明确报错而不是去撞别人的服务器）

## 3. 四类面的落地

### 3.1 二进制内字符串

- 环境变量前缀统一为 `HICODEER_AGENT_`；读侧与写侧必须同改（`herdr/src/sound.rs` 的
  Windows 脚本读 `HICODEER_AGENT_SOUND_PATH`，写侧常量同名）。
- 诊断前缀 `herdr:` → `hicodeer-agent:`；CLI 帮助里的程序名走 `brand::COMMAND`。
- Windows conpty 的开关变量与临时文件名（`herdr/vendor/portable-pty/src/win/psuedocon.rs`）。
- 远端分发：`herdr.exe` / `herdr-dev.exe` → `hicodeer-agent.exe` / `hicodeer-agent-dev.exe`，
  探针标记 `herdr-api-bridge-v1` → `hicodeer-api-bridge-v1`（生产侧与探针侧必须同一常量）。

### 3.2 落盘产物

| 原值 | 新值 |
|---|---|
| `$XDG_*/herdr/` | `$XDG_*/hicodeer/agenttui/` |
| `herdr.sock` | `hicodeer-agent.sock`（daemon 侧 `agent.sock` 不变） |
| `herdr.log` / `herdr-*.tmp` | `hicodeer-agent.log` / `hicodeer-*.tmp` |
| `herdr-agent-state.sh` | `hicodeer-agent-state.sh` |

改名后旧路径不再被创建，也不清理——新装机器不存在旧路径。

### 3.3 日志内容

tracing 的 target 与 `RUST_LOG` 过滤规则同步改名；启动/退出横幅用 `brand::NAME`；
panic/错误信息里的产品名统一。日志文件名走 `FILE_PREFIX`。

### 3.4 用户可见文案

帮助文本、错误提示、onboarding 页、pane 右键菜单、toast、`herdr/skills/herdr/SKILL.md` 的
标题与正文、API schema 的 `title` 描述。右键菜单的「在应用内打开 / 在 pane 内打开」取值从
`herdr` 改为 `in_app`（枚举只接受 `in_app`，这一点被批量替换误伤过，见 §6.2）。

## 4. 兼容层清理的取舍

`herdr/src/integration` 里有一批「清理上一代 herdr 安装」的代码。按「只考虑新装机器」的
口径，凡是**取值就是 herdr 旧标识**的，删掉（留着与目标直接冲突）：

- `LEGACY_HOOK_FILE_NAMES`（`herdr-agent-state.*`）、`legacy_bash_hook_path()`、
  `remove_legacy_bash_hook_file()`（内含 `HERDR_INTEGRATION_ID=`）
- `LEGACY_OMP_EXTENSION_INSTALL_NAME`、`LEGACY_OPENCODE_*`、`LEGACY_HERMES_PLUGIN_NAME`
- `removed_legacy_pi_extension` 字段与其分支

**品牌无关**的兼容逻辑（例如 `hook_command_variants()` 里保留「当前路径的 bash 变体」、
`legacy_bash_hook_command()` 的通用形态）保持原样，不借品牌改名顺手删功能。

## 5. 验证方法

用「反向归一化」而不是肉眼比对，三步：

1. **纯替换判定**：把工作区文件里的新名还原成旧名（`hicodeer-agent`→`herdr`、
   `hicodeer`→`herdr`、`HiCodeer`→`Herdr`、`HICODEER_AGENT_`→`HERDR_`），与 `git HEAD`
   的同名文件逐字节比较。相等 ⇒ 这个文件只做了品牌替换，没有夹带其它改动。
2. **字面量计数守恒**：只统计字符串字面量里的品牌词次数，旧文件 vs 新文件必须相等，
   用来抓「漏替换」与「多替换」。
3. **残留扫描**：扫描仍含品牌词的字面量，白名单只放三类：
   `herdr-api.schema.json`、`skills/herdr`、第三方仓库名。

跳过软链（本机 OHOS 授权对跟随访问返回 EACCES）与构建缓存目录（`target`、`.zig-cache`、
`node_modules`、`oh_modules`、`build`）。

## 6. 结果

- **135 个文件**为纯品牌替换（归一化后与基线逐字节一致）；字面量计数不守恒 0 项；
  残留 1 处，是有意保留的第三方 GitHub 仓库名 `OGULCANSLIK/HERDR-PLUGIN-EXAMPLES`。
- 全量构建：`script/bundle-ohos` → `EXIT=0` + `HAP BUILD SUCCESSFUL`，HNP 内的
  `hicodeerd` 与 HAP 同时换新。
- 运行期（新 daemon）：`integration.list` 12 个可用集成全部 `current`；hook 文件的
  `HICODEER_AGENT_INTEGRATION_ID/VERSION` 齐全；安装产物里 0 处 `herdr` 字符串。
- hook 上报：codewhale / deveco 各 9 项全过（含审批态 `blocked`）。
- 会话恢复：claude / deveco / zcode / atomcode / codewhale 各 12 项全过，
  `agent_session.source == hicodeer:<agent>`。

## 7. 踩过的坑

1. **脚本化替换误伤标识符**：`remote_herdr` → `remote_hicodeer-agent`、
   `herdr.executable` → `hicodeer-agent.executable`，以及 86 处函数名/测试名里的品牌词。
   修法：只替换字符串字面量与注释，代码段按语法还原；之后以「归一化后逐字节相等」
   作为验收，而不是靠人眼。
2. **CLI 取值被改坏**：`--right-click` 的合法值只有 `in_app`，批量替换把它写成了
   `hicodeer-agent`，解析器直接拒绝。修法：值域枚举与品牌常量分开，枚举值不参与替换。
3. **读写两侧不同步**：Windows 声音脚本读的是旧变量名，写侧已换新 —— 表现为通知永远静默。
4. **探针与生产端不同常量**：远端桥接探针比较 `herdr-api-bridge-v1`，生产端已写
   `hicodeer-api-bridge-v1`，探针永不命中。
5. **凭空的文件名**：API schema 快照路径被替换成不存在的 `hicodeer-api.schema.json`，
   测试一跑就 panic。修法：文件名不在替换范围，恢复原名并用
   `HICODEER_AGENT_UPDATE_API_SCHEMA=1` 重新生成快照。
6. **旧常量自撞**：`LEGACY_OMP_EXTENSION_INSTALL_NAME` 被替换成与当前名相同，导致同一次
   安装删两遍。修法：直接删除该常量与其调用点。
7. **同一批验证里暴露的 claude settings 缺陷**（与本主题无关，但同一分支修复）：
   `install()` 已经要写 4 类 hook 事件，而格式化保留层只重建 `SessionStart`，
   导致真机上 `integration.install claude` 必然失败。修法：CST 层按目标对象补齐所有缺失
   事件，插入点取「最后一个元素的结束位置」以保住用户原有字节；同时修掉「删除阶段误删
   用户未匹配到的空 hook 组」这一数据丢失问题。

## 8. 边界与后续

- 老机器上的 `herdr/` 目录与旧 hook 文件不做清理，新装机器不受影响。
- `herdr/` 这个 crate 目录名、Rust 标识符、`herdr-api.schema.json` 文件名保留，
  若后续要做「源码级去品牌」，是另一条独立工作线。
- Zed 侧的字符串去品牌属「去 Zed 化」线，本文不涉及。
