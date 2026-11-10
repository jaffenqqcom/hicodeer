# Agent 选择题面板上方的引子未推送到 IM（codex / gemini / kimi）

## 问题描述

codex、gemini、kimi 三个 agent 弹出选择题等待确认时，TUI 上除了面板本身，面板上方还会先写一段回复作为引子（「随口说一句……」「说点无关的……」「下面这题，用 ↑↓ 选，回车确认：」）。推送到 IM 的正文只有面板内的题目与选项，引子整段缺失。

同一问题在 minimax 上已于更早一轮修复，本轮补上其余三个 agent，并在代码检视时发现并修掉了同源的第二个缺陷（面板滚动时读到上一轮回复）。

## 问题表现

- codex：TUI 上是「• 又来了，这次随口说一句…… / 题目：穿越到古代……」+ 面板；IM 只收到「穿越到古代…… + 四个选项」。
- gemini：TUI 上是「✦ 随口说一句：crates/ 里那个 fuzzy_nucleo…… / 题目换了，用上下键选：」+ 框线面板；IM 只收到题干与选项。
- kimi：TUI 上是「● 说点无关的：章鱼有三颗心脏…… / 下面这题，用 ↑ ↓ 选，回车确认：」+ 规则线面板；IM 只收到「如果只能额外获得一种「超能力」……」+ 选项。
- 三个 agent 在 agent.list 中都是 blocked，pane.read 能取到完整屏幕，说明状态与屏幕都正常，问题只在正文提取。

## 问题原因

三个 agent 的选择面板都画在屏幕中部，提取器此前只从面板内部读取，面板上方的回复块从未被纳入。

三个 agent 的面板顶部标记各不相同，这也是各自需要专用读取的原因：

- codex：引子是 `•` 块，其下方是进度行 `Question 1/1 (1 unanswered)`，再下方才是问题行与 `› 1.` 选项。
- gemini：引子是 `✦` 块，其下方是面板顶边框 `╭────╮`。
- kimi：引子是 `●` 块，其下方是面板顶分隔线，再下方是 `question` 标题、`? 题目` 与 `→ [1]` 选项。

### 检视发现的第二个缺陷：面板滚动时读到上一轮回复

grep 与单测复核时构造出「面板顶边框滚出可视区」的布局：一旦 `╭───`（gemini）或顶部分隔线（kimi）不可见，`reply_above_panel` 的起点就退化为第一个选项行。它向上遍历时会越过面板内部的缩进行，继续命中上一轮回复的 `✦` / `●` 标记行，于是把上一轮的内容当成本次引子推送出去。gemini 更严重：连题干都会被读成上一轮回复。

复现用例（修复前失败）：

~~~rust
// herdr/src/fetch_reply/screen/mod.rs
let screen = "✦ 上一轮的回答\n\n│ ▲\n│ ● 1.  A. O(log n)\n│   2.  B. O(n)\n│ ▼\n│ Enter to select · ↑/↓ to navigate · Esc to cancel\n╰────────────────────────────╯\n";
assert!(gemini::extract_choice(&useful_lines(screen)).is_none());
~~~

## 解决方案

### 公共部分

`herdr/src/fetch_reply/screen/mod.rs` 新增两个共享助手：

- `reply_above_panel`：从面板顶行向上越过空行、框线与分隔线、面板标题行，找到回复的标记行后收集整块；跨越途中遇到任何顶格的非标记文本即放弃，避免把更早一轮的话当成引子。
- `collect_marked_block`：按「标记行 + 缩进续行」收集一块回复，段内空行保留为段落分隔。

minimax 此前自带一份同构实现，改为复用共享助手，删掉重复代码。

### 各 agent

- codex：`extract_choice` 仅在面板上方存在 `Question N/N` 进度行时才取引子。审批门（`• Running …`）没有这行，因此不会把工具调用行当回复。引子末尾若已复述题目，面板内重复的题干不再输出。
- gemini：以面板顶边框 `╭` 为起点向上取 `✦` 块；并把「顶边框必须存在」作为前置条件，找不到就整块不读（见下）。
- kimi：新增 `extract_choice`，以面板顶分隔线为起点向上取 `●` 块，选项按 `→ [n]` 读取，`Other` 截断。
- gemini 与 kimi 的 `panel_top` 由 `usize` 改为 `Option<usize>`：找不到面板顶边即返回 `None`，`extract_choice` 随之放弃，回退到普通回复提取或通用选项读取。这样面板滚动时宁可少一段引子，也不会把上一轮内容当本轮推送。

## 验证

- `herdr/src/fetch_reply/screen/mod.rs` 新增四条真实屏幕用例：三个 agent 各一条「引子必须出现在正文最前」，另两条分别是 gemini 与 kimi 的「面板顶边不可见时不得读到上一轮」。
- `bash herdr/agent-tests/run-unit-tests.sh 'fetch_reply::screen'`：35 项通过。
- 改动只涉及 `herdr/src/fetch_reply/screen/` 五个文件，均为 daemon 侧逻辑，需重启 hicodeerd 生效。
