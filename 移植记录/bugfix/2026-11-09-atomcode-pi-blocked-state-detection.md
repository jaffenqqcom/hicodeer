# atomcode 选择题不上报 blocked、pi 完成后不回 idle

## 问题描述

HiCodeer 通过 hook 与屏幕识别判断 pane 里 agent 的状态。同一批实测里发现两个 agent 的等待/结束状态不对：atomcode 弹出 AskUserQuestion 后一直显示「运行中」，pi 在任务已经写完答复之后仍停在 working。两者都直接影响 Agent panel 的阻塞提示与 IM 通知，因为面板拿不到 blocked 就不会提醒用户去选。

## 问题表现

- atomcode：发「用选择题问我…」，屏幕很快画出选项框，状态序列只有 idle → working，之后 30 秒不再变化；用 agent.list 看始终是 working。
- pi：发「只回复两个字：完成」，屏幕已经出现「完成」，状态仍是 working；等待 30 秒也不回落。
- 两者共同的旁证：面板不亮阻塞灯，IM 收不到「等待确认」，必须用户自己盯着 pane。

## 问题原因

### atomcode：hook 源头没有等待类事件，只能靠屏幕

拉取 atomcode 源码（atomgit 上的 atomgit_atomcode/atomcode）确认，crates/atomcode-capabilities/src/cc_hooks.rs 的 HookEvent 只有 8 个：

~~~
PreToolUse / PostToolUse / PostToolUseFailure / SessionStart /
SessionEnd / UserPromptSubmit / Stop / StopFailure
~~~

没有 Notification，也没有 PermissionRequest。AskUserQuestion 对 atomcode 来说只是一次普通工具调用，PreToolUse 报 working，之后直到轮到 Stop 之前都没有新信号，所以状态必然停在 working。

它的 hooks matcher 只支持精确名或「前缀 *」，不是正则，也没有排除语法；给 PreToolUse 再加一条 matcher=AskUserQuestion 的 blocked 规则，会和通用的 PreToolUse→working 同时命中，谁后执行不确定，属于引入随机状态，不可取。

结论：atomcode 只能用屏幕识别。屏幕上有两个稳定特征：

~~~
❯ 1. Alpha
  2. Beta
  3. Type something.
  ─────────────────────────
  4. Chat about this
~~~

~~~text
⠹ running 1 tools （2s elapsed · ↑ 26.8K · ↓ 144 · 48 tok/s · cached 98.72%）
~~~

### atomcode：规则写了却没被加载

第一版只新增了 herdr/src/detect/manifests/atomcode.toml 并注册进 BUNDLED_MANIFESTS，实测仍不生效。用 agent.explain 看引擎判定：

~~~text
state: idle
manifest_source: null
evaluated_rules: []
fallback_reason: default_known_agent_idle_fallback
~~~

原因是 Agent::SCREEN_MANIFEST_AGENTS 这个白名单常量里没有 AtomCode。规则缓存只遍历白名单里的 agent，没进白名单就根本不会加载 manifest，写好的规则等于不存在。把 AtomCode 加进去（数组长度 24 → 25）之后，explain 立刻变成：

~~~text
state: blocked
manifest_source: bundled
matched_rule: drawn_choice
visible_blocker: true
~~~

### pi：集成监听了一个不存在的事件

pi 有完整 JS 源码（npm 包 dist/ 下有 .js/.d.ts/.js.map）。扩展 API 的事件联合类型只包含 agent_start、agent_end、turn_start、turn_end、session_* 等，没有 agent_settled。我们的 assets/pi/hicodeer-agent-state.ts 却写着 pi.on("agent_settled", …)，这个回调永远不会触发，所以 agent_start 之后没有任何事件把 working 改回 idle。

### pi：改对事件名之后仍不回 idle

把事件名换成 agent_end 后复测，状态还是 working。给扩展临时加日志（写到临时文件，验证后删除）拿到：

~~~text
publish idle
publish working
agent_end isIdle=false root=true
~~~

agent_end 确实触发了，但回调执行时 ctx.isIdle() 仍返回 false，被原来的判断 ctx?.isIdle?.() !== true 直接 return。也就是说在 pi 的事件时序里，agent_end 发出时上下文还没切到空闲。

### 走过的死路

- 一开始怀疑 atomcode 的事件表注册不全，反复核对 7 条 hooks，方向错了；真正的缺口是屏幕规则没进白名单。
- 给 kimi/codewhale 的 hook 脚本加事件日志时，把打印语句插在 case 语句的 esac 后面，脚本出现 unexpected 'esac'，codewhale 直接提示 message_submit hook blocked submission，消息都发不出去。后来改成追加到脚本末尾（不碰结构）才拿到事件序列。诊断打点改运行时脚本，必须避开 case/heredoc 这些结构。
- pi 一开始想走屏幕兜底，但 pi 属于 full-lifecycle hook authority，screen_detection_skipped=true，屏幕结果根本不参与；而且 pi.toml 本来就有 Working... 规则。真正该修的是事件名，不是加规则。

## 解决方案

### atomcode：新增屏幕规则

新增 herdr/src/detect/manifests/atomcode.toml，两条规则：

- drawn_choice（blocked，优先级 950）：底部 8 个非空行内同时出现「编号 + Type something.」与「Chat about this」。编号正则兼容 1. / 1) / [1]；再加 not 条件排除出现「✻ Done」的屏幕，避免答完题后残留的选择框继续把状态压成 blocked。
- status_running（working，优先级 800）：行首盲文 spinner + running N tools，覆盖 hook 丢失时的工作态。

~~~toml
[[rules]]
id = "drawn_choice"
state = "blocked"
region = "bottom_non_empty_lines(8)"
visible_blocker = true
all = [
  { regex = ['(?m)^\s*(?:\d+[.)]|\[\d+\])\s*Type something\.?\s*$'] },
  { contains = ["Chat about this"] },
]
not = [{ contains = ["✻ Done"] }]
~~~

在 manifest.rs 的 BUNDLED_MANIFESTS 注册，并把 AtomCode 加入 SCREEN_MANIFEST_AGENTS。atomcode 不在 full-lifecycle 白名单，屏幕的 visible_blocker 可以压过 hook 的 working（visible_blocker_overrides_hook），所以弹题后状态会从 working 翻成 blocked。

### pi：改成真正存在的事件

~~~ts
// 之前
pi.on("agent_settled", (_event, ctx) => {
  if (!rootSession || ctx?.isIdle?.() !== true) return;
  agentActive = false;
  publishState();
});

// 之后
pi.on("agent_end", (_event, _ctx) => {
  if (!rootSession) return;
  agentActive = false;
  publishState();
});
~~~

agent_end 就是整轮 agent loop 结束；实测一次包含 4 次工具调用的任务只会触发一次，不会在工具循环中间闪 idle。若 pi 之后自动续跑，会再发 agent_start，状态自然回到 working。集成版本 11 → 13。

## 修改文件

- herdr/src/detect/manifests/atomcode.toml — 新增：选择题 blocked 规则与运行态 working 规则，底部区域限定 + Done 排除
- herdr/src/detect/manifest.rs — 把 atomcode 注册进 BUNDLED_MANIFESTS
- herdr/src/detect/mod.rs — SCREEN_MANIFEST_AGENTS 加入 AtomCode（24 → 25），否则规则不加载
- herdr/src/detect/manifest/tests.rs — 新增解析测试与两条真实屏幕检测测试（弹题判 blocked、带 Done 的答完屏不再 blocked、running 行判 working）
- herdr/src/integration/assets/pi/hicodeer-agent-state.ts — agent_settled 改为 agent_end，去掉回调里恒为 false 的 isIdle 判断，版本头 11 → 13
- herdr/src/integration/mod.rs — PI_INTEGRATION_VERSION 11 → 13
