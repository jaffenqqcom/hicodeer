# 命令面板把 zed 显示成 hicodeer（仅显示层）

## 问题描述 (Problem Description)

命令面板（Ctrl+Shift+P）输入 zed 会列出大量带 zed 字样的命令：`zed::` 命名空间的 action
（`crates/zed_actions/src/lib.rs:38` 的 `actions!(zed, [...])`、同文件 :17/:25/:33 的
`#[action(namespace = zed)]`，以及 `crates/onboarding/src/onboarding.rs:42/:51`、
`crates/extensions_ui/src/extensions_ui.rs:60` 等）经 `humanize_action_name` 显示成
"zed: open settings" 这类文本。移植目标要求用户看到 hicodeer，但不改 workspace 名、不改 action
真实 id、不动 keymap/settings 兼容性，避免大范围改动。

## 问题表现 (Symptoms)

- 面板输入 zed 命中一串 "zed: ..." 命令（zed: open settings、zed: quit、zed: about 等）；
- 另有名字里嵌 zed 的：`zed::OpenZedUrl` → "zed: open zed url"，`feedback::EmailZed`
  （`crates/zed_actions/src/lib.rs:435-439`）→ "feedback: email zed"；
- 品牌上暴露上游名字，与 hicodeer 移植目标冲突。

## 问题原因 (Root Cause)

显示名唯一生成点在命令面板构造处（`crates/command_palette/src/command_palette.rs`，下同）：

- `CommandPalette::new`（:122-136）遍历 `window.available_actions(cx)`，用
  `humanize_action_name(action.name())`（:131，函数体 :867-932）把 `zed::OpenSettings` 转成
  "zed: open settings"，装进 `Command { name, action, usage }`（:212-216）。`name` 从一开始就是
  纯显示/搜索字符串，`action` 才是真实 id 载体——二者分离，只是 `name` 带着上游字样。

`name` 的全部消费点：

- 模糊搜索：`update_matches`（:580）在 :612-616 用 `StringMatchCandidate::new(ix, &command.name)`
  建候选，:618 `fuzzy_nucleo::match_strings_async` 拿它匹配——搜索用的就是显示名；
- 高亮：`render_match`（:764）在 :786-790 用
  `HighlightedLabel::new(command.name.clone(), matching_command.positions.clone())`，positions 是
  显示名上的字符索引；
- 历史：`confirm`（:714）在 :750-757 把 `command.name` 写入命令历史库
  （`crates/command_palette/src/persistence.rs:16/:77`），`command_usage`（:393-414）在 :608 按
  同一 name 回查 usage。

而执行/键位/过滤全都不走 `name`：:761 `window.dispatch_action(action, cx)` 派发的是
`command.action`（真实 boxed action）；键位提示 :796 与 footer :819-820 用
`KeyBinding::for_action_in(&*command.action, ...)`；secondary confirm 在 :722-726 用
`action.name()`（真实 id，喂给 keymap 编辑器）；`command_palette_hooks.rs` 的
`CommandPaletteFilter::is_hidden`（:55-66）也按 `action.name()` 判命名空间。

即：显示、搜索、高亮、历史 key 同为一串；执行、键位、过滤、keymap 全走真实 action。这决定了
「只换 name」即可同时满足显示与搜索，且不碰任何行为。

## 解决方案 (Solution)

### 为什么选「只换显示名」，不改 action id / workspace 名

- 改 action id（`zed::` → `hicodeer::`）要动 zed_actions、onboarding、extensions_ui、feedback
  等所有声明点，还会破坏用户 keymap/settings 里按真实 id 写的引用及 `deprecated_aliases`
  机制，改动面与兼容风险都不符合约束；
- 改 workspace 名与本题无关且被明确禁止；
- 显示层单点替换：搜索/高亮天然跟随（同一字符串），执行/键位天然不受影响（不同通路），是最小改动。

### 改法（修改前后对比）

生成点包一层 token 级替换（command_palette.rs）：

```rust
// 修改前（:131）
name: SharedString::from(humanize_action_name(action.name())),

// 修改后（:131）
name: SharedString::from(palette_command_name(action.name())),
```

新增 `palette_command_name`（:934-950）：

```rust
/// Builds the command palette display name, rebranding `zed` as `hicodeer`
/// while leaving the underlying action ids untouched.
fn palette_command_name(action_name: &str) -> String {
    let humanized = humanize_action_name(action_name);
    if !humanized.contains("zed") {
        return humanized;
    }
    humanized
        .split(' ')
        .map(|word| match word {
            "zed" => "hicodeer",
            "zed:" => "hicodeer:",
            _ => word,
        })
        .collect::<Vec<_>>()
        .join(" ")
}
```

为什么是 token 级、只匹配小写 `"zed"`/`"zed:"`：`humanize_action_name` 输出里命名空间前缀是
"zed: "（:875-881 的冒号分支），`Zed` 这类单词经大写拆分后也落成小写独立 token（:885-924），
所以 "zed" 恒为空格分隔的完整 token，不存在子串误伤（"AGENTS.md" 等不受影响），也无需处理大写
变体。配套单测见 `test_palette_command_name_rebrands_zed`（:1002-1010）。

## 修改文件 (Modified Files)

- `crates/command_palette/src/command_palette.rs`
  - :131 `CommandPalette::new` 改调 `palette_command_name`；
  - :934-950 新增 `palette_command_name`（token 级 zed → hicodeer）；
  - :1002-1010 新增单测 `test_palette_command_name_rebrands_zed`（覆盖命名空间前缀、
    `OpenZedUrl` 双重替换、非 zed 名不动三个样例）。
- `移植记录/bugfix/2026-10-31-hicodeer-command-palette-rebrand.md`（本记录）

## 验证

- 未执行任何编译（约定由总指挥统一跑 cargo）；本机 toolchain 缺 rustfmt 组件，格式按仓库既有
  风格手写，待编译/测试时一并确认。
- 新增单测断言三个代表样例；既有 `test_humanize_action_name`、`test_normalize_query` 不含 zed
  字样，不受影响。
- 静态核对链路：搜索候选（:612-616）与高亮（:786-790）用的都是替换后的 `command.name`，输入
  hicodeer 可命中并正确高亮；执行（:761）与键位（:796）走 `command.action`，行为不变。

## 遗留

- 旧命令历史一次性失配：历史库按 `command.name` 存取（:750-757 写、:608 回查），改名后旧记录
  里的 "zed: ..." 行不再匹配新显示名，对应命令的 usage 计数等效清零一次；旧记录本身无害残留。
  仍想输 zed 的用户可配 `command_aliases` 设置（`update_matches` :587 先查
  `WorkspaceSettings.command_aliases`）把 "zed" 映射到 "hicodeer"。
- which_key 与 keymap 编辑器仍显示 zed：它们各自直调 `humanize_action_name`
  （`crates/which_key/src/which_key.rs:97`、`crates/which_key/src/pending_bindings.rs:350/:515/:751`、
  `crates/keymap_editor/src/keymap_editor.rs:1759/:2587/:2897`，另有
  `crates/zed/src/main.rs:1951` 的 keymap 文档生成）。本次按「只影响命令面板」的约束未动，
  存在跨 UI 名称不一致，后续如需统一可把替换下沉到 `humanize_action_name` 或抽公共包装。
