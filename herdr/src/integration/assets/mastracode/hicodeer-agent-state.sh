#!/bin/sh
# installed by HiCodeer
# managed by HiCodeer; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HICODEER_AGENT_INTEGRATION_ID=mastracode
# HICODEER_AGENT_INTEGRATION_VERSION=6

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/hicodeer-mastracode-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session|working|idle|blocked|notification) ;;
  *) exit 0 ;;
esac

[ "${HICODEER_AGENT_ENV:-}" = "1" ] || exit 0
[ -n "${HICODEER_AGENT_SOCKET_PATH:-}" ] || exit 0
[ -n "${HICODEER_AGENT_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HICODEER_AGENT_ACTION="$action" HICODEER_AGENT_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "hicodeer:mastracode"
action = os.environ.get("HICODEER_AGENT_ACTION", "")
pane_id = os.environ.get("HICODEER_AGENT_PANE_ID")
socket_path = os.environ.get("HICODEER_AGENT_SOCKET_PATH")
hook_input_file = os.environ.get("HICODEER_AGENT_HOOK_INPUT_FILE")

if not pane_id or not socket_path:
    raise SystemExit(0)

# A Notification carries the wait reason; only the ones that park the turn on
# the user count as blocked. agent_done arrives alongside AgentEnd (which
# already reports idle), so reporting it here would flicker the panel.
if action == "notification":
    reason = hook_input.get("reason")
    if reason in ("ask_question", "tool_approval", "plan_approval", "sandbox_access"):
        action = "blocked"
    else:
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

request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
report_seq = time.time_ns()
session_id = hook_input.get("session_id")
if isinstance(session_id, str) and session_id:
    agent_session_id = session_id
else:
    agent_session_id = None
if action == "session":
    if not agent_session_id:
        raise SystemExit(0)
    request = {
        "id": request_id,
        "method": "pane.report_agent_session",
        "params": {
            "pane_id": pane_id,
            "source": source,
            "agent": "mastracode",
            "agent_session_id": agent_session_id,
            "session_start_source": "startup",
            "seq": report_seq,
        },
    }
else:
    request = {
        "id": request_id,
        "method": "pane.report_agent",
        "params": {
            "pane_id": pane_id,
            "source": source,
            "agent": "mastracode",
            "state": action,
            "seq": report_seq,
        },
    }
    if agent_session_id:
        request["params"]["agent_session_id"] = agent_session_id

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
