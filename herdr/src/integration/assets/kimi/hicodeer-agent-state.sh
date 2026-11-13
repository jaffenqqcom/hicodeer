#!/bin/sh
# managed by HiCodeer; reinstalling the integration replaces this file.
# HICODEER_INTEGRATION_ID=kimi
# HICODEER_INTEGRATION_VERSION=15

action="${1:-}"
case "$action" in
  session|working|blocked|idle) ;;
  *) exit 0 ;;
esac

[ "${HICODEER_ENV:-}" = "1" ] || exit 0
[ -n "${HICODEER_AGENT_SOCKET_PATH:-}" ] || exit 0
[ -n "${HICODEER_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

python3 -c '
import json
import os
import socket
import sys
import time

action = sys.argv[1]
try:
    hook_input = json.load(sys.stdin)
except Exception:
    hook_input = {}

session_id = hook_input.get("session_id")
if not isinstance(session_id, str) or not session_id:
    session_id = None

seq = time.time_ns()
def describe_prompt(hook_input, blocked=False):
    """The text the user needs in order to answer this hook."""
    if blocked:
        # A wait has to describe what the user must answer now. The last
        # assistant line belongs to the turn that just ended and would quote
        # the previous answer instead.
        hook_input = dict(hook_input)
        hook_input.pop("last_assistant_message", None)
    tool_input = hook_input.get("tool_input")
    if isinstance(tool_input, dict):
        questions = tool_input.get("questions")
        if isinstance(questions, list):
            parts = []
            for question in questions:
                if not isinstance(question, dict):
                    continue
                text = question.get("question") or question.get("header")
                if isinstance(text, str) and text.strip():
                    parts.append(text.strip())
                options = question.get("options")
                if isinstance(options, list):
                    for option in options:
                        if not isinstance(option, dict):
                            continue
                        label = option.get("label")
                        if isinstance(label, str) and label.strip():
                            parts.append("- " + label.strip())
            if parts:
                return "\n".join(parts)
        for key in ("command", "description", "prompt", "url"):
            value = tool_input.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
    # The question and its options live in tool_input, which is the text the
    # reader has to answer. The last assistant line is only a fallback for a
    # report that carries no tool input, such as a finished turn.
    last_reply = hook_input.get("last_assistant_message")
    if isinstance(last_reply, str) and last_reply.strip():
        return last_reply.strip()
    message = hook_input.get("message")
    if isinstance(message, str) and message.strip():
        return message.strip()
    return None

params = {
    "pane_id": os.environ["HICODEER_PANE_ID"],
    "source": "hicodeer:kimi",
    "agent": "kimi",
    "seq": seq,
}
if action == "session":
    if session_id is None:
        raise SystemExit(0)
    method = "pane.report_agent_session"
    params["session_start_source"] = "startup"
else:
    method = "pane.report_agent"
    params["state"] = action
    # Only the two states that need attention carry text: a finished turn
    # has its final reply, a wait has the question and its options. A working
    # report is a tool call, and its command is not a reply.
    if action in ("idle", "blocked"):
        prompt = describe_prompt(hook_input, blocked=(action == "blocked"))
        if prompt:
            params["message"] = prompt
if session_id is not None:
    params["agent_session_id"] = session_id

request = json.dumps({"id": f"hicodeer:kimi:{seq}", "method": method, "params": params})
try:
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(0.5)
        client.connect(os.environ["HICODEER_AGENT_SOCKET_PATH"])
        client.sendall((request + "\n").encode())
        client.recv(4096)
except Exception:
    pass
' "$action" 2>/dev/null || true
