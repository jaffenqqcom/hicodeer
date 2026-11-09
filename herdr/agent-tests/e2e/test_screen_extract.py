#!/usr/bin/env python3
"""校验屏幕提取：把 fetch_reply 的提取结果和原始 40 行屏幕逐项对比。

每个 agent 弹一道长选择题，然后：
- 用与 daemon 相同的参数取 recent_unwrapped 40 行；
- 调 agent.reply 取提取后的正文；
- 检查提取结果有没有少（缺问题、缺选项、缺描述）或多（边框、状态栏、快捷键提示）。

结果写到 --out 目录，每个 agent 一份原文 + 提取结果 + 检查结论。
"""

import argparse
import os
import re
import sys
import time

sys.path.insert(0, "/storage/Users/currentUser/workspace/hicodeer/herdr/agent-tests/e2e")
import agent_prereqs
import agent_rpc
import test_hooks
import test_last_reply

# Three lines on a typical pane: the question is deliberately long.
CHOICE_PROMPT = (
    "我在做 HarmonyOS 上的终端工具，现在有三个方向需要你帮我决定："
    "第一是优先补充多窗口支持，第二是先把现有会话恢复做稳，"
    "第三是整理界面的配色与字号。请用选择题问我这三个方向先做哪个，"
    "每个选项都写一句说明，两个以上选项，不要自己选，等我选完。"
)

BORDER_CHARS = "│┃▎╭╮╰╯─━═▀▄█"
CHROME_HINTS = [
    "esc to interrupt",
    "ctrl+c",
    "? for shortcuts",
    "shift+tab",
    "enter to select",
    "esc dismiss",
    "enter submit",
    "tok/s",
    "context ",
    "ctrl+p",
    "type something",
    "type your own answer",
    "↑↓",
    "select  enter",
    "enter submit",
]
OPTION_PATTERN = re.compile(
    r"^\s*(?:❯\s*|→\s*)?(?:\[(\d+)\]\s*(.+)|(\d+)[.、)]\s*(.+))$"
)


def option_label(line):
    match = OPTION_PATTERN.match(line)
    if not match:
        return None
    return match.group(2) if match.group(1) else match.group(4)
# Some agents draw a bar in front of every row of the panel.
FRAME_PREFIX = re.compile(r"^[│┃▎\s]+")
# A drawn choice always carries one of these footers. Numbered lines in a
# plain answer have none, so they do not count as a choice on screen.
CHOICE_FOOTERS = ("enter to select", "esc dismiss", "↑↓ select", "enter submit", "to navigate")


def fetch_screen(rpc, pane_id, lines=40):
    read = rpc.call(
        "pane.read",
        {
            "pane_id": pane_id,
            "source": "recent_unwrapped",
            "lines": lines,
            "format": "text",
            "strip_ansi": True,
        },
    )
    return (read.get("read") or {}).get("text") or ""


def compare(screen, reply):
    """Returns (problems, notes) for one agent.

    A screen with no numbered options never reached a choice, so there is
    nothing to compare; that is reported as unusable rather than as a fault.
    """
    problems = []
    notes = []
    screen_lines = [line.strip() for line in screen.splitlines() if line.strip()]
    screen_text = [FRAME_PREFIX.sub("", line).strip() for line in screen_lines]
    reply_lines = [line.strip() for line in reply.splitlines() if line.strip()]
    screen_options = [
        label for line in screen_text for label in [option_label(line)] if label
    ]
    lowered_screen = screen.lower()
    has_footer = any(footer in lowered_screen for footer in CHOICE_FOOTERS)
    if len(screen_options) < 2 or not has_footer:
        return ["未弹题（屏幕没有选择框），无法验证"], notes
    if not reply or not reply.strip():
        problems.append("提取结果为空")
        return problems, notes

    for character in BORDER_CHARS:
        if character in reply:
            problems.append("含边框字符 %r" % character)
            break
    lowered = reply.lower()
    for hint in CHROME_HINTS:
        if hint in lowered:
            problems.append("含界面提示 %r" % hint)
            break

    reply_options = [label for line in reply_lines for label in [option_label(line)] if label]
    if len(screen_options) >= 2 and len(reply_options) < 2:
        problems.append("选项缺失：屏幕 %d 个，提取 %d 个" % (len(screen_options), len(reply_options)))
    notes.append("选项：屏幕 %d，提取 %d" % (len(screen_options), len(reply_options)))

    # The first option's label and its description must survive.
    def plain(text):
        return re.sub(r"[*_`]", "", text).strip()

    if screen_options:
        first = plain(screen_options[0])
        if first not in plain(reply):
            problems.append("首个选项文本缺失 %r" % first)
    descriptions = [
        line for line in screen_text
        if line.startswith(("  ", "     ")) and not OPTION_PATTERN.match(line) and len(line) > 6
    ]
    if descriptions:
        kept = sum(1 for description in descriptions if description.strip() in reply)
        notes.append("描述行：屏幕 %d，提取 %d" % (len(descriptions), kept))
        if kept == 0:
            problems.append("选项描述全部缺失")

    # The question row must be there. Its wording is the first non-option row
    # above the first option in the screen.
    question_rows = []
    for line in screen_text:
        if OPTION_PATTERN.match(line):
            break
        if len(line) > 8 and not line.startswith(tuple(BORDER_CHARS)):
            question_rows.append(line)
    if question_rows:
        last_question = question_rows[-1]
        if last_question not in reply:
            problems.append("问题行缺失 %r" % last_question[:40])
        notes.append("问题行数：%d" % len(question_rows))

    if len(reply) > len(screen):
        problems.append("提取结果比屏幕还长")
    return problems, notes


def run_agent(rpc, agent, out_dir, cwd):
    env, reason = agent_prereqs.prepare(agent)
    if reason:
        return ["环境不可用：%s" % reason], []
    created = rpc.create_workspace(cwd=cwd, focus=True, label="screen-check-" + agent, env=env)
    pane_id = (created.get("root_pane") or {}).get("pane_id")
    workspace_id = (created.get("workspace") or {}).get("workspace_id")
    if not pane_id:
        return ["没有 root pane"], []
    try:
        test_hooks.start_agent_when_shell_ready(rpc, agent, pane_id, 30000)
        test_hooks.dismiss_startup_prompt(rpc, pane_id, agent, 15.0)
        test_hooks.wait_until(rpc, pane_id, lambda s: s.get("agent") == agent, 40.0)
        time.sleep(12)
        # The model does not always take the tool on the first ask, so the same
        # question is asked up to three times before calling it unanswered.
        screen = ""
        status = None
        for attempt in range(3):
            rpc.call("pane.send_text", {"pane_id": pane_id, "text": CHOICE_PROMPT})
            time.sleep(1.5)
            rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["enter"]})

            deadline = time.time() + 30
            while time.time() < deadline:
                entry = next((a for a in rpc.agents() if a.get("pane_id") == pane_id), {})
                status = entry.get("agent_status")
                if status == "blocked":
                    break
                if status in ("idle", "done") and time.time() > deadline - 22:
                    rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["enter"]})
                time.sleep(0.5)
            time.sleep(2.0)

            screen = fetch_screen(rpc, pane_id)
            options = [
                match
                for line in screen.splitlines()
                for match in [OPTION_PATTERN.match(line.strip())]
                if match
            ]
            if len(options) >= 2:
                break
            if attempt < 2:
                rpc.call("pane.send_keys", {"pane_id": pane_id, "keys": ["escape"]})
                time.sleep(2.0)

        reply = rpc.call("agent.reply", {"target": pane_id}).get("reply") or ""
        problems, notes = compare(screen, reply)

        os.makedirs(out_dir, exist_ok=True)
        path = os.path.join(out_dir, agent + ".txt")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("status: %s\n\n===== reply =====\n%s\n\n===== screen 40 =====\n%s\n" % (status, reply, screen))
        if status != "blocked":
            problems.append("状态未变 blocked（%s）" % status)
        return problems, notes
    finally:
        if workspace_id:
            rpc.close_workspace(workspace_id, close_group=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--agents", default="all")
    parser.add_argument("--out", default="/storage/Users/currentUser/tmp/screen_extract")
    parser.add_argument("--cwd", default="/storage/Users/currentUser/workspace/hicodeer")
    args = parser.parse_args()

    agents = test_last_reply.DEFAULT_AGENTS if args.agents == "all" else [
        agent.strip() for agent in args.agents.split(",") if agent.strip()
    ]
    rpc = agent_rpc.AgentRpc()
    rpc.ping()
    failed = []
    for agent in agents:
        print("=== %s" % agent, flush=True)
        try:
            problems, notes = run_agent(rpc, agent, args.out, args.cwd)
        except Exception as error:
            problems, notes = ["执行异常：%r" % error], []
        if problems and problems[0].startswith("未弹题"):
            print("skip  " + problems[0], flush=True)
        elif problems:
            failed.append(agent)
            print("FAIL  " + "; ".join(problems), flush=True)
        else:
            print("ok    提取结果与屏幕一致", flush=True)
        for note in notes:
            print("      " + note, flush=True)
    print("")
    print("失败：%s" % (", ".join(failed) if failed else "无"), flush=True)


if __name__ == "__main__":
    main()
