#!/usr/bin/env python3
"""Panel operation check: rename / pin / archive / delete, all reversible.

What is actually testable over the RPC surface, and what is not — measured in
this tree, not assumed:

  * history title / pin / archive have **no RPC method**. The daemon only
    collects sessions; the panel keeps these three in
    `<data_dir>/agent/history.json` (crates/agent_panel/src/session_service.rs:1-6
    and :106-124). So this script exercises that file through
    `agent_rpc.HistoryState`, then asks `agent.sessions` again to prove the
    daemon's raw row did not move — the panel state is an overlay, not a rewrite.
  * `agent.rename` exists but renames the **live pane's agent**, not a history
    row (herdr/src/app/agents.rs:165-218). It is exercised only when `--pane` is
    given, because it needs a running agent.
  * `agent.session.delete` exists and removes one session through the store
    that produced it (herdr/src/app/agents.rs, routed in
    herdr/src/sessions/fad.rs). It is the only delete path there is: the panel's
    own button reaches the same RPC through `client::delete_session`
    (crates/agent_panel/src/agent_panel.rs -> crates/agent_panel/src/client.rs),
    so nothing is unlinked locally, and a row whose store reports
    `deletable: false` has no button at all. It is irreversible, so it is
    skipped unless `--allow-delete` is passed, and even then it only ever
    deletes a session that this run created itself (`--create-cmd`), never a
    pre-existing one.

UI clicks are out of scope: this machine has no gpui-mcp integration (the
inspector answers "No running GPUI app found"), so the reachable surface is the
server plus the data layer. See README.md.

Every step rolls back what it changed, including on failure. `--keep` disables
the rollback and is for debugging only.

Exit codes: 0 all checks pass, 1 assertion failed, 2 environment not met.

Usage:
    python3 test_panel_ops.py --agent codebuddy --session <id>
    python3 test_panel_ops.py --agent claude --session <id> --pane w1:p2
    python3 test_panel_ops.py --agent codebuddy --session <id> \\
        --allow-delete --create-cmd "codebuddy -p hello"
"""

import subprocess
import sys
import time

import agent_rpc

TEST_TITLE_SUFFIX = " [e2e]"
RENAME_NAME_PREFIX = "e2e-rename-"
DELETE_POLL_INTERVAL = 1.0


def add_arguments(parser):
    parser.add_argument("--agent", required=True, metavar="SLUG", help="会话所属 agent")
    parser.add_argument(
        "--session",
        default=None,
        metavar="ID",
        help="会话 id；省略时取该 agent 最近更新的一条",
    )
    parser.add_argument(
        "--data-dir",
        default=None,
        help="应用数据根，默认从 %s/%s 读取"
        % (agent_rpc.DEFAULT_FILES_ROOT, agent_rpc.DATA_DIR_RECORD_FILE),
    )
    parser.add_argument(
        "--pane",
        default=None,
        metavar="PANE_ID",
        help="可选：对该 pane 的 agent 做 agent.rename RPC 校验（需要一个运行中的 agent）",
    )
    parser.add_argument(
        "--allow-delete",
        action="store_true",
        help="允许执行不可逆的 agent.session.delete；仍只删 --create-cmd 生成的会话",
    )
    parser.add_argument(
        "--create-cmd",
        default=None,
        help="造一条新会话的 shell 命令，例如 \"codebuddy -p hello\"；配 --allow-delete 使用",
    )
    parser.add_argument(
        "--create-timeout",
        type=float,
        default=90.0,
        help="等待新会话出现在 agent.sessions 的秒数（默认 %(default)s）",
    )
    parser.add_argument(
        "--keep",
        action="store_true",
        help="不回滚（仅调试用，会把测试状态留在设备上）",
    )


def pick_session(rpc, agent, session_id, harness):
    """Return the session entry to operate on, or None."""
    sessions = rpc.sessions()
    if session_id:
        for entry in sessions:
            if entry.get("agent") == agent and entry.get("session_id") == session_id:
                return entry
        harness.fail(
            "找到指定会话 %s:%s" % (agent, session_id),
            "agent.sessions 中存在该会话",
            "共 %d 条会话，无匹配" % len(sessions),
        )
        return None
    candidates = [entry for entry in sessions if entry.get("agent") == agent]
    if not candidates:
        harness.fail(
            "agent %s 至少有一条会话" % agent,
            ">= 1 条",
            "0 条",
        )
        return None
    candidates.sort(key=lambda entry: entry.get("updated_at") or 0, reverse=True)
    chosen = candidates[0]
    harness.info("未指定 --session，选用最近一条: %s" % chosen.get("session_id"))
    return chosen


def rpc_row(rpc, agent, session_id):
    """The daemon's own row for one session, as `agent.sessions` reports it."""
    if rpc is None:
        return None
    for entry in rpc.sessions():
        if entry.get("agent") == agent and entry.get("session_id") == session_id:
            return entry
    return None


# Fields the panel's history state must never touch: it is an overlay, not a
# rewrite of what the daemon scanned.
RPC_ROW_FIELDS = ("agent", "session_id", "title", "workspace_root", "source_path")


def row_projection(entry):
    if entry is None:
        return None
    return {field: entry.get(field) for field in RPC_ROW_FIELDS}


def history_state_checks(harness, state, agent, session_id, keep, rpc=None):
    """rename(title) -> pin -> archive, each read back from the file.

    Also proves the panel state is an overlay: the raw `agent.sessions` row must
    stay byte-for-byte equal while history.json changes.
    """
    original_raw = state.read_raw()
    original_entry = state.entry(agent, session_id)
    row_before = rpc_row(rpc, agent, session_id)
    harness.info("history.json: %s" % ("不存在" if original_raw is None else "存在"))
    if rpc is not None:
        harness.expect(
            row_before is not None,
            "agent.sessions 中存在该会话的原始行",
            "1 行",
            "0 行（history.json 断言仍会执行）",
        )

    try:
        # 1. History title rename.
        base_title = ""
        if original_entry and isinstance(original_entry.get("title_override"), str):
            base_title = original_entry["title_override"]
        title = (base_title or agent) + TEST_TITLE_SUFFIX
        state.set_entry(agent, session_id, title=title)
        stored = state.entry(agent, session_id) or {}
        harness.equal("history 标题写回", title, stored.get("title_override"))

        # 2. Pin.
        state.set_entry(agent, session_id, pinned=True)
        stored = state.entry(agent, session_id) or {}
        harness.equal("history pin 写回", True, stored.get("pinned"))

        # 3. Archive. Pinned wins in the view, but both flags are stored
        # independently (crates/agent_panel/src/session_service.rs:191-209).
        state.set_entry(agent, session_id, archived=True)
        stored = state.entry(agent, session_id) or {}
        harness.equal("history archive 写回", True, stored.get("archived"))
        harness.equal("pin 与 archive 可同时存在", True, stored.get("pinned"))

        # 4. Unpin keeps the archive flag.
        state.set_entry(agent, session_id, pinned=False)
        stored = state.entry(agent, session_id) or {}
        harness.equal("取消 pin 后 archive 保留", True, stored.get("archived"))

        # 5. The daemon's raw row is untouched by any of the above.
        if rpc is not None:
            row_after = rpc_row(rpc, agent, session_id)
            harness.equal(
                "写 history.json 后 agent.sessions 原始行未变",
                row_projection(row_before),
                row_projection(row_after),
            )
            if (
                row_after is not None
                and row_before is not None
                and row_after.get("updated_at") != row_before.get("updated_at")
            ):
                harness.warn("该会话 updated_at 变化，可能有其它进程正在写它")
    finally:
        if keep:
            harness.warn("--keep：未回滚 history.json")
        else:
            state.restore_raw(original_raw)
            restored = state.entry(agent, session_id)
            if original_entry is None:
                harness.equal("回滚后该会话无残留条目", None, restored)
            else:
                harness.equal(
                    "回滚后 pinned 恢复原值",
                    original_entry.get("pinned", False),
                    (restored or {}).get("pinned", False),
                )
                harness.equal(
                    "回滚后 archived 恢复原值",
                    original_entry.get("archived", False),
                    (restored or {}).get("archived", False),
                )
                harness.equal(
                    "回滚后标题恢复原值",
                    original_entry.get("title_override"),
                    (restored or {}).get("title_override"),
                )
            harness.equal(
                "回滚后文件内容与原始一致",
                True,
                state.read_raw() == original_raw,
            )
            if rpc is not None:
                harness.equal(
                    "回滚后 agent.sessions 原始行仍未变",
                    row_projection(row_before),
                    row_projection(rpc_row(rpc, agent, session_id)),
                )


def agent_rename_checks(harness, rpc, pane_id, keep):
    """agent.rename on a live pane: rename, read back, restore."""
    agents = rpc.agents()
    target = None
    for entry in agents:
        if entry.get("pane_id") == pane_id:
            target = entry
            break
    if target is None:
        harness.fail(
            "找到 --pane %s 对应的 agent" % pane_id,
            "agent.list 中有该 pane",
            "pane 数 %d，无匹配" % len(agents),
        )
        return

    original_name = target.get("name")
    new_name = ("%s%d" % (RENAME_NAME_PREFIX, 1000 + int(time.time()) % 9000))[:32]
    try:
        renamed = rpc.rename(pane_id, new_name)
        harness.equal("agent.rename 返回的新名字", new_name, renamed.get("name"))
        reloaded = None
        for entry in rpc.agents():
            if entry.get("pane_id") == pane_id:
                reloaded = entry
                break
        harness.equal("agent.rename 后 agent.list 读到新名字", new_name, (reloaded or {}).get("name"))
    finally:
        if keep:
            harness.warn("--keep：未恢复 pane 上的 agent 名字")
        else:
            restored = rpc.rename(pane_id, original_name)
            harness.equal("恢复原 agent 名字", original_name, restored.get("name"))


def delete_checks(harness, rpc, agent, args):
    """Create a session with --create-cmd, then delete only that one."""
    if not args.allow_delete or not args.create_cmd:
        harness.info("跳过删除校验（需要同时给 --allow-delete 与 --create-cmd）")
        return

    baseline = {(entry.get("agent"), entry.get("session_id")) for entry in rpc.sessions()}
    harness.info("删除前基线 %d 条会话" % len(baseline))

    try:
        completed = subprocess.run(
            args.create_cmd,
            shell=True,
            capture_output=True,
            text=True,
            timeout=max(args.create_timeout, 30.0) + 30.0,
        )
    except subprocess.TimeoutExpired:
        harness.fail(
            "造会话命令在超时前结束",
            "命令自行退出",
            "%.0fs 内未退出，已杀进程" % (max(args.create_timeout, 30.0) + 30.0),
        )
        return
    if completed.returncode != 0:
        harness.fail(
            "造会话命令成功退出",
            "returncode 0",
            "returncode %d, stderr: %s"
            % (completed.returncode, (completed.stderr or "").strip()[:500]),
        )
        return
    harness.ok("造会话命令执行完毕", "returncode 0")

    deadline = time.time() + args.create_timeout
    created = None
    while time.time() < deadline:
        for entry in rpc.sessions():
            key = (entry.get("agent"), entry.get("session_id"))
            if key not in baseline and entry.get("agent") == agent:
                created = entry
                break
        if created is not None:
            break
        time.sleep(DELETE_POLL_INTERVAL)

    if created is None:
        harness.fail(
            "新会话出现在 agent.sessions 中",
            "%.0fs 内出现 %s 的新会话" % (args.create_timeout, agent),
            "未出现（基线 %d 条，当前 %d 条）" % (len(baseline), len(rpc.sessions())),
        )
        return
    harness.ok("新会话已出现", "%s:%s" % (created.get("agent"), created.get("session_id")))

    source_path = created.get("source_path") or ""
    if not created.get("deletable"):
        harness.fail(
            "该会话所在的 store 可删除",
            "deletable = true",
            "deletable = false；面板不会对这一行显示删除按钮，"
            "daemon 也会以「sessions cannot be removed here」拒绝",
        )
        harness.warn(
            "本轮造出的会话 %s:%s 未删除，需要人工清理"
            % (created.get("agent"), created.get("session_id"))
        )
        return

    # Guard: never delete something that predates this run.
    if (created.get("agent"), created.get("session_id")) in baseline:
        harness.fail("只删除本轮新建的会话", "不在基线中", "在基线中，拒绝删除")
        return
    if created.get("source_path") in {entry.get("source_path") for entry in rpc.sessions()
                                      if (entry.get("agent"), entry.get("session_id")) in baseline}:
        harness.fail("只删除本轮新建的会话", "source_path 不属于既有会话", created.get("source_path"))
        return

    rpc.delete_session(created.get("agent") or agent, source_path)
    harness.ok("agent.session.delete 调用成功", source_path)

    deadline = time.time() + args.create_timeout
    remaining = None
    while time.time() < deadline:
        remaining = [
            entry
            for entry in rpc.sessions()
            if entry.get("agent") == agent and entry.get("session_id") == created.get("session_id")
        ]
        if not remaining:
            break
        time.sleep(DELETE_POLL_INTERVAL)
    harness.expect(
        not remaining,
        "删除后该会话不再出现在 agent.sessions",
        "0 条",
        "%d 条（源文件 %s 仍在扫描结果中）" % (len(remaining or []), source_path),
    )


def body(harness, args):
    rpc = agent_rpc.rpc_from_args(args)
    rpc.ping()

    chosen = pick_session(rpc, args.agent, args.session, harness)
    if chosen is None:
        return harness.finish()
    session_id = chosen.get("session_id")
    harness.info("本次操作对象: %s:%s" % (args.agent, session_id))

    state = agent_rpc.HistoryState(data_dir=args.data_dir)
    harness.info("history.json 路径: %s" % state.path)
    history_state_checks(harness, state, args.agent, session_id, args.keep, rpc=rpc)

    if args.pane:
        agent_rename_checks(harness, rpc, args.pane, args.keep)
    else:
        harness.info("跳过 agent.rename 校验（未给 --pane；它需要一个运行中的 agent）")

    delete_checks(harness, rpc, args.agent, args)

    return harness.finish()


def main():
    return agent_rpc.run_guarded("test_panel_ops", body, add_arguments)


if __name__ == "__main__":
    sys.exit(main())
