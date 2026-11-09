#!/usr/bin/env python3
"""端到端校验：agent 完成一轮后，daemon 还原的最后答复是否随 agent.list 带出。

覆盖本轮新增的链路：

  会话存储 → fetch_reply adapter（daemon 侧）→ AgentInfo.last_reply
  → agent.list → 面板 AgentRef.last_reply → IM 通知正文

断言分三层：

  1. 契约：last_reply 出现时 completion_seq 必须存在；被中断的轮次两者都不出现。
  2. 内容：last_reply 是该轮 agent 的答复本身，不是终端快照——含期望文本，
     且不含提示符 / 「esc to interrupt」这类屏幕装饰。
  3. 稳定：同一 completion_seq 连续两次 agent.list 返回同一文本（daemon 的
     memo 只解析一次）；再跑一轮后 seq 变大、文本跟着换（--second-round）。

两种跑法：

  --list-only  不新建 space、不发提示，只对当前 pane 做契约校验，适合快速回测。
  默认         新建独立 space、真起一个 agent、发一轮提示并等它完成。
"""

import argparse
import os
import time

import agent_prereqs
import agent_rpc
import test_hooks
from agent_rpc import run_guarded

# 提示词只要求两个字，答复容易做包含断言，也不会因为模型措辞不同而误判。
DEFAULT_PROMPT = "只回复两个字：成功"
DEFAULT_EXPECT = "成功"
SECOND_PROMPT = "只回复两个字：收到"
SECOND_EXPECT = "收到"

# Ask for a real choice so the agent parks the turn on the user, which is the
# state the IM notification has to describe.
CHOICE_PROMPT = (
    "用选择题问我：下面哪种方案更好。必须调用 AskUserQuestion 工具，"
    "两个选项的 label 分别写成 Alpha 和 Beta，不要自己选，等我选完再继续。"
)
# A permission prompt can stand between the agent and its question. Approving it
# is part of reaching the wait, so the test does it instead of stopping there.
WAITING_APPROVALS = 1
# A choice prompt can be swallowed before the TUI is ready; retry a couple of
# times while the pane stays idle, and leave a gap so the first submit lands.
CHOICE_CONFIRM_SECONDS = 2.5
CHOICE_SUBMIT_ATTEMPTS = 2
# The choice is drawn a moment after the pane turns blocked; poll the reply
# for this long before calling it missing.
CHOICE_REPLY_WINDOW_SECONDS = 5.0
# The labels the question must carry. Checking them is the point: a bare
# "needs your permission" line is not the question the reader has to answer.
CHOICE_LABELS = ("Alpha", "Beta")
# A choice is a model round plus a tool call, so it gets more room than a
# plain reply.
WAITING_TIMEOUT_SECONDS = 10.0

# 表格与符号必须原样穿过 daemon 的解析与 JSON 传输。提示词明确要求不改写，
# 断言挑的是稳定标记：markdown 表格的三行 + 各类符号。
SPECIAL_PROMPT = (
    "原样输出下面四行，不要解释、不要改写、不要加代码块：\n"
    "| 项目 | 结果 |\n"
    "| --- | --- |\n"
    "| 编译 | ✅ 通过 |\n"
    "符号：→ ≤ ≥ ★ ☑ 「引号」① ½ ⚠"
)
SPECIAL_MARKS = (
    "| 项目 | 结果 |",
    "| --- | --- |",
    "| 编译 | ✅ 通过 |",
    "→",
    "≤",
    "≥",
    "★",
    "☑",
    "「引号」",
    "①",
    "½",
    "⚠",
)
# U+FFFD 出现即说明某处按错误的字符集截断或替换过。
REPLACEMENT_CHARACTER = "\ufffd"

DEFAULT_AGENT = "codebuddy"

# Every agent the reply pipeline has a working adapter and hook path for. Kept
# as a list so --agents all can cover the whole set in one run.
DEFAULT_AGENTS = [
    "claude",
    "codebuddy",
    "codex",
    "codewhale",
    "deveco",
    "gemini",
    "hermes",
    "kimi",
    "minimax",
    "opencode",
    "pi",
    "qwen",
    "zcode",
]

# Credentials an agent needs before its first message. A pane inherits the
# shell's startup files, not this process's environment, so these are injected
# through workspace.create(env=...). Values are never printed.
# Local services (gemini bridge, codex relay) and per-agent environment live in
# agent_prereqs so both e2e scripts share one recipe and one cleanup path.

# TUI 首帧未就绪时会把第一个回车吞掉，所以输入后先等窗口摆好。
TUI_READY_DELAY_SECONDS = 1.0
# 按下回车后状态在这个窗口内没离开起始值就认为没提交，再按一次。
PROMPT_CONFIRM_SECONDS = 6.0
PROMPT_SUBMIT_ATTEMPTS = 3
# 新 pane 的 shell 就绪前 agent.start 会回 agent_pane_busy，最多等这么久。
START_AGENT_WAIT_SECONDS = 10.0

# 识别出 agent 后、发第一轮提示前的稳定期。opencode / hermes 的首帧画完之前
# 会把送进去的文字丢掉，表现为「状态一直 idle、pane 里却能看到提示」。
DEFAULT_SETTLE_SECONDS = 5.0

# 终端快照里常见、但 agent 答复里不该出现的装饰。命中说明取错了来源。
TERMINAL_NOISE = ("esc to interrupt", "ctrl+c to", "? for shortcuts")


def agent_entry(rpc, pane_id):
    """agent.list 里该 pane 的条目（pane.get 不含 completion_seq / last_reply）。"""
    for entry in rpc.agents():
        if entry.get("pane_id") == pane_id:
            return entry
    return {}


def pane_state(rpc, pane_id):
    result = rpc.call("pane.get", {"pane_id": pane_id})
    return result.get("pane") or {}


def wait_until(rpc, pane_id, predicate, timeout, interval=0.5):
    deadline = time.time() + timeout
    last = {}
    while time.time() < deadline:
        last = pane_state(rpc, pane_id)
        if predicate(last):
            return last
        time.sleep(interval)
    return last


def wait_for_reply(rpc, pane_id, timeout, previous_seq=None, interval=0.5):
    """轮询 agent.list 直到该 pane 带出非空 last_reply。

    daemon 只在被拉取时解析会话存储，首次读可能早于 transcript 落盘，所以
    这里按 completion_seq 轮询，而不是拿到 idle 就立刻断言。
    """
    deadline = time.time() + timeout
    last = {}
    while time.time() < deadline:
        last = agent_entry(rpc, pane_id)
        seq = last.get("completion_seq")
        fresh = previous_seq is None or (
            isinstance(seq, int) and seq > previous_seq
        )
        if fresh and seq is not None and last.get("last_reply"):
            return last
        time.sleep(interval)
    return last


def dismiss_selection_menu(rpc, pane_id, timeout, interval=0.5):
    """Closes a startup picker the agent shows before accepting input.

    qwen opens a selection menu on first start; while it is up, typed text goes
    nowhere and the pane stays idle. Escape leaves the menu without choosing.
    """
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            text = rpc.call(
                "pane.read", {"pane_id": pane_id, "source": "visible", "lines": 20}
            )["read"]["text"]
        except agent_rpc.RpcError:
            return
        if "Enter to select" not in text:
            return
        rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["escape"]})
        time.sleep(interval)


def start_agent_when_shell_ready(rpc, agent, pane_id, timeout_ms):
    deadline = time.time() + START_AGENT_WAIT_SECONDS
    while True:
        try:
            return rpc.start_agent(agent, pane_id, timeout_ms=timeout_ms)
        except agent_rpc.RpcError as error:
            if error.code != "agent_pane_busy" or time.time() >= deadline:
                raise
            time.sleep(1.0)


def run_prompt_round(rpc, pane_id, prompt, timeout):
    """发一轮提示并跟到 idle，返回过程中看到的最终 pane 状态。"""
    baseline = pane_state(rpc, pane_id).get("agent_status")
    rpc.call("pane.send_text", {"pane_id": pane_id, "text": prompt})
    time.sleep(TUI_READY_DELAY_SECONDS)

    deadline = time.time() + timeout
    next_submit = time.time()
    submits = 0
    seen_working = False
    idle_after_working = False
    pane = {}
    status = baseline

    while time.time() < deadline:
        pane = pane_state(rpc, pane_id)
        status = pane.get("agent_status")
        if status == "working":
            seen_working = True
        elif seen_working and status in ("idle", "done"):
            idle_after_working = True
            break
        if (
            not seen_working
            and submits < PROMPT_SUBMIT_ATTEMPTS
            and time.time() >= next_submit
            and status == baseline
        ):
            rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["enter"]})
            submits += 1
            next_submit = time.time() + PROMPT_CONFIRM_SECONDS
        time.sleep(0.5)

    return {
        "pane": pane,
        "seen_working": seen_working,
        "idle_after_working": idle_after_working,
        "last_status": status,
    }


def check_contract(harness, entry, label):
    """last_reply 与 completion_seq 的关系，以及它是否干净。"""
    pane_id = entry.get("pane_id") or "?"
    seq = entry.get("completion_seq")
    reply = entry.get("last_reply")
    status = entry.get("agent_status")
    if reply is not None:
        # A working agent has not finished the turn: its tool call is not a
        # reply, and quoting it in a notification would be wrong.
        if status == "working":
            harness.fail(
                "%s %s: working 态不应有 last_reply" % (label, pane_id),
                "null/空",
                repr(reply),
            )
        # A waiting agent has no completion marker: the turn is not finished,
        # and its text is the question it is asking. Only a settled turn is
        # required to carry the marker that identifies it.
        if status in ("idle", "done") and seq is None:
            # The text can come straight from the stop hook, which does not
            # carry a completion marker. That is a valid source, not a fault.
            harness.warn(
                "%s %s: 完成态的 last_reply 没有 completion_seq（hook 直供文本）"
                % (label, pane_id)
            )
        harness.expect(
            isinstance(reply, str) and reply.strip() != "",
            "%s %s: last_reply 非空" % (label, pane_id),
            "非空字符串",
            repr(reply),
        )
        lowered = reply.lower() if isinstance(reply, str) else ""
        for noise in TERMINAL_NOISE:
            harness.expect(
                noise not in lowered,
                "%s %s: last_reply 不含终端装饰 %r" % (label, pane_id, noise),
                "不含",
                "包含",
            )
    elif seq is not None:
        # 已完成却取不到答复：可能是 adapter 未覆盖该 agent，或会话尚未写完。
        # 只告警，避免把环境差异当成功能回归。
        harness.warn(
            "%s %s: completion_seq=%s 但没有 last_reply（adapter 未覆盖或会话不完整）"
            % (label, pane_id, seq)
        )


def check_existing(harness, rpc, args):
    agents = rpc.agents()
    harness.ok("agent.list 返回 %d 个 agent" % len(agents))
    with_reply = 0
    with_completion = 0
    for entry in agents:
        check_contract(harness, entry, "existing")
        if entry.get("completion_seq") is not None:
            with_completion += 1
        if entry.get("last_reply"):
            with_reply += 1
    harness.info(
        "现有 pane：%d 个有条目，%d 个带 completion_seq，%d 个带 last_reply"
        % (len(agents), with_completion, with_reply)
    )
    return True


def check_special(harness, rpc, pane_id, previous_seq, args):
    """表格与符号原样穿过 daemon 的解析和 JSON 传输。"""
    harness.info("")
    harness.info("== 特殊字符 / 表格")

    result = run_prompt_round(rpc, pane_id, SPECIAL_PROMPT, args.wait_seconds)
    harness.expect(
        bool(result["seen_working"]),
        "特殊字符轮上报过 working",
        "至少一次 agent_status=working",
        repr(result["last_status"]),
    )
    entry = wait_for_reply(
        rpc, pane_id, args.reply_seconds, previous_seq=previous_seq
    )
    check_contract(harness, entry, "special")
    reply = entry.get("last_reply")
    if not isinstance(reply, str):
        harness.fail("特殊字符轮 last_reply 存在", "字符串", repr(reply))
        return
    for mark in SPECIAL_MARKS:
        harness.expect(
            mark in reply,
            "特殊字符原样保留 %r" % mark,
            "包含",
            "缺失",
        )
    harness.expect(
        REPLACEMENT_CHARACTER not in reply,
        "没有编码损坏字符 U+FFFD",
        "不含",
        "包含",
    )
    harness.expect(
        reply.count("|") >= 6,
        "表格竖线至少 6 个",
        ">= 6",
        repr(reply.count("|")),
    )
    if args.show_reply:
        harness.info("特殊字符轮 last_reply 原文：")
        harness.info(reply)


def check_waiting(harness, rpc, pane_id, args, deadline=None):
    """等待选择时，agent.reply 仍能给出它问的问题与选项。

    这一态没有任何 assistant 消息写进会话文件：agent 还没回答，只是在等。
    所以取到的内容只能来自 hook 上报（AskUserQuestion 的 tool_input）。

    deadline 是本次 agent 的总预算截止时刻；没有剩余时间就直接判失败，
    不再用一个默认超时把整轮拖长。
    """
    def left(default):
        if deadline is None:
            return default
        return max(0.0, min(default, deadline - time.time()))

    harness.info("")
    harness.info("== 等待选择（notify）")
    if deadline is not None and deadline - time.time() < 5.0:
        harness.fail(
            "等待选择（notify）",
            "剩余预算 ≥ 5 秒",
            "%.1f 秒" % (deadline - time.time()),
        )
        return
    # A choice never returns the pane to idle, so waiting for a finished round
    # here would burn the whole round budget before the choice is even read.
    # Send, then wait for blocked directly.
    rpc.call("pane.send_text", {"pane_id": pane_id, "text": CHOICE_PROMPT})
    time.sleep(TUI_READY_DELAY_SECONDS)
    rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["enter"]})
    # The first return can be swallowed while the TUI is still laying out, so
    # the prompt is submitted again while the pane is still idle. Each retry
    # waits before pressing again; pressing while working would interrupt.
    blocked_deadline = time.time() + left(WAITING_TIMEOUT_SECONDS)
    submits = 0
    next_submit = time.time() + CHOICE_CONFIRM_SECONDS
    pane = {}
    while time.time() < blocked_deadline:
        pane = pane_state(rpc, pane_id)
        status = pane.get("agent_status")
        if status == "blocked":
            break
        if (
            submits < CHOICE_SUBMIT_ATTEMPTS
            and time.time() >= next_submit
            and status in ("idle", "done")
        ):
            rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["enter"]})
            submits += 1
            next_submit = time.time() + CHOICE_CONFIRM_SECONDS
        time.sleep(0.5)
    if not harness.equal(
        "等待选择时状态变 blocked", "blocked", pane.get("agent_status")
    ):
        return
    # The pane turns blocked before the choice is drawn, so the reply is polled
    # for a short while: on several agents the text appears a moment later.
    reply = None
    approvals = 0
    reply_deadline = time.time() + left(CHOICE_REPLY_WINDOW_SECONDS)
    while time.time() < reply_deadline:
        try:
            result = rpc.call("agent.reply", {"target": pane_id})
        except agent_rpc.EnvironmentNotMet as error:
            # An unavailable reply interface is exactly what this check exists
            # to catch; it is a failure, never a skip.
            harness.fail("等待选择时 agent.reply 可用", "接口可用", str(error))
            return
        reply = result.get("reply")
        if isinstance(reply, str) and all(label in reply for label in CHOICE_LABELS):
            break
        if approvals < WAITING_APPROVALS and "permission" in (reply or "").lower():
            # A permission prompt stands before the choice; approve it.
            rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["enter"]})
            approvals += 1
        time.sleep(0.5)
    harness.expect(
        isinstance(reply, str) and reply.strip() != "",
        "等待选择时 agent.reply 非空",
        "非空字符串",
        repr(reply),
    )
    for label in CHOICE_LABELS:
        harness.expect(
            isinstance(reply, str) and label in reply,
            "等待选择的正文含选项 %r" % label,
            "包含",
            repr(reply),
        )
    if isinstance(reply, str):
        lowered = reply.lower()
        for noise in TERMINAL_NOISE:
            harness.expect(
                noise not in lowered,
                "等待选择的正文不含终端装饰 %r" % noise,
                "不含",
                "包含",
            )
        if args.show_reply:
            harness.info("等待选择的正文：")
            harness.info(reply)
    # Leave the wait so closing the workspace does not strand a blocked agent.
    rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["escape"]})


def check_agent(harness, rpc, args, agent):
    harness.info("")
    harness.info("== %s" % agent)

    env, reason = agent_prereqs.prepare(agent)
    if reason:
        # Not runnable is a failure of this check, not a skip.
        harness.fail("%s 环境准备" % agent, "可运行", reason)
        return

    created = rpc.create_workspace(
        cwd=args.cwd,
        focus=True,
        label="last-reply-%s" % agent,
        env=env,
    )
    workspace = created.get("workspace") or {}
    root_pane = created.get("root_pane") or {}
    pane_id = root_pane.get("pane_id")
    if not pane_id:
        harness.fail("%s 拿到 root pane" % agent, "pane_id", repr(root_pane))
        return

    try:
        started = start_agent_when_shell_ready(rpc, agent, pane_id, args.start_timeout_ms)
        harness.non_empty(
            "%s agent.start 返回 argv" % agent, " ".join(started.get("argv") or [])
        )
        # deveco asks for the working directory on every TUI start and offers a
        # self-update; either prompt blocks the prompt box. The dismissal logic
        # is shared with test_hooks so the prompt table is maintained once.
        test_hooks.dismiss_startup_prompt(rpc, pane_id, agent, args.startup_prompt_seconds)
        pane = wait_until(
            rpc, pane_id, lambda p: p.get("agent") == agent, args.wait_seconds
        )
        if not harness.equal("%s pane 识别出 agent" % agent, agent, pane.get("agent")):
            return
        wait_until(rpc, pane_id, lambda p: p.get("agent_status") != "unknown", 15)
        time.sleep(args.settle_seconds)
        test_hooks.dismiss_startup_prompt(rpc, pane_id, agent, args.startup_prompt_seconds)
        dismiss_selection_menu(rpc, pane_id, args.startup_prompt_seconds)

        first = run_prompt_round(rpc, pane_id, args.prompt, args.wait_seconds)
        harness.expect(
            bool(first["seen_working"]),
            "%s 上报过 working" % agent,
            "至少一次 agent_status=working",
            repr(first["last_status"]),
        )
        harness.expect(
            bool(first["idle_after_working"]),
            "%s 收尾回到 idle/done" % agent,
            "working 之后出现 idle/done",
            repr(first["last_status"]),
        )

        entry = wait_for_reply(rpc, pane_id, args.reply_seconds)
        check_contract(harness, entry, "round1")
        reply = entry.get("last_reply")
        harness.expect(
            isinstance(reply, str) and args.expect in reply,
            "%s 第一轮 last_reply 含期望文本" % agent,
            "包含 %r" % args.expect,
            repr(reply),
        )
        first_seq = entry.get("completion_seq")

        # 同一 completion_seq 再拉一次：daemon 的 memo 应命中，文本不得改变。
        again = agent_entry(rpc, pane_id)
        harness.equal(
            "%s 同一 completion_seq 的 last_reply 稳定" % agent,
            reply,
            again.get("last_reply"),
        )

        if args.second_round:
            second = run_prompt_round(rpc, pane_id, args.second_prompt, args.wait_seconds)
            harness.expect(
                bool(second["seen_working"]),
                "%s 第二轮上报过 working" % agent,
                "至少一次 agent_status=working",
                repr(second["last_status"]),
            )
            entry = wait_for_reply(
                rpc, pane_id, args.reply_seconds, previous_seq=first_seq
            )
            check_contract(harness, entry, "round2")
            harness.expect(
                isinstance(first_seq, int)
                and isinstance(entry.get("completion_seq"), int)
                and entry["completion_seq"] > first_seq,
                "%s 第二轮 completion_seq 变大" % agent,
                "大于 %r" % (first_seq,),
                repr(entry.get("completion_seq")),
            )
            reply = entry.get("last_reply")
            harness.expect(
                isinstance(reply, str) and args.second_expect in reply,
                "%s 第二轮 last_reply 含期望文本" % agent,
                "包含 %r" % args.second_expect,
                repr(reply),
            )

        if args.special_chars:
            check_special(harness, rpc, pane_id, entry.get("completion_seq"), args)

        if args.waiting:
            check_waiting(harness, rpc, pane_id, args)
    finally:
        if not args.keep:
            rpc.close_workspace(workspace.get("workspace_id") or "", close_group=True)


def body(harness, args):
    rpc = agent_rpc.AgentRpc(socket_path=args.socket, timeout=args.timeout)
    rpc.ping()

    if args.list_only:
        check_existing(harness, rpc, args)
        return harness.finish()

    agents = [name.strip() for name in args.agents.split(",") if name.strip()]
    if agents == ["all"]:
        agents = list(DEFAULT_AGENTS)
    if not agents:
        agents = [DEFAULT_AGENT]
    for agent in agents:
        check_agent(harness, rpc, args, agent)
    return harness.finish()


def add_arguments(parser):
    parser.add_argument(
        "--agents",
        default=DEFAULT_AGENT,
        help="逗号分隔的 agent 名单，默认 %s；all 表示覆盖整个清单" % DEFAULT_AGENT,
    )
    parser.add_argument(
        "--list-only",
        action="store_true",
        help="不新建 space、不发提示，只校验现有 pane 的契约",
    )
    parser.add_argument("--prompt", default=DEFAULT_PROMPT, help="第一轮提示")
    parser.add_argument("--expect", default=DEFAULT_EXPECT, help="第一轮答复应包含的文本")
    parser.add_argument(
        "--second-round",
        action="store_true",
        help="再跑一轮，校验 completion_seq 变大且 last_reply 换成新答复",
    )
    parser.add_argument("--second-prompt", default=SECOND_PROMPT, help="第二轮提示")
    parser.add_argument(
        "--second-expect", default=SECOND_EXPECT, help="第二轮答复应包含的文本"
    )
    parser.add_argument(
        "--special-chars",
        dest="special_chars",
        action="store_true",
        default=True,
        help="额外跑一轮表格 + 特殊符号，验证原文保真（默认开启）",
    )
    parser.add_argument(
        "--no-special-chars",
        dest="special_chars",
        action="store_false",
        help="跳过表格 + 特殊符号轮",
    )
    parser.add_argument(
        "--show-reply",
        action="store_true",
        help="打印特殊字符轮的 last_reply 原文，便于核对",
    )
    parser.add_argument(
        "--cwd", default=None, help="workspace 的工作目录，默认沿用 daemon 当前目录"
    )
    parser.add_argument(
        "--wait-seconds",
        type=float,
        default=60.0,
        help="等待 agent 识别与一轮完成的秒数，默认 60",
    )
    parser.add_argument(
        "--reply-seconds",
        type=float,
        default=15.0,
        help="完成后等待 last_reply 出现的秒数，默认 15",
    )
    parser.add_argument(
        "--start-timeout-ms",
        type=int,
        default=60000,
        help="agent.start 的启动超时，默认 60000",
    )
    parser.add_argument(
        "--startup-prompt-seconds",
        type=float,
        default=15.0,
        help="等待 agent 启动提示（如 deveco 的目录信任）的上限，默认 15 秒",
    )
    parser.add_argument(
        "--settle-seconds",
        type=float,
        default=DEFAULT_SETTLE_SECONDS,
        help="识别出 agent 后、发提示前的稳定期，默认 %(default)s 秒",
    )
    parser.add_argument(
        "--waiting",
        action="store_true",
        help="额外校验等待选择（notify）时 agent.reply 是否给出问题与选项",
    )
    parser.add_argument(
        "--keep", action="store_true", help="结束后不关闭本轮新建的 workspace"
    )


if __name__ == "__main__":
    raise SystemExit(run_guarded("最后答复透传校验", body, add_arguments))
