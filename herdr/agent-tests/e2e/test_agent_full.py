#!/usr/bin/env python3
"""一个 agent、一次启动、三项验收：状态通知、最后答复、历史会话。

把 test_hooks / test_last_reply / test_scan 的三段断言合到一条流程里，避免
同一个 agent 被启动三次。指定一个 agent 就能同时得到三项结果。

每个 agent 跑两轮：先要求它回答"成功"，再要求它弹一道 Alpha/Beta 选择题。
两轮的正文都必须出现约定的关键词，否则判失败。

用法：
    python3 test_agent_full.py                 # 一次跑完清单里的所有 agent
    python3 test_agent_full.py --agents claude # 只跑一个
"""

import argparse
import time

import agent_prereqs
import agent_rpc
import test_hooks
import test_last_reply
from agent_rpc import run_guarded

DEFAULT_PROMPT = "只回复两个字：成功"
DEFAULT_EXPECT = "成功"

# One agent, two rounds, one minute. The budget is enforced, not advisory: a
# check that would run past it fails and the next agent starts.
AGENT_BUDGET_SECONDS = 90.0

# The integration registry names two agents differently from the panel's own
# agent label. Reading the registry is still the right check; only the key
# differs.
INTEGRATION_TARGETS = {"codebuddy": "code_buddy"}


def integration_target(agent):
    return INTEGRATION_TARGETS.get(agent, agent)


def selected_agents(spec):
    if spec.strip() == "all":
        return list(test_last_reply.DEFAULT_AGENTS)
    return [agent for agent in spec.split(",") if agent.strip()]


def check_sessions(harness, rpc, agent):
    """历史会话：扫描结果里该 agent 至少一条，且字段完整。"""
    result = rpc.call("agent.sessions", {})
    sessions = result.get("sessions") or []
    mine = [session for session in sessions if session.get("agent") == agent]
    harness.expect(
        bool(mine),
        "%s 有历史会话" % agent,
        "至少 1 条",
        str(len(mine)),
    )
    if not mine:
        return
    broken = [
        session
        for session in mine
        if not (
            session.get("session_id")
            and session.get("source_path")
            and isinstance(session.get("deletable"), bool)
        )
    ]
    harness.expect(
        not broken,
        "%s 历史会话字段完整" % agent,
        "session_id/source_path/deletable 齐全",
        repr(broken[:1]),
    )


def check_agent(harness, rpc, args, agent):
    harness.info("")
    harness.info("== %s（通知 / 最后答复 / 历史会话）" % agent)
    deadline = time.time() + AGENT_BUDGET_SECONDS
    budget_reported = False

    def left(default):
        """Time this step may take: its own limit, capped by what is left."""
        nonlocal budget_reported
        remaining = deadline - time.time()
        if remaining > 1.0:
            return min(default, remaining)
        if not budget_reported:
            budget_reported = True
            harness.fail(
                "%s 在 %d 秒内完成" % (agent, int(AGENT_BUDGET_SECONDS)),
                "≤ %d 秒" % int(AGENT_BUDGET_SECONDS),
                "超出预算",
            )
        return None

    env, reason = agent_prereqs.prepare(agent)
    if reason:
        # Not runnable is a failure of this check, not a skip: an agent that
        # cannot be started cannot demonstrate any of the three features.
        harness.fail("%s 环境准备" % agent, "可运行", reason)
        return

    # 通知：集成必须装好，状态才由我们的 hook 上报。安装也受总预算约束。
    install_timeout = left(15.0)
    if install_timeout is None:
        return
    if not test_hooks.ensure_installed(
        harness, rpc, integration_target(agent), True, install_timeout
    ):
        return

    created = rpc.create_workspace(
        cwd=args.cwd,
        focus=True,
        label="agent-full-%s" % agent,
        env=env,
    )
    workspace = created.get("workspace") or {}
    root_pane = created.get("root_pane") or {}
    pane_id = root_pane.get("pane_id")
    if not pane_id:
        harness.fail("%s 拿到 root pane" % agent, "pane_id", repr(root_pane))
        return

    try:
        start_seconds = left(args.start_timeout_ms / 1000.0)
        if start_seconds is None:
            return
        started = test_hooks.start_agent_when_shell_ready(
            rpc, agent, pane_id, int(start_seconds * 1000)
        )
        harness.non_empty(
            "%s agent.start 返回 argv" % agent, " ".join(started.get("argv") or [])
        )
        startup = left(args.startup_prompt_seconds)
        if startup is None:
            return
        test_hooks.dismiss_startup_prompt(rpc, pane_id, agent, startup)
        wait = left(args.wait_seconds)
        if wait is None:
            return
        pane = test_hooks.wait_until(
            rpc, pane_id, lambda state: state.get("agent") == agent, wait
        )
        if not harness.equal("%s pane 识别出 agent" % agent, agent, pane.get("agent")):
            return
        time.sleep(min(args.settle_seconds, max(0.0, deadline - time.time())))
        startup = left(args.startup_prompt_seconds)
        if startup is None:
            return
        test_hooks.dismiss_startup_prompt(rpc, pane_id, agent, startup)

        # 通知：一轮提示必须上报 working，再回到 idle/done。
        wait = left(args.wait_seconds)
        if wait is None:
            return
        round_result = test_hooks.run_prompt_round(
            rpc, pane_id, args.prompt, wait, agent=agent
        )
        harness.expect(
            bool(round_result["seen_working"]),
            "%s 上报过 working" % agent,
            "至少一次 agent_status=working",
            repr(round_result["last_status"]),
        )
        harness.expect(
            bool(round_result["idle_after_working"]),
            "%s 收尾回到 idle/done" % agent,
            "working 之后出现 idle/done",
            repr(round_result["last_status"]),
        )
        entry = test_hooks.agent_entry(rpc, pane_id)
        session = entry.get("agent_session") or {}
        harness.expect(
            session.get("source") == "hicodeer:" + agent,
            "%s 状态上报来自 Hook 集成" % agent,
            "hicodeer:%s" % agent,
            repr(session.get("source")),
        )

        # A working report is a tool call, not a reply: no text may be attached
        # to it, or the notification would quote the command being run.
        working_entry = next(
            (
                candidate
                for candidate in rpc.agents()
                if candidate.get("pane_id") == pane_id
                and candidate.get("agent_status") == "working"
            ),
            None,
        )
        if working_entry is not None:
            harness.expect(
                working_entry.get("last_reply") in (None, ""),
                "%s working 态不携带正文" % agent,
                "null/空",
                repr(working_entry.get("last_reply")),
            )

        # 最后答复：agent.list 与 agent.reply 给出同一段文本。A missing reply
        # is a result, so the wait is short: waiting longer only delays the
        # verdict on code that is not going to produce one.
        reply_wait = left(args.reply_seconds)
        if reply_wait is None:
            return
        entry = test_last_reply.wait_for_reply(rpc, pane_id, reply_wait)
        harness.expect(
            entry.get("completion_seq") is not None,
            "%s 本轮完成带 completion_seq" % agent,
            "非 null",
            repr(entry.get("completion_seq")),
        )
        reply = entry.get("last_reply")
        harness.expect(
            isinstance(reply, str) and args.expect in reply,
            "%s last_reply 含期望文本" % agent,
            "包含 %r" % args.expect,
            repr(reply),
        )
        direct = rpc.call("agent.reply", {"target": pane_id}).get("reply")
        harness.expect(
            isinstance(direct, str) and args.expect in direct,
            "%s agent.reply 含期望文本" % agent,
            "包含 %r" % args.expect,
            repr(direct),
        )
        harness.equal("%s agent.reply 与 agent.list 一致" % agent, reply, direct)

        # 历史会话：扫描能给出这个 agent 的条目。
        check_sessions(harness, rpc, agent)

        # Always the second round: a reply that exists is not enough, the text
        # the user has to answer must be there too.
        test_last_reply.check_waiting(harness, rpc, pane_id, args, deadline)
    finally:
        if not args.keep:
            rpc.close_workspace(workspace.get("workspace_id") or "", close_group=True)


def body(harness, args):
    rpc = agent_rpc.AgentRpc(socket_path=args.socket, timeout=args.timeout)
    rpc.ping()
    # One run covers every agent. A crash in one must not hide the rest, so each
    # agent's failure is recorded and the loop continues.
    for agent in selected_agents(args.agents):
        try:
            check_agent(harness, rpc, args, agent)
        except Exception as error:
            harness.fail("%s 执行未完成" % agent, "无异常", repr(error))
    return True


def add_arguments(parser):
    parser.add_argument(
        "--agents",
        default="all",
        help="逗号分隔的 agent 名单；默认 all，一次跑完整个清单",
    )
    parser.add_argument("--cwd", default=None, help="新建 workspace 的工作目录")
    parser.add_argument("--prompt", default=DEFAULT_PROMPT, help="用于触发一轮工作的通知提示")
    parser.add_argument("--expect", default=DEFAULT_EXPECT, help="最后答复应包含的文本")
    parser.add_argument("--show-reply", action="store_true", help="打印取到的正文")
    parser.add_argument("--wait-seconds", type=float, default=10.0, help="单轮状态等待上限：问题 10 秒内必答")
    parser.add_argument("--reply-seconds", type=float, default=5.0, help="等待 last_reply 出现的上限")
    parser.add_argument(
        "--settle-seconds",
        type=float,
        default=12.0,
        help="识别出 agent 后的稳定期：TUI 启动要时间，太早发指令会被吞掉",
    )
    parser.add_argument("--start-timeout-ms", type=int, default=15000, help="agent.start 的启动超时")
    parser.add_argument("--startup-prompt-seconds", type=float, default=6.0, help="等待启动提示的上限")
    parser.add_argument("--keep", action="store_true", help="结束后不关闭新建的 workspace")


if __name__ == "__main__":
    raise SystemExit(run_guarded("Agent 三项能力校验", body, add_arguments))
