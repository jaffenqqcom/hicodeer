#!/usr/bin/env python3
"""Scan check: every expected agent must have at least one history session.

Read-only. Calls `agent.sessions` once (method `agent.sessions`, no params,
herdr/src/api/schema.rs:120) and asserts:

  1. the daemon answers, and the result type is `agent_sessions`;
  2. each agent in the expected set contributes at least one session;
  3. every session carries a non-empty `agent`, `session_id`, `source_path`
     (the three fields the panel refuses to render a row without,
     crates/agent_panel/src/client.rs) and a boolean `deletable` (whether the
     store that produced the row can remove it);
  4. `(agent, session_id)` pairs are unique — the daemon dedups on that pair
     (herdr/src/sessions/mod.rs:50-58), so a repeat means the scan regressed.

Exit codes: 0 all checks pass, 1 assertion failed, 2 environment not met
(socket missing / method unknown).

Usage:
    python3 test_scan.py
    python3 test_scan.py --agent codebuddy --agent claude
    python3 test_scan.py --agents codebuddy,pi --min-sessions 2
"""

import sys

import agent_rpc

# The agents this build is expected to have history for on the test device.
DEFAULT_AGENTS = [
    "codebuddy",
    "deepseek",
    "deveco",
    "zcode",
    "codewhale",
    "atomcode",
    "minimax",
    "openclaw",
    "opendesk",
    "hermes",
    "claude",
    "codex",
    "gemini",
    "qwen",
    "kimi",
    "opencode",
    "pi",
]

REQUIRED_SESSION_FIELDS = ("agent", "session_id", "source_path")


def add_arguments(parser):
    parser.add_argument(
        "--agent",
        action="append",
        dest="agents",
        metavar="SLUG",
        help="期望有会话的 agent 名单，可重复（给定时替换默认名单）",
    )
    parser.add_argument(
        "--agents",
        default=None,
        dest="agents_csv",
        metavar="A,B,C",
        help="逗号分隔的期望 agent 名单（与 --agent 同义）",
    )
    parser.add_argument(
        "--min-sessions",
        type=int,
        default=1,
        metavar="N",
        help="每个 agent 至少几条会话（默认 %(default)s）",
    )


def expected_agents(args):
    names = list(args.agents or [])
    if args.agents_csv:
        names.extend(part.strip() for part in args.agents_csv.split(",") if part.strip())
    if not names:
        names = list(DEFAULT_AGENTS)
    seen = []
    for name in names:
        if name not in seen:
            seen.append(name)
    return seen


def body(harness, args):
    rpc = agent_rpc.rpc_from_args(args)
    expected = expected_agents(args)

    ping = rpc.ping()
    harness.equal("daemon 应答 ping", "pong", ping.get("type"))
    if ping.get("version"):
        harness.info("daemon 版本: %s (protocol %s)" % (ping.get("version"), ping.get("protocol")))

    sessions = rpc.sessions()
    harness.info("扫描到 %d 条会话，来自 %d 个 agent"
                 % (len(sessions), len({entry.get("agent") for entry in sessions})))

    counts = {}
    for entry in sessions:
        slug = entry.get("agent")
        counts[slug] = counts.get(slug, 0) + 1
    for slug in sorted(counts, key=lambda name: (str(name))):
        harness.info("  %-14s %d" % (slug, counts[slug]))

    # 1. Every expected agent has enough sessions.
    for slug in expected:
        count = counts.get(slug, 0)
        harness.expect(
            count >= args.min_sessions,
            "agent %s 有 >= %d 条会话" % (slug, args.min_sessions),
            ">= %d 条" % args.min_sessions,
            "%d 条" % count,
        )

    unexpected = sorted(set(counts) - set(expected))
    if unexpected:
        harness.warn("出现未在期望名单内的 agent: %s" % ", ".join(unexpected))

    # 2. Required fields are present and non-empty.
    for position, entry in enumerate(sessions):
        for field in REQUIRED_SESSION_FIELDS:
            harness.non_empty("会话 #%d 的 %s 非空" % (position, field), entry.get(field))
        # The daemon decides this per store, and the panel only shows the delete
        # button for a row that has it; a missing value would mean the field
        # never reached the DTO.
        if not isinstance(entry.get("deletable"), bool):
            harness.fail(
                "会话 #%d 的 deletable 是布尔" % position,
                "true 或 false",
                "实际 %r" % (entry.get("deletable"),),
            )
        if not entry.get("workspace_root"):
            # Not a failure, and it will keep happening: a connector that cannot
            # recover a directory still lists the session, and the daemon keeps
            # returning that row with `workspace_root: null`. What the panel does
            # with it (unknown-folder group; on open it omits `cwd` so the server
            # derives one) is UI behaviour and is not observable here.
            harness.warn(
                "会话 %s:%s 没有 workspace_root：面板会归入未知目录组，恢复时由服务端推导目录"
                % (entry.get("agent"), entry.get("session_id"))
            )

    # 3. (agent, session_id) is unique.
    seen = {}
    duplicates = []
    for entry in sessions:
        key = (entry.get("agent"), entry.get("session_id"))
        if key in seen:
            duplicates.append(key)
        seen[key] = True
    harness.expect(
        not duplicates,
        "(agent, session_id) 全局唯一",
        "无重复",
        "重复: %s" % duplicates[:5] if duplicates else "无重复",
    )

    # 4. Sanity: no session reports an empty agent after normalisation.
    empty_agent = [entry for entry in sessions if not str(entry.get("agent") or "").strip()]
    harness.expect(
        not empty_agent,
        "没有 agent 字段为空的会话",
        "0 条",
        "%d 条" % len(empty_agent),
    )

    return harness.finish()


def main():
    return agent_rpc.run_guarded("test_scan", body, add_arguments)


if __name__ == "__main__":
    sys.exit(main())
