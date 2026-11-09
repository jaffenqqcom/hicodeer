# Agent 选择题推送到 IM 时丢失题面（qwen / zcode）

## 问题描述

HiCodeer 通过 hook 把 agent 的「等待确认 / 完成」状态推送到 IM 机器人，正文由 daemon 的 agent.reply 提供。qwen 与 zcode 弹出 AskUserQuestion 选择题时，TUI 上题面、选项、描述都完整，但推到 IM 的内容缺了最前面的标题与问题段：qwen 只剩选项列表或一句工具面板标题，zcode 只剩选项列表。问题只在「等待确认」这一路出现，完成态通知不受影响。

## 问题表现

- qwen：TUI 上是「连通测试 + 三行问题 + 四个选项」，IM 收到的是「AskUserQuestion Ask user 1 question ← + 四个选项」，题面整段丢失。
- qwen：另一种形态是 IM 收到「链路正常 / 说明 / Filter: type to search / 选项」，标题与问题都在第一行之前被截掉。
- zcode：TUI 上是「连通性测试 · 1/1 + 问题 + 三个选项」，IM 只收到选项与描述。
- 同一时刻用 agent.list 看，两个 agent 都是 blocked；用 pane.read 取屏幕，题面清晰可见。
- 旧 daemon 上还有一个旁证：agent.reply 的返回里没有 blocked_seq 字段，取到 Null。

## 问题原因

### 第一层：daemon 还跑着旧映像

zcode 与 qwen 的现象最初被当成提取规则的问题，实测发现 agent.reply 返回的正文是旧版格式：题目只有一行、选项不拆列、Other 未剔除，且 blocked_seq 为 Null。这说明设备上的 hicodeerd 仍是上一版二进制（daemon 是 HNP 常驻进程，不随 HAP 覆盖安装重启）。所以现象里有一部分不是代码缺陷，而是新代码没生效。

### 第二层：通用提取器按空行断句，qwen 的标题被挡在空行之外

通用选择题提取从第一个编号选项往上读，遇到空行就停止。qwen 的等待面板结构是：

~~~
? AskUserQuestion Ask user 1 question ←     ← 面板头
（空行）
连通测试                                     ← 标题
（空行）
这是一道纯粹用于测试工具连通性的选择题……        ← 问题（三行换行）
一个选项即可……
选项呢？
（空行）
❯ 1. 选项甲：蓝色
     第一个选项……
~~~

向上读只能碰到「选项呢？」到「这是一道……」这三行，遇到标题前的空行就停，标题必然丢失。

更早为 kimi 加的修法是「优先找以 ? 开头的行当问题」，在 qwen 上反而更糟：qwen 的面板头本身就以 ? 开头，于是「AskUserQuestion Ask user 1 question ←」被当成了题目。

### 第三层：取的是最早一题，不是当前题

一个 pane 回答完选择题后，旧题面板仍留在屏幕上。40 行快照里可能同时存在上一题和当前题。原来的查找用 position（第一个匹配）：

- qwen 找面板头用 position，命中上一题；
- 通用路径找编号 1 用 position，同样命中上一题。

结果是连续问两次选择题时，IM 发出的是上一题的问题与选项。

### 第四层：qwen 完成态没有普通回复分支

提取入口按 agent 名分派，qwen 没有分支，落到默认的 None。完成态通知在 hook 与会话文件都取不到文本时依赖屏幕兜底，qwen 这一路恒为空。

### 走过的死路

- 怀疑是快照只取 40 行导致题面被截掉。实测同时取 40 / 80 / 200 行，三份内容完全一致，排除截断。
- 怀疑 zcode 专用提取器没生效。实际上是 daemon 旧版，新提取器根本没被调用；用真实屏幕离线跑提取逻辑，一次就得到正确结果。
- 先做了「优先找 ? 开头的行」的修法，qwen 实测反而更差，才改成「拼接后剥离问号前缀」。

## 解决方案

### qwen 专用提取

新增 herdr/src/fetch_reply/screen/qwen.rs。选择题不再从选项往上猜，而是从最后一个面板头（以 ? 开头且含 AskUserQuestion）往下读到第一个编号 1，中间所有非空、非装饰、非状态栏的行都算题面：

~~~
let panel = lines.iter().rposition(|line| {
    let text = line.trim();
    text.starts_with('?') && text.contains("AskUserQuestion")
})?;
let first = lines.iter().enumerate().skip(panel + 1).find_map(...)?;
let question_lines: Vec<String> = lines[panel + 1..first]
    .iter()
    .filter(|line| !line.trim().is_empty())
    .filter(|line| !is_chrome(line) && !is_decoration(line))
    .map(|line| line.trim().to_string())
    .collect();
~~~

这样标题、空行之后的问题、多行换行都按顺序保留，选项与描述复用公共解析。

普通回复按 qwen 自己的标记取：最后一个以 ◆ 开头的行，续行必须缩进；遇到工具结果标记 ⎿ 或面板竖线就停。

~~~
◆ 成功
 ⎿ Stop says:            ← 停在这里
    Warning: ...
~~~

### 公共解析抽出来

把 strip_frame、numbered、collect_numbered_options 从 choice_block 内部提到模块级，qwen 与通用路径共用同一套编号解析（支持 1. / 1) / [1] / 1、，以及 ❯ 与 → 前缀、│┃▎ 边框）。

### 取最新一题

- qwen 的面板头、通用路径的编号 1，都从 position 改为 rposition。
- zcode 原本就用 rposition，保持一致。

### zcode 去重复

zcode 的选择题在第 31 行已被专用函数读过一次，第 49 行的 zcode::extract 内部又读一次。改为 extract 只做普通回复（last_plain_block），判断题只留一处。

### 用真实屏幕做回归

单测此前用的是手写合成屏幕，结构过于理想，真实界面一上就漏。这次把现场抓到的屏幕原样固化：

- qwen 选择题（面板头 + 标题 + 三行问题 + 四个选项 + Type something）
- qwen 普通回复（◆ 成功 + ⎿ 工具结果 + 输入框 + 状态栏）
- zcode 选择题（过滤行 + 预览区 + 三选项 + Other）
- 两题同屏，断言取到的是后一题

16 项提取测试全过。

### 顺带修掉

zcode 测试里的 assert!(reply.contains("1. ") == false) 会触发 clippy::bool_comparison，在 script/clippy 的 --deny warnings 下直接失败，改为 assert!(!...)。

## 修改文件

- herdr/src/fetch_reply/screen/qwen.rs — 新增：选择题从面板头往下读，保留标题与多行问题；普通回复按 ◆ 标记提取，遇 ⎿ 停止
- herdr/src/fetch_reply/screen/mod.rs — 抽出 strip_frame / numbered / collect_numbered_options 供各 agent 共用；分派顺序改为专用选择题 → 通用选择题 → 普通回复；补 qwen 普通回复分支；通用选择题改取最后一次；新增 qwen 与多题真实样本测试
- herdr/src/fetch_reply/screen/zcode.rs — extract 去掉重复的选择题扫描，只做 last_plain_block；新增真实屏幕回归测试；修正 clippy 会拦的布尔断言
