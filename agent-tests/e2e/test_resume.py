#!/usr/bin/env python3
"""Resume check: open one history session and prove an agent pane came back.

The sequence is the panel's own (crates/agent_panel/src/client.rs:185-262):

    1. `workspace.create` {"cwd": <session root>, "focus": true, "reuse_root": true}
       -> {"type":"workspace_created","workspace":{...},"root_pane":{...}}
    2. `agent.start` {"kind": <agent>, "pane_id": <root pane>,
                       "resume_session": <session id>, "timeout_ms": 30000}
       -> {"type":"agent_started","agent":{...},"argv":[...]}

There is no dedicated `agent.resume` method. The session id travels in
`agent.start`'s `resume_session` field (schema/agents.rs:180); the server builds
the per-agent resume command in `resume_args_for` -> `agent_resume::plan`
(herdr/src/app/agents.rs:568-590, herdr/src/agent_resume.rs:204-366), prefixes
the agent's executable and returns the whole argv. `resume_session` and `args`
are mutually exclusive: sending both is rejected with `invalid_agent_argument`
(agents.rs:300-310), so this script asserts the returned argv against the
server's own table (`expected_resume_argv`, copied from that match).

The panel no longer keeps a resumable-agent whitelist: it forwards any session
that has a workspace root and lets the server answer. There is no hardcoded
"cannot resume" list here either — `opendesk` used to have no `workspace_root`,
but `sessions/opendesk.rs::workspace_of` now reads the directory out of the
task's system prompt, so it takes the normal path.

A `resume_unsupported` reply is a **failure**, not an environment limit: every
agent this build scans must also resume (enforced by the unit test
`sessions/fad.rs::every_scanned_agent_has_a_resume_plan`), so a refusal means
the scanned set and the resume table disagree. The same rule applies to this
script's own argv table — an agent missing from it fails instead of skipping.

One agent needs a caveat: `openclaw`. The server does emit
`openclaw resume <id>`, but the CLI's `resume <query>` only matches a session
key / display name / label (https://docs.openclaw.ai/cli/resume) and the scanned
id is a UUID, so the TUI will most likely exit 1. The request is still sent and
judged on its real outcome; `--expect-known-failure` only exists to record that
outcome as expected instead of red, and the session assertion is skipped for it
because a dying CLI clears the session (`terminal/state.rs:2355-2364`).

`deepseek` (the `dsh` launcher) takes a profile name before the app's own flags,
so the server emits `dsh tui --resume <id>`. The profile has to exist under
`$DSH_HOME/profiles`: this device only has `headless`, so the argv is right but
the CLI exits with `profile "tui" does not exist` until one is created with
`dsh plugin --profile tui add <package>`. Run it without `--require-ready` and
read the argv + `agent_session` assertions; the pane failure is that missing
profile, not the resume table.

`agent_session` is asserted by default: `resume_session` is turned into a
`PersistedAgentSession` and written before `agent.start` returns
(herdr/src/app/agents.rs:237-243, :337), so a resumed pane reports
`{source: "hicodeer:<label>", agent: "<label>", kind: "id", value: "<session id>"}`.
Antigravity is the one source exception (`hicodeer:antigravity_cli`). An empty
`agent_session` is a real failure: it means the launch settled out, the process
died, or the agent opened a different session. `--no-require-agent-session`
skips the assertion (do not use it to paper over that).

Sessions whose scanner could not recover a directory still get opened: the panel
omits `cwd` and lets the server derive one from the focused space
(`herdr/src/app/api/workspaces.rs:95-112`, `creation.rs::workspace_creation_source`).
This script does the same rather than guessing a directory. `reuse_root` is inert
without a cwd (`api/workspaces.rs:48-49`), so such a session always opens a new
space — the assertions compare against the pre-run workspace list accordingly.
If `workspace.create` itself fails, nothing has been launched yet, so it is
reported as an environment problem (exit 2) with the server's own error code.

Timing note: the `agent` field — on the `agent.start` reply and on `agent.list` —
stays null while `launch_pending` is true, because the server has not identified
the process yet (`effective_known_agent()` is empty). That assertion therefore
runs after the launch settles: with `--require-ready` it waits for
`interactive_ready`; without it, a still-pending launch is reported as a skip
(`WARN`) instead of a failure. `pane_id`, `argv` and `agent_session` are all
available earlier and are asserted immediately.

UI clicks are out of scope: this machine has no gpui-mcp integration, so the
reachable surface is the server plus the data layer.

Exit codes: 0 all checks pass, 1 assertion failed, 2 environment not met
(socket missing / method unknown / workspace.create rejected).

Usage:
    python3 test_resume.py --agent codebuddy --session <id>
    python3 test_resume.py --agent deveco --session <id> --require-ready
    python3 test_resume.py --agent codebuddy --session <id> --cleanup
    python3 test_resume.py --agent openclaw --session <id> --expect-known-failure

Full sweep: `python3 test_resume.py --all` resumes the newest session of every
agent in `test_scan.DEFAULT_AGENTS`, forcing `--require-ready` on each, and
reports which agents had no session to try. It first closes every space in the
daemon (`--keep-spaces` opts out): panes and their agent processes live in the
daemon, so a leftover pane from an earlier run keeps its session locked and the
next resume fails with "open in another window". Killing the panel app does not
help — it is only a client of that daemon.
"""

import json
import sys
import time

import agent_rpc

# Agents that DO have a resume argv but are expected to fail in the CLI. Optional:
# run with --expect-known-failure to record the result as expected rather than red.
# The default path still sends the request and judges the real outcome, so an
# `resume_unsupported` reply from the server (or a healthy CLI) is reported as-is.
KNOWN_RESUME_WILL_FAIL = {
    "openclaw": "`openclaw resume <query>` 的 query 只匹配 session key / display name / label"
    "（https://docs.openclaw.ai/cli/resume），而扫描出的 session id 是 UUID，"
    "argv 会正确下发但 TUI 大概率起不来（exit 1）",
}

# The panel's retry budget for a pane whose shell is not interactive yet
# (crates/agent_panel/src/client.rs:229-259).
DEFAULT_START_ATTEMPTS = 8
DEFAULT_RETRY_DELAY = 0.3
RETRYABLE_START_CODES = ("agent_pane_busy", "agent_pane_unavailable")

READY_POLL_INTERVAL = 1.0

# The session is written before `agent.start` returns (agents.rs:337), so the
# following read should already see it. This bound only absorbs the settle edge
# case; a longer wait would turn a real failure into a slow one.
AGENT_SESSION_POLL_SECONDS = 3.0

# `official_source_for` maps every agent to `hicodeer:<label>` except Antigravity,
# whose CLI label is `agy` while its integration is `hicodeer:antigravity_cli`.
SOURCE_OVERRIDES = {"agy": "hicodeer:antigravity_cli"}


def expected_session_source(agent, label):
    """The `agent_session.source` a resumed pane must report."""
    override = SOURCE_OVERRIDES.get(agent)
    if override:
        return override
    if not label:
        return None
    return "hicodeer:%s" % label


def expected_resume_argv(agent, session_id):
    """The argv `agent.start` must return, copied from `agent_resume::plan`.

    Source: herdr/src/agent_resume.rs:204-357. Cursor's Windows spelling
    (`cursor-agent.cmd`) does not apply here. Returns None for an agent the table
    does not cover, so callers can skip instead of inventing an expectation.
    """
    separator_forms = {
        "claude": ("claude", "--resume"),
        "devin": ("devin", "--resume"),
        "droid": ("droid", "--resume"),
        "hermes": ("hermes", "--resume"),
        "gemini": ("gemini", "--resume"),
        "qodercli": ("qodercli", "--resume"),
        "codebuddy": ("codebuddy", "--resume"),
        "qwen": ("qwen", "--resume"),
        "grok": ("grok", "--resume"),
        "zcode": ("zcode", "--resume"),
        "codewhale": ("codewhale", "--resume"),
        "atomcode": ("atomcode", "--resume"),
        "opendesk": ("opendesk", "--resume"),
        "cursor": ("cursor-agent", "--resume"),
    }
    session_forms = {
        "kimi": ("kimi", "--session"),
        "pi": ("pi", "--session"),
        "opencode": ("opencode", "--session"),
        "kilo": ("kilo", "--session"),
        "deveco": ("deveco", "--session"),
        "minimax": ("mcode", "--session"),
    }
    equals_forms = {
        "copilot": ("copilot", "--resume="),
        "omp": ("omp", "--resume="),
    }
    conversation_forms = {
        "agy": ("agy", "--conversation"),
    }

    if agent in separator_forms:
        command, flag = separator_forms[agent]
        return [command, flag, session_id]
    if agent in session_forms:
        command, flag = session_forms[agent]
        return [command, flag, session_id]
    if agent in equals_forms:
        command, flag = equals_forms[agent]
        return [command, flag + session_id]
    if agent in conversation_forms:
        command, flag = conversation_forms[agent]
        return [command, flag, session_id]
    if agent == "codex":
        # Codex spells it as a subcommand, with no dashes.
        return ["codex", "resume", session_id]
    if agent == "openclaw":
        # The server does emit this, even though the CLI cannot match a UUID.
        return ["openclaw", "resume", session_id]
    if agent == "deepseek":
        # DeepSeek Harness boots a profile first; the app's own flags follow it.
        return ["dsh", "tui", "--resume", session_id]
    if agent == "mastracode":
        return ["mastracode", "--thread", session_id]
    if agent == "letta":
        prefix = "default:"
        if session_id.startswith(prefix):
            agent_id = session_id[len(prefix):]
            if not agent_id:
                return None
            return ["letta", "--conversation", "default", "--agent", agent_id]
        return ["letta", "--conversation", session_id]
    return None



def add_arguments(parser):
    parser.add_argument(
        "--all",
        action="store_true",
        help="全量模式：对 test_scan.py 名单里每个有会话的 agent 各恢复一次，"
        "全部必须达到 interactive_ready；此模式下 --agent/--session 被忽略",
    )
    parser.add_argument(
        "--agent",
        default=None,
        metavar="SLUG",
        help="要恢复的 agent（必须与服务端恢复表一致；--all 时忽略）",
    )
    parser.add_argument(
        "--session", default=None, metavar="ID", help="要恢复的历史会话 id（--all 时忽略）"
    )
    parser.add_argument(
        "--workspace-root",
        default=None,
        help="工作目录；省略时取 agent.sessions 里这条会话的 workspace_root",
    )
    parser.add_argument(
        "--start-timeout-ms",
        type=int,
        default=30000,
        help="agent.start 的启动超时（必须 >3000 且 <=300000，默认 %(default)s）",
    )
    parser.add_argument(
        "--require-ready",
        action="store_true",
        help="额外硬断言：会话在 --wait-seconds 内到达 interactive_ready"
        "（openclaw 可能停在 picker 永不就绪，叠加它大概率超时）",
    )
    parser.add_argument(
        "--wait-seconds",
        type=float,
        default=30.0,
        help="等待 interactive_ready 的秒数（默认 %(default)s）",
    )
    parser.add_argument(
        "--keep-spaces",
        action="store_true",
        help="全量模式开始前不关闭 daemon 里已有的 space（默认会关：残留的 pane 会占着"
        "会话，让下一轮报 open in another window）",
    )
    parser.add_argument(
        "--isolate-workspace",
        action="store_true",
        help="新建独立 space 而不是复用同目录的 space（--all 会自动打开，"
        "避免两个 agent 抢同一个 root pane、以及跑完留下占用会话的 pane）",
    )
    parser.add_argument(
        "--cleanup",
        action="store_true",
        help="结束后关闭本次新建的 workspace（仅当它此前不存在时）",
    )
    parser.add_argument(
        "--method-args",
        default=None,
        metavar="JSON",
        help="额外合并进 agent.start 参数的 JSON 对象，字符串里的 {session} 会替换成会话 id。"
        "不允许带 args（与 resume_session 同传会被服务端判 invalid_agent_argument）",
    )
    parser.add_argument(
        "--expect-known-failure",
        action="store_true",
        help="把该 agent 的失败记为「已知失败」（退出码 0），用于 openclaw 这类"
        "服务端有 argv 但 CLI 实际起不来的情况",
    )
    parser.add_argument(
        "--require-agent-session",
        dest="require_agent_session",
        action="store_true",
        default=True,
        help="断言 agent.list 报出的 agent_session.value 等于会话 id（默认开启）",
    )
    parser.add_argument(
        "--no-require-agent-session",
        dest="require_agent_session",
        action="store_false",
        help="跳过 agent_session 断言（例如 openclaw 这类预期起不来的 agent）",
    )


def parse_method_args(text, session_id):
    """Parse --method-args into extra agent.start params, with {session} filled in.

    `args` is rejected outright: `agent.start` treats `resume_session` and `args`
    as mutually exclusive and answers `invalid_agent_argument` when both arrive
    (herdr/src/app/agents.rs:300-310). Everything else is merged into the request.
    """
    if not text:
        return {}

    try:
        overrides = json.loads(text)
    except ValueError as error:
        raise agent_rpc.EnvironmentNotMet("--method-args 不是合法 JSON: %s" % error)
    if not isinstance(overrides, dict):
        raise agent_rpc.EnvironmentNotMet("--method-args 必须是 JSON 对象，实际是 %r" % type(overrides))

    def substitute(value):
        if isinstance(value, str):
            return value.replace("{session}", session_id)
        if isinstance(value, list):
            return [substitute(item) for item in value]
        if isinstance(value, dict):
            return {key: substitute(item) for key, item in value.items()}
        return value

    extra = {}
    for key, value in overrides.items():
        if key == "args":
            raise agent_rpc.EnvironmentNotMet(
                "--method-args 不能带 args：resume_session 与 args 互斥，"
                "服务端会回 invalid_agent_argument（herdr/src/app/agents.rs:300-310）"
            )
        elif key in ("kind", "pane_id", "timeout_ms", "resume_session"):
            # Owned by this script; a caller-supplied value would make the
            # failure messages point at the wrong thing.
            raise agent_rpc.EnvironmentNotMet(
                "--method-args 不允许覆盖 %s（由脚本负责）" % key
            )
        else:
            extra[key] = substitute(value)
    return extra


def find_session(sessions, agent, session_id):
    for entry in sessions:
        if entry.get("agent") == agent and entry.get("session_id") == session_id:
            return entry
    return None


def find_agent(agents, pane_id):
    for entry in agents:
        if entry.get("pane_id") == pane_id:
            return entry
    return None


def start_with_retry(rpc, agent, pane_id, session_id, timeout_ms, harness, extra=None):
    """`agent.start` with `resume_session`, retrying only 'shell not ready yet'."""
    last_error = None
    for attempt in range(DEFAULT_START_ATTEMPTS):
        if attempt:
            time.sleep(DEFAULT_RETRY_DELAY)
        try:
            result = rpc.start_agent(
                kind=agent,
                pane_id=pane_id,
                timeout_ms=timeout_ms,
                resume_session=session_id,
                extra=extra,
            )
            if attempt:
                harness.info("agent.start 第 %d 次尝试成功" % (attempt + 1))
            return result
        except agent_rpc.RpcError as error:
            # A refusal is never an environment limit: every agent this build
            # scans has a resume plan (enforced by the unit test
            # `every_scanned_agent_has_a_resume_plan`), so `resume_unsupported`
            # means the table and the code disagree — a defect, not a skip.
            if error.code not in RETRYABLE_START_CODES:
                raise
            last_error = error
            harness.info("agent.start 第 %d 次尝试命中 %s，重试" % (attempt + 1, error.code))
    if last_error is not None:
        raise last_error
    raise agent_rpc.RpcError("agent.start", "no_attempt", "never attempted")


def wait_for_ready(rpc, pane_id, seconds, harness):
    """Poll agent.list until the pane reports interactive_ready."""
    deadline = time.time() + seconds
    seen = None
    while time.time() < deadline:
        seen = find_agent(rpc.agents(), pane_id)
        if seen is not None and seen.get("interactive_ready") and not seen.get("launch_pending"):
            return True, seen
        time.sleep(READY_POLL_INTERVAL)
    return False, seen


def assert_agent_kind(harness, label, target, entry, settled):
    """Judge the `agent` field, which stays null until the launch settles.

    Pending launches carry no kind yet (`effective_known_agent()` is empty), so a
    null there is reported as a skip instead of a failure. Once settled it is a
    real assertion.
    """
    entry = entry or {}
    kind = entry.get("agent")
    if kind == target:
        harness.equal(label, target, kind)
        return True
    if not settled:
        harness.warn(
            "%s：pane 仍在启动（launch_pending=%r，agent=%r），跳过这条断言"
            "（要等到就绪请加 --require-ready）" % (label, entry.get("launch_pending"), kind)
        )
        return False
    harness.equal(label, target, kind)
    return False


def resume_one(harness, args):
    """One resume attempt. Returns the known-failure note, if any.

    `body` decides when to finish, because `--all` runs this once per agent and
    reports a single summary.
    """
    if args.agent in KNOWN_RESUME_WILL_FAIL:
        harness.warn(
            "已知问题：%s —— %s" % (args.agent, KNOWN_RESUME_WILL_FAIL[args.agent])
        )
    if not (3000 < args.start_timeout_ms <= 300000):
        raise agent_rpc.EnvironmentNotMet(
            "agent.start 的 timeout_ms 必须 >3000 且 <=300000，实际 %d（herdr/src/app/agents.rs:11-12）"
            % args.start_timeout_ms
        )
    # A known-broken agent still runs; --expect-known-failure records the result
    # as expected instead of red.
    known_failure = None
    if args.expect_known_failure and args.agent in KNOWN_RESUME_WILL_FAIL:
        known_failure = KNOWN_RESUME_WILL_FAIL[args.agent]

    # Parsed before any RPC so a malformed --method-args fails on its own terms.
    extra_params = parse_method_args(args.method_args, args.session)

    rpc = agent_rpc.rpc_from_args(args)
    rpc.ping()

    session = find_session(rpc.sessions(), args.agent, args.session)
    if session is None:
        harness.fail(
            "找到待恢复会话 %s:%s" % (args.agent, args.session),
            "agent.sessions 中存在该会话",
            "不存在（先跑 test_scan.py 看看扫描器有没有收到它）",
        )
        return known_failure

    workspace_root = args.workspace_root or session.get("workspace_root")
    if workspace_root:
        harness.ok("会话自带工作目录", workspace_root)
    else:
        # A session the scanner could not place still opens. The panel omits `cwd`
        # and lets the server derive one from the focused space, so this script
        # does exactly that instead of guessing a directory:
        # api/workspaces.rs:95-112 -> creation.rs::workspace_creation_source.
        # `reuse_root` is inert without a cwd (api/workspaces.rs:48-49), so this
        # always opens a new space; the baseline comparison above accounts for it.
        harness.info("会话没有 workspace_root，按面板语义省略 cwd，由服务端推导")

    baseline_workspaces = {ws.get("workspace_id") for ws in rpc.workspaces()}
    baseline_panes = {pane.get("pane_id") for pane in rpc.panes()}

    if extra_params:
        harness.info("附加到 agent.start 的参数: %s" % json.dumps(extra_params, ensure_ascii=False))

    created = None
    started = None
    try:
        # workspace_root is None for a session the scanner could not place: the
        # request then carries no `cwd` on purpose, so the server derives it.
        #
        # If the daemon has no space at all, that derivation still succeeds but
        # falls back to the `new_terminal_cwd` policy — $HOME for Follow/Home, the
        # process cwd for Current, `/` as the last resort
        # (creation.rs:103-116 -> :10-28). Such a launch is still a valid resume
        # check; this script therefore reports the landing directory instead of
        # asserting it.
        try:
            created = rpc.create_workspace(
                cwd=workspace_root,
                focus=True,
                # The panel reuses the space of the same directory, and a single
                # run must do the same. A full sweep turns this off: two agents in
                # one space compete for its root pane, and a pane that stays open
                # after the run keeps its session locked (Codewhale refuses the
                # next resume with "open in another window").
                reuse_root=not getattr(args, "isolate_workspace", False),
            )
        except agent_rpc.RpcError as error:
            # Nothing has been launched yet, so a failure here is never a
            # resume-logic bug: either there is no focused space to derive a cwd
            # from, or the directory we were handed cannot host a space. Report
            # the server's own code instead of turning it into an assertion
            # failure the reader would chase in the wrong place.
            agent_rpc.print_raw("workspace.create", rpc.last_raw)
            raise agent_rpc.EnvironmentNotMet(
                "workspace.create 失败（服务端错误码 %s）：%s" % (error.code, error.message)
            )
        workspace_id = (created.get("workspace") or {}).get("workspace_id")
        pane_id = (created.get("root_pane") or {}).get("pane_id")
        harness.non_empty("workspace.create 返回 workspace_id", workspace_id)
        harness.non_empty("workspace.create 返回 root_pane.pane_id", pane_id)
        if not workspace_id or not pane_id:
            agent_rpc.print_raw("workspace.create", rpc.last_raw)
            return known_failure

        # Reported, never asserted. Note the distinction: this is where the space
        # was created, not where the agent ends up running — a CLI may change its
        # own cwd.
        landing_cwd = (created.get("root_pane") or {}).get("cwd")
        if not landing_cwd:
            # `cwd` is skipped when unknown, and a just-created shell may not have
            # reported it yet, so fall back to foreground_cwd and then one direct
            # pane read.
            landing_cwd = (created.get("root_pane") or {}).get("foreground_cwd")
        if not landing_cwd and pane_id:
            try:
                pane = rpc.call("pane.get", {"pane_id": pane_id}).get("pane") or {}
                landing_cwd = pane.get("cwd") or pane.get("foreground_cwd")
            except agent_rpc.RpcError as error:
                harness.info("pane.get 读取创建目录失败: %s" % error)
        if landing_cwd:
            harness.info("workspace 创建目录: %s（不是 agent 最终 cwd）" % landing_cwd)

        started = start_with_retry(
            rpc,
            args.agent,
            pane_id,
            args.session,
            args.start_timeout_ms,
            harness,
            extra=extra_params,
        )
        started_agent = started.get("agent") or {}
        harness.equal("agent.start 返回的 pane_id", pane_id, started_agent.get("pane_id"))
        # `agent` only becomes known once the server settles the launch
        # (`effective_known_agent()` is empty while pending), so a null here is
        # normal and the real assertion waits for readiness below.
        kind_at_start = started_agent.get("agent")
        if kind_at_start is None:
            harness.info("agent.start 返回的 agent 为 null（启动中），就绪后再断言")
        else:
            harness.equal("agent.start 返回的 agent", args.agent, kind_at_start)

        # The resume argv belongs to the server; this compares it against the
        # table copied from agent_resume::plan.
        expected_argv = expected_resume_argv(args.agent, args.session)
        returned_argv = started.get("argv") or []
        if expected_argv is None:
            # The table is what the server's plan is compared against, so a gap
            # in it would silently accept any argv. Every agent that reaches this
            # point is one the panel listed, hence one the server must resume.
            harness.fail(
                "agent %s 在 resume argv 表里有期望值" % args.agent,
                "expected_resume_argv 返回非空",
                "表里没有该 agent；实际 argv=%s（补 test_resume.py 的表，"
                "与服务端 agent_resume::plan 对齐）" % returned_argv,
            )
        else:
            expectation = expected_argv
            harness.expect(
                returned_argv == expectation,
                "agent.start 返回的 argv 等于服务端恢复命令",
                expectation,
                "%s（若 daemon 未重启，旧映像会忽略 resume_session 而只起裸可执行文件）"
                % returned_argv,
            )

        agents = rpc.agents()
        found = find_agent(agents, pane_id)
        if not harness.expect(
            found is not None,
            "agent.list 出现新 pane",
            "pane %s 在 agent.list 中" % pane_id,
            "不在（当前 %d 个 agent，pane 集合新增 %s）"
            % (len(agents), sorted({pane.get("pane_id") for pane in agents} - baseline_panes)),
        ):
            agent_rpc.print_raw("agent.start", rpc.last_raw)
            return known_failure
        harness.equal("新 pane 属于新建的 workspace", workspace_id, found.get("workspace_id"))

        session_info = found.get("agent_session")
        # openclaw is expected to die at the prompt, and dying clears the session
        # (terminal/state.rs:2355-2364), so a session assertion there would only
        # restate the known failure.
        check_session = args.require_agent_session and args.agent not in KNOWN_RESUME_WILL_FAIL
        if not isinstance(session_info, dict) and check_session:
            # Written before `agent.start` returns, so this rarely loops; it only
            # absorbs the settle edge case.
            deadline = time.time() + min(AGENT_SESSION_POLL_SECONDS, args.wait_seconds)
            while time.time() < deadline and not isinstance(session_info, dict):
                time.sleep(READY_POLL_INTERVAL)
                refreshed = find_agent(rpc.agents(), pane_id)
                session_info = (refreshed or {}).get("agent_session")
        if isinstance(session_info, dict):
            harness.equal(
                "agent.list 报出的 agent_session.value", args.session, session_info.get("value")
            )
            harness.equal(
                "agent.list 报出的 agent_session.agent", args.agent, session_info.get("agent")
            )
            label = session_info.get("agent")
            harness.equal(
                "agent.list 报出的 agent_session.source",
                expected_session_source(args.agent, label),
                session_info.get("source"),
            )
        elif check_session:
            harness.fail(
                "agent.list 报出 agent_session",
                "非空对象（value = 会话 id）",
                "null；%.0fs 内该 pane 没有把这次启动记成会话"
                "（settle 超时或 agent 自己开了新会话都会清掉它，属真实失败）"
                % min(AGENT_SESSION_POLL_SECONDS, args.wait_seconds),
            )
        else:
            harness.info(
                "跳过 agent_session 断言（--no-require-agent-session 或该 agent 预期起不来）"
            )
        if found.get("launch_pending") is True:
            # The server reports pending until the CLI reaches its prompt, so this
            # is not a failure on its own; --require-ready turns it into a hard
            # check by waiting for interactive_ready.
            harness.warn(
                "pane %s 仍处于 launch_pending（服务端仍在等 CLI 就绪）" % pane_id
            )

        if args.require_ready:
            ready, seen = wait_for_ready(rpc, pane_id, args.wait_seconds, harness)
            harness.expect(
                ready,
                "%.0fs 内到达 interactive_ready" % args.wait_seconds,
                "interactive_ready=true",
                "interactive_ready=%r launch_pending=%r agent_status=%r"
                % (
                    (seen or {}).get("interactive_ready"),
                    (seen or {}).get("launch_pending"),
                    (seen or {}).get("agent_status"),
                ),
            )
            settled = ready
            latest = seen if seen is not None else find_agent(rpc.agents(), pane_id)
        else:
            harness.info("未加 --require-ready，跳过交互就绪等待")
            latest = find_agent(rpc.agents(), pane_id) or found
            settled = not latest.get("launch_pending")
        # The `agent` field is only populated after the launch settles, so this is
        # the one place it can be judged. Pending and not waiting for readiness
        # means the information is not available yet — that is a skip, not a pass
        # and not a failure.
        assert_agent_kind(harness, "新 pane 的 agent 字段", args.agent, latest, settled)
    finally:
        if args.cleanup and created is not None:
            workspace_id = (created.get("workspace") or {}).get("workspace_id")
            if workspace_id and workspace_id not in baseline_workspaces:
                try:
                    rpc.close_workspace(workspace_id)
                    harness.info("已关闭本次新建的 workspace %s" % workspace_id)
                except agent_rpc.RpcError as error:
                    harness.warn("关闭 workspace %s 失败: %s" % (workspace_id, error))
            elif workspace_id:
                harness.info("workspace %s 本轮前已存在，未关闭" % workspace_id)
        elif created is not None:
            harness.info("保留新开的 workspace 与 agent（面板行为一致；--cleanup 可关闭）")

    return known_failure


def body(harness, args):
    if args.all:
        try:
            return resume_all(harness, args)
        finally:
            # Nothing written into the shell startup file may outlive the run,
            # including the abnormal exits `run_guarded` turns into exit codes.
            remove_shell_environment()
    if not args.agent or not args.session:
        raise agent_rpc.EnvironmentNotMet(
            "--agent 与 --session 必须同时给出，或改用 --all 跑全部 agent"
        )
    known_failure = resume_one(harness, args)
    return harness.finish(known_failure=known_failure)


# Agents the full sweep cannot check yet, with the reason. They are reported as
# skipped, never as passed: the list is explicit so a new gap cannot hide here.
SWEPT_AGENTS_SKIPPED = {
    "deepseek": "dsh 的 tui profile 在本机不存在，发行包和 npm 上都没有 tui bundle，"
    "`dsh tui --resume` 无法执行；等官方发布后再打开这条",
}

# Environment a pane needs before its CLI can start at all. `agent.start` has no
# env parameter and a pane only reads the shell's startup files, so the value is
# written into `~/.zshrc` for the duration of that one check and removed right
# after (`apply_shell_environment`). Agents listed here run last, so the file is
# touched for the shortest possible window.
AGENT_SHELL_ENVIRONMENT = {
    # ZCode binds a broker socket under `os.tmpdir()`; /storage is hmdfs, where
    # that listen fails with EPERM.
    "zcode": {"TMPDIR": "/data/storage/el2/base/haps/entry/files/tmp"},
}

SHELL_ENVIRONMENT_MARKER = "herdr-test-env"


# Environment an agent needs before it can be *used*, as opposed to started: the
# CLI comes up either way, but a missing key turns the first message into an
# error. The pane inherits the shell's startup files, not this script's
# environment, so the check below reads `~/.zshrc` as well as `os.environ`.
AGENT_ENVIRONMENT = {
    "codex": ("DEEPSEEK_API_KEY",),
    "deepseek": ("DEEPSEEK_API_KEY",),
    "qwen": ("OPENAI_API_KEY",),
    "opendesk": ("OPENDESK_PROVIDER_API_KEY",),
}


def shell_startup_path():
    import os

    return os.path.expanduser("~/.zshrc")


def remove_shell_environment():
    """Drop the managed block, leaving the rest of the file untouched."""
    path = shell_startup_path()
    try:
        with open(path, "r", encoding="utf-8") as startup:
            text = startup.read()
    except OSError:
        return
    begin = "# >>> %s" % SHELL_ENVIRONMENT_MARKER
    if begin not in text:
        return
    head, _, rest = text.partition(begin)
    end = "# <<< %s <<<" % SHELL_ENVIRONMENT_MARKER
    _, _, tail = rest.partition(end)
    # `apply_shell_environment` prefixes the block with a blank line, so the
    # restored text is byte-for-byte what it was before (`tail` already starts
    # with the newline that closed the block).
    with open(path, "w", encoding="utf-8") as startup:
        startup.write(head.rstrip("\n") + "\n" + tail.lstrip("\n"))


def apply_shell_environment(harness, agent):
    """Make the agent's variables visible to the pane it is about to start.

    A pane is a fresh shell started by the daemon: it sees `~/.zshrc` and the
    daemon's own environment, never this script's, so writing the file is the
    only way to reach it. The caller's `finally` removes the block, which keeps
    the change scoped to this run.
    """
    variables = AGENT_SHELL_ENVIRONMENT.get(agent)
    remove_shell_environment()
    if not variables:
        return
    block = ["", "# >>> %s (written for one check, removed afterwards) >>>" % SHELL_ENVIRONMENT_MARKER]
    block.extend('export %s="%s"' % (name, value) for name, value in variables.items())
    block.append("# <<< %s <<<" % SHELL_ENVIRONMENT_MARKER)
    try:
        with open(shell_startup_path(), "a", encoding="utf-8") as startup:
            startup.write("\n".join(block) + "\n")
    except OSError as error:
        harness.warn("%s：无法写入 shell 启动文件（%s）" % (agent, error))
        return
    harness.info(
        "%s：临时写入 shell 启动文件 %s（本条检查结束后删除）"
        % (agent, ", ".join("%s=%s" % item for item in variables.items()))
    )


def clean_spaces(harness, rpc):
    """Close every space before a full sweep, so a previous run cannot poison it.

    Panes and their agent processes live in the daemon, not in the panel: killing
    the app leaves them running, and an agent that is still alive keeps its
    session locked (`session … is open in another … window`) so the next resume
    fails for a reason that has nothing to do with resume. Closing the space is
    what actually reaps the pane and its process group.

    This is destructive for whatever the user had open in that daemon, which is
    why `--all` opts in and `--keep-spaces` turns it off.
    """
    spaces = [space.get("workspace_id") for space in rpc.workspaces()]
    if not spaces:
        return
    harness.info("清场：关闭 daemon 里 %d 个既有 space（--keep-spaces 可保留）" % len(spaces))
    for workspace_id in spaces:
        try:
            rpc.close_workspace(workspace_id)
        except agent_rpc.RpcError as error:
            harness.warn("关闭 space %s 失败：%s" % (workspace_id, error.code))


def report_missing_environment(harness):
    """Name the agents that would start but fail on their first message.

    Not a failure: a resume is about reopening the pane, and a missing key is a
    property of this machine. Reporting it keeps `--all` from looking green for
    an agent nobody can actually talk to.
    """
    import os

    shell_startup = ""
    startup_path = os.path.expanduser("~/.zshrc")
    try:
        with open(startup_path, "r", encoding="utf-8", errors="replace") as startup:
            shell_startup = startup.read()
    except OSError:
        pass
    missing = []
    for agent, variables in AGENT_ENVIRONMENT.items():
        for variable in variables:
            if os.environ.get(variable) or variable in shell_startup:
                continue
            missing.append("%s 需要 %s" % (agent, variable))
    if missing:
        harness.warn(
            "这些 agent 能恢复但不带对话所需的变量（pane 只读 shell 启动文件）：%s"
            % "；".join(missing)
        )


def resume_all(harness, args):
    """Resume one session per agent in the scan list; every one must come up.

    The agent list is shared with test_scan.py on purpose: a build that lists an
    agent in the history panel must also be able to open it, so the two checks
    cannot drift apart into "scanned but not resumable". A launch that stays
    pending, dies at the prompt, or is refused by the server is a failure here —
    that is the whole point of the mode.
    """
    import copy

    import test_scan

    rpc = agent_rpc.rpc_from_args(args)
    rpc.ping()
    if not args.keep_spaces:
        clean_spaces(harness, rpc)
    report_missing_environment(harness)
    newest = {}
    for entry in rpc.sessions():
        agent = entry.get("agent")
        if not agent:
            continue
        current = newest.get(agent)
        updated = entry.get("updated_at") or 0
        if current is None or updated > (current.get("updated_at") or 0):
            newest[agent] = entry

    covered = []
    uncovered = []
    skipped = []
    # Agents that need a shell-startup change run last: the file is edited for
    # their check only, so keeping them at the end narrows that window.
    ordered = [agent for agent in test_scan.DEFAULT_AGENTS if agent not in AGENT_SHELL_ENVIRONMENT]
    ordered.extend(
        agent for agent in test_scan.DEFAULT_AGENTS if agent in AGENT_SHELL_ENVIRONMENT
    )
    for agent in ordered:
        reason = SWEPT_AGENTS_SKIPPED.get(agent)
        if reason:
            skipped.append(agent)
            print("")
            print("== %s ==" % agent)
            harness.warn("%s：本轮跳过，原因：%s" % (agent, reason))
            continue
        entry = newest.get(agent)
        if entry is None:
            uncovered.append(agent)
            continue
        covered.append(agent)
        print("")
        print("== %s ==" % agent)
        run = copy.copy(args)
        run.agent = agent
        run.session = entry["session_id"]
        run.workspace_root = None
        run.all = False
        # Readiness is the success criterion of this mode, and a leftover space
        # per agent would accumulate across a full run.
        run.require_ready = True
        run.cleanup = True
        run.isolate_workspace = True
        run.expect_known_failure = False
        apply_shell_environment(harness, agent)
        try:
            resume_one(harness, run)
        finally:
            remove_shell_environment()

    extra = sorted(set(newest) - set(test_scan.DEFAULT_AGENTS))
    if extra:
        harness.warn(
            "扫到名单外的 agent：%s（不在 test_scan.DEFAULT_AGENTS 里）" % ", ".join(extra)
        )
    if uncovered:
        harness.warn(
            "没有会话、本轮无法验证恢复：%s（先跑 test_scan.py，或让该 agent 产生一条会话）"
            % ", ".join(uncovered)
        )
    print("")
    print(
        "恢复覆盖：%d 通过 / %d 跳过 / %d 总计"
        % (len(covered), len(skipped), len(test_scan.DEFAULT_AGENTS))
    )
    if skipped:
        print("跳过的 agent 不算通过，原因见各自的 WARN 行")
    return harness.finish()


def main():
    return agent_rpc.run_guarded("test_resume", body, add_arguments)


if __name__ == "__main__":
    sys.exit(main())
