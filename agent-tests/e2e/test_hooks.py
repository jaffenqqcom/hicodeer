#!/usr/bin/env python3
"""端到端校验：hook 集成是否装好，且真的把状态报到了 pane 上。

与 test_scan / test_resume 共用 `agent_rpc`，只走 herdr 的 agent API：

  1. `integration.list` 断言目标可用；必要时 `integration.install` 后断言
     `state == current`（自动安装那条链路）。
  2. 新建独立 space，`agent.start` 起该 agent 的 TUI，等 pane 识别出 agent。
  3. `pane.send_text` + `pane.send_keys` 发一轮提示，轮询 `pane.get` 的
     `agent_status`，要求先出现 `working`、再回到 `idle`。
  4. 断言 `agent_session` 的 `source` 是 `hicodeer:<agent>`，即上报来自我们装的 hook。

覆盖有 hook 的四个 agent：zcode / codewhale / atomcode / deveco。
openclaw 在 `agent --local` 下两套 hook 都不触发（2026-10-28 实测），不在此列。
"""

import argparse
import os
import time

import agent_rpc
from agent_rpc import run_guarded

DEFAULT_PROMPT = "用 bash 执行 echo hi，然后只回复两个字：成功"

# 按配置复杂度排，简单的前面先跑。
HOOK_AGENTS = ["deveco", "atomcode", "codewhale", "zcode"]

# 启动提示的轮询是 0.5 秒一轮；连续这么多轮没看到提示就认为启动提示已经处理完。
QUIET_ROUNDS_BEFORE_DONE = 12

# 提示送进输入框后等 TUI 摆好窗口，再按回车：TUI 首帧未就绪时会把第一个回车吞掉。
TUI_READY_DELAY_SECONDS = 1.0
# 按下回车后，状态在这个窗口内没离开起始值就认为没提交，再按一次。
PROMPT_CONFIRM_SECONDS = 12.0
PROMPT_SUBMIT_ATTEMPTS = 3
# 重发回车前再扫一遍启动提示的窗口（秒）。
STARTUP_PROMPT_RESCAN_SECONDS = 8.0
# 新 pane 的 shell 就绪前 agent.start 会回 agent_pane_busy，最多等这么久。
START_AGENT_WAIT_SECONDS = 20.0

# DevEco Code asks permission for the working directory on every TUI start and
# only offers a session-scoped answer; it may also offer a self-update. Both
# block the prompt box, so they are dismissed before the first message.
STARTUP_PROMPTS = {
    "deveco": [
        ("Safety Check:", ("down", "enter")),
        ("Update Available", ("escape",)),
    ],
}

# 有真实「等审批」来源、能测 blocked 的 agent。deveco 的插件把
# `permission.asked` 映射成 blocked，其它三个的命令式 hook 没有审批类事件。
BLOCKED_AGENTS = ["deveco", "zcode"]
# 每个 agent 触发审批的方式不同：deveco 靠临时把 bash 改成 ask，zcode 则要一条
# 它自己会拦下来确认的命令。
BLOCKED_PROMPTS = {
    "zcode": "用 bash 执行 rm -rf /storage/Users/currentUser/tmp/zz-nonexistent-dir，然后只回复两个字：成功",
}
# 运行中会弹审批框的 agent：不答就会一直停在 working（codewhale 的 hook 事件集
# 里没有审批类事件，报不出 blocked）。
PROMPT_APPROVALS = {
    "codewhale": ("Do you want to proceed?", ("1",)),
}
DEFAULT_BLOCKED_PROMPT = "用 bash 执行 echo hi，然后只回复两个字：成功"
DEVECO_CONFIG_PATH = os.path.join(
    os.path.expanduser("~"), ".config", "deveco", "deveco.jsonc"
)
DEVECO_PERMISSION_ENTRY = '  "permission": {"bash": "ask"},\n'


class DevecoPermissionAsk(object):
    """把 deveco 的 bash 权限临时改成 ask，让工具调用停下来等审批。

    deveco.jsonc 是带注释的 JSONC，这里只做文本级插入并在退出时按原字节还原，
    不重排用户原有的任何内容。
    """

    def __init__(self):
        self.original = None
        self.injected = False

    def __enter__(self):
        try:
            with open(DEVECO_CONFIG_PATH, encoding="utf-8") as handle:
                text = handle.read()
        except OSError:
            return self
        if '"permission"' in text:
            return self
        brace = text.find("{")
        if brace < 0:
            return self
        self.original = text
        self.injected = True
        updated = text[: brace + 1] + "\n" + DEVECO_PERMISSION_ENTRY + text[brace + 1 :]
        with open(DEVECO_CONFIG_PATH, "w", encoding="utf-8") as handle:
            handle.write(updated)
        return self

    def __exit__(self, *_error):
        if self.original is None:
            return False
        with open(DEVECO_CONFIG_PATH, "w", encoding="utf-8") as handle:
            handle.write(self.original)
        return False


def integration_entry(rpc, target):
    result = rpc.call("integration.list", {})
    for entry in result.get("integrations") or []:
        if entry.get("target") == target:
            return entry
    return None


def ensure_installed(harness, rpc, target, allow_install):
    """断言目标可用，并在需要时走一次真实的自动安装。"""
    entry = integration_entry(rpc, target)
    if entry is None:
        harness.fail(
            "%s 出现在 integration.list" % target,
            "条目存在",
            "缺失（daemon 是否是这个构建？）",
        )
        return False
    harness.expect(
        entry.get("available") is True,
        "%s 的命令可用" % target,
        "available=true",
        repr(entry.get("available")),
    )
    if entry.get("state") == "current":
        harness.ok("%s 集成已是 current" % target)
        return True
    if not allow_install:
        harness.fail(
            "%s 集成未安装" % target, "state=current", repr(entry.get("state"))
        )
        return False
    harness.info("      installing %s (state=%s)" % (target, entry.get("state")))
    rpc.call("integration.install", {"target": target}, timeout=120.0)
    entry = integration_entry(rpc, target)
    actual = entry.get("state") if entry else None
    harness.equal("%s 安装后状态" % target, "current", actual)
    return actual == "current"


def agent_entry(rpc, pane_id):
    """`agent.list` 里该 pane 的条目（pane.get 不含通知相关字段）。"""
    for entry in rpc.call("agent.list", {}).get("agents") or []:
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


def dismiss_startup_prompt(rpc, pane_id, agent, timeout):
    prompts = STARTUP_PROMPTS.get(agent)
    if not prompts:
        return
    deadline = time.time() + timeout
    quiet_rounds = 0
    while time.time() < deadline:
        text = rpc.call(
            "pane.read", {"pane_id": pane_id, "source": "visible", "lines": 30}
        )["read"]["text"]
        matched = next(
            (keys for needle, keys in prompts if needle in text),
            None,
        )
        if not matched:
            quiet_rounds += 1
            if quiet_rounds >= QUIET_ROUNDS_BEFORE_DONE:
                return
            time.sleep(0.5)
            continue
        quiet_rounds = 0
        for key in matched:
            rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": [key]})
            time.sleep(0.4)


def start_agent_when_shell_ready(rpc, agent, pane_id, timeout_ms):
    """启动 agent，期间重试到新 pane 的 shell 就绪为止。

    新建 space 的 root pane 偶尔还没交出可用的 shell，服务端会回
    `agent_pane_busy`；那是启动时序而不是功能缺陷。
    """
    deadline = time.time() + START_AGENT_WAIT_SECONDS
    while True:
        try:
            return rpc.start_agent(agent, pane_id, timeout_ms=timeout_ms)
        except agent_rpc.RpcError as error:
            if error.code != "agent_pane_busy" or time.time() >= deadline:
                raise
            time.sleep(1.0)


def run_prompt_round(rpc, pane_id, prompt, timeout, agent=None):
    """Sends one prompt and follows the pane's state until it settles.

    The first return can be swallowed while a TUI is still laying out its
    window, which leaves the text sitting in the prompt box with the pane
    reporting idle. When the state has not moved off its starting value within
    `PROMPT_CONFIRM_SECONDS`, the prompt is submitted again instead of failing
    the run.
    """
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
    approval = PROMPT_APPROVALS.get(agent)
    approved = False

    while time.time() < deadline:
        pane = pane_state(rpc, pane_id)
        status = pane.get("agent_status")
        if approval and not approved and status == "working":
            text = rpc.call(
                "pane.read", {"pane_id": pane_id, "source": "visible", "lines": 30}
            )["read"]["text"]
            if approval[0] in text:
                for key in approval[1]:
                    rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": [key]})
                    time.sleep(0.3)
                approved = True
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
            # A prompt that arrived late (an update notice, for instance) may
            # have taken the box since the last check.
            dismiss_startup_prompt(rpc, pane_id, agent, STARTUP_PROMPT_RESCAN_SECONDS)
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


def check_agent(harness, rpc, args, agent):
    harness.info("")
    harness.info("== %s" % agent)
    target = agent
    if not ensure_installed(harness, rpc, target, not args.no_install):
        return

    created = rpc.create_workspace(cwd=args.cwd, focus=True, label="hook-test-%s" % agent)
    workspace = created.get("workspace") or {}
    root_pane = created.get("root_pane") or {}
    pane_id = root_pane.get("pane_id")
    if not pane_id:
        harness.fail("%s 拿到 root pane" % agent, "pane_id", repr(root_pane))
        return

    try:
        started = start_agent_when_shell_ready(rpc, agent, pane_id, args.start_timeout_ms)
        harness.non_empty("%s agent.start 返回 argv" % agent, " ".join(started.get("argv") or []))

        dismiss_startup_prompt(rpc, pane_id, agent, args.startup_prompt_seconds)

        def identified(pane):
            return pane.get("agent") == agent

        pane = wait_until(rpc, pane_id, identified, args.wait_seconds)
        if not harness.equal("%s pane 识别出 agent" % agent, agent, pane.get("agent")):
            return
        wait_until(rpc, pane_id, lambda p: p.get("agent_status") != "unknown", 15)
        time.sleep(2)
        # Some TUIs raise a second prompt (an update notice) a few seconds after
        # the first one, so the box is checked again right before typing.
        dismiss_startup_prompt(rpc, pane_id, agent, args.startup_prompt_seconds)

        pane = run_prompt_round(rpc, pane_id, args.prompt, args.wait_seconds, agent)
        harness.expect(
            bool(pane["seen_working"]),
            "%s 上报过 working" % agent,
            "至少一次 agent_status=working",
            repr(pane["last_status"]),
        )
        harness.expect(
            bool(pane["idle_after_working"]),
            "%s 收尾回到 idle" % agent,
            "working 之后出现 idle",
            repr(pane["last_status"]),
        )

        session = pane["pane"].get("agent_session") or {}
        # The server marks a Working -> Idle transition as a real completion on
        # `completion_seq` and leaves it unset when the turn was interrupted, so
        # this is the server's own record of "finished", not a UI detail.
        harness.expect(
            agent_entry(rpc, pane_id).get("completion_seq") is not None,
            "%s 完成标记 completion_seq" % agent,
            "非 null",
            repr(agent_entry(rpc, pane_id).get("completion_seq")),
        )

        harness.equal(
            "%s agent_session.source" % agent,
            "hicodeer:%s" % agent,
            session.get("source"),
        )
        harness.equal(
            "%s agent_session.agent" % agent, agent, session.get("agent")
        )
    finally:
        if not args.keep:
            rpc.close_workspace(workspace.get("workspace_id") or "", close_group=True)


def check_blocked(harness, rpc, args, agent):
    """断言「等审批」时 pane 会变成 blocked。

    靠 agent 自己的审批链路产生状态：把 bash 权限临时改成 ask，再让它执行一条
    bash 命令，插件收到 permission 事件后应上报 blocked。
    """
    harness.info("")
    harness.info("== %s（blocked 审批）" % agent)
    with DevecoPermissionAsk() as permission:
        # The deveco round depends on the injected permission rule: running it
        # without one would just prove that a plain prompt finishes.
        if agent == "deveco" and not permission.injected:
            harness.fail(
                "%s 把 bash 权限改成 ask" % agent,
                "已在 %s 注入 permission" % DEVECO_CONFIG_PATH,
                "配置不存在、读不了、或已带 permission 键",
            )
            return
        created = rpc.create_workspace(
            cwd=args.cwd, focus=True, label="hook-blocked-%s" % agent
        )
        workspace = created.get("workspace") or {}
        root_pane = created.get("root_pane") or {}
        pane_id = root_pane.get("pane_id")
        if not pane_id:
            harness.fail("%s blocked 拿到 root pane" % agent, "pane_id", repr(root_pane))
            return
        try:
            start_agent_when_shell_ready(rpc, agent, pane_id, args.start_timeout_ms)
            dismiss_startup_prompt(rpc, pane_id, agent, args.startup_prompt_seconds)
            if not harness.equal(
                "%s blocked pane 识别出 agent" % agent,
                agent,
                wait_until(
                    rpc, pane_id, lambda p: p.get("agent") == agent, args.wait_seconds
                ).get("agent"),
            ):
                return
            wait_until(rpc, pane_id, lambda p: p.get("agent_status") != "unknown", 15)
            time.sleep(2)
            dismiss_startup_prompt(rpc, pane_id, agent, args.startup_prompt_seconds)

            blocked_prompt = BLOCKED_PROMPTS.get(agent, args.blocked_prompt)
            rpc.call("pane.send_text", {"pane_id": pane_id, "text": blocked_prompt})
            time.sleep(TUI_READY_DELAY_SECONDS)
            deadline = time.time() + args.wait_seconds
            next_submit = time.time()
            submits = 0
            pane = {}
            while time.time() < deadline:
                pane = pane_state(rpc, pane_id)
                if pane.get("agent_status") == "blocked":
                    break
                if submits < PROMPT_SUBMIT_ATTEMPTS and time.time() >= next_submit:
                    dismiss_startup_prompt(
                        rpc, pane_id, agent, STARTUP_PROMPT_RESCAN_SECONDS
                    )
                    rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["enter"]})
                    submits += 1
                    next_submit = time.time() + PROMPT_CONFIRM_SECONDS
                time.sleep(0.5)
            harness.equal(
                "%s 审批时上报 blocked" % agent,
                "blocked",
                pane.get("agent_status"),
            )
            session = pane.get("agent_session") or {}
            harness.equal(
                "%s blocked 时 agent_session.source" % agent,
                "hicodeer:%s" % agent,
                session.get("source"),
            )
        finally:
            if not args.keep:
                rpc.close_workspace(
                    workspace.get("workspace_id") or "", close_group=True
                )


def body(harness, args):
    agents = [name.strip() for name in args.agents.split(",") if name.strip()]
    if not agents:
        agents = HOOK_AGENTS if args.all else []
    if not agents:
        harness.fail("agent 名单", "非空", "用 --all 或 --agents zcode,codewhale")
        return harness.finish()

    rpc = agent_rpc.AgentRpc(socket_path=args.socket, timeout=args.timeout)
    rpc.ping()

    for agent in agents:
        if agent not in HOOK_AGENTS:
            harness.warn("跳过 %s：这个脚本只覆盖 %s" % (agent, ", ".join(HOOK_AGENTS)))
            continue
        check_agent(harness, rpc, args, agent)
        if args.blocked and agent in BLOCKED_AGENTS:
            check_blocked(harness, rpc, args, agent)

    return harness.finish()


def add_arguments(parser):
    parser.add_argument(
        "--all", action="store_true", help="跑全部四个有 hook 的 agent"
    )
    parser.add_argument(
        "--agents", default="", help="逗号分隔的 agent 名单，例如 zcode,codewhale"
    )
    parser.add_argument(
        "--no-install",
        action="store_true",
        help="不安装，只校验；未安装时判失败而不是自动装",
    )
    parser.add_argument(
        "--cwd", default=None, help="workspace 的工作目录，默认沿用 daemon 当前目录"
    )
    parser.add_argument(
        "--prompt", default=DEFAULT_PROMPT, help="发给 agent 的一轮提示"
    )
    parser.add_argument(
        "--wait-seconds",
        type=float,
        default=180.0,
        help="等待 agent 识别与空闲的上限，默认 180 秒",
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
        default=30.0,
        help="等待 agent 首次启动提示（如 deveco 的目录信任）的上限，默认 30 秒",
    )
    parser.add_argument(
        "--no-blocked",
        dest="blocked",
        action="store_false",
        help="跳过 blocked（等审批）校验；默认对 %s 开启" % ", ".join(BLOCKED_AGENTS),
    )
    parser.add_argument(
        "--blocked-prompt",
        default=DEFAULT_BLOCKED_PROMPT,
        help="用于触发审批的提示，默认让 agent 执行一条 bash 命令",
    )
    parser.add_argument(
        "--keep", action="store_true", help="结束后不关闭本轮新建的 workspace"
    )


if __name__ == "__main__":
    raise SystemExit(run_guarded("hook 集成端到端校验", body, add_arguments))
