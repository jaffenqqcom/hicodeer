#!/bin/sh
# installed by HiCodeer
# managed by HiCodeer; reinstalling or updating the integration overwrites this file.
# HICODEER_INTEGRATION_ID=minimax
# HICODEER_INTEGRATION_VERSION=1

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/hicodeer-minimax-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session|working|blocked|idle) ;;
  *) exit 0 ;;
esac

[ "${HICODEER_ENV:-}" = "1" ] || exit 0
[ -n "${HICODEER_AGENT_SOCKET_PATH:-}" ] || exit 0
[ -n "${HICODEER_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HICODEER_ACTION="$action" HICODEER_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "hicodeer:minimax"
action = os.environ.get("HICODEER_ACTION", "")
pane_id = os.environ.get("HICODEER_PANE_ID")
socket_path = os.environ.get("HICODEER_AGENT_SOCKET_PATH")
hook_input_file = os.environ.get("HICODEER_HOOK_INPUT_FILE")

if not pane_id or not socket_path:
    raise SystemExit(0)

hook_input = {}
if hook_input_file:
    try:
        with open(hook_input_file, encoding="utf-8") as handle:
            content = handle.read()
        if content.strip():
            hook_input = json.loads(content)
    except Exception:
        hook_input = {}

if not isinstance(hook_input, dict):
    hook_input = {}

# A subagent turn reports on the subagent, not on the pane's own agent.
if hook_input.get("agent_id") or hook_input.get("subagent_id"):
    raise SystemExit(0)

session_id = hook_input.get("session_id")
agent_session_id = session_id if isinstance(session_id, str) and session_id else None
request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
report_seq = time.time_ns()


def describe_prompt():
    """The text the user has to answer, never the reply of the finished turn."""
    tool_input = hook_input.get("tool_input")
    if isinstance(tool_input, dict):
        for key in ("question", "header", "command", "description", "prompt"):
            value = tool_input.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
    for key in ("question", "message"):
        value = hook_input.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def last_reply():
    for key in ("last_assistant_message", "lastAssistantMessage"):
        value = hook_input.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


if action == "session":
    if not agent_session_id:
        raise SystemExit(0)
    request = {
        "id": request_id,
        "method": "pane.report_agent_session",
        "params": {
            "pane_id": pane_id,
            "source": source,
            "agent": "minimax",
            "seq": report_seq,
            "agent_session_id": agent_session_id,
        },
    }
else:
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "minimax",
        "state": action,
        "seq": report_seq,
    }
    # Only the two states that need attention carry text: a wait states the
    # question, a finished turn states its reply.
    if action == "blocked":
        text = describe_prompt()
        if text:
            params["message"] = text
    elif action == "idle":
        text = last_reply()
        if text:
            params["message"] = text
    if agent_session_id:
        params["agent_session_id"] = agent_session_id
    request = {
        "id": request_id,
        "method": "pane.report_agent",
        "params": params,
    }

try:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(0.5)
    client.connect(socket_path)
    client.sendall((json.dumps(request) + "\n").encode())
    try:
        client.recv(4096)
    except Exception:
        pass
    client.close()
except Exception:
    pass
PY
