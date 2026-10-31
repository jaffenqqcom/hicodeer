#!/bin/sh
# installed by HiCodeer
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=copilot
# HERDR_INTEGRATION_VERSION=6

set -eu

hook_input_file="$(mktemp "${TMPDIR:-/tmp}/hicodeer-copilot-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HICODEER_AGENT_SOCKET_PATH:-}" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HERDR_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "hicodeer:copilot"
pane_id = os.environ.get("HERDR_PANE_ID")
socket_path = os.environ.get("HICODEER_AGENT_SOCKET_PATH")
hook_input_file = os.environ.get("HERDR_HOOK_INPUT_FILE")

if not pane_id or not socket_path:
    raise SystemExit(0)

hook_input = {}
if hook_input_file:
    try:
        with open(hook_input_file, encoding="utf-8") as handle:
            content = handle.read()
        if content.strip():
            parsed = json.loads(content)
            if isinstance(parsed, dict):
                hook_input = parsed
    except Exception:
        hook_input = {}

def first_text(*keys):
    for key in keys:
        value = hook_input.get(key)
        if isinstance(value, str) and value:
            return value
    return None

def normalize_event(event):
    return event.replace("_", "").replace("-", "").lower()

event = first_text("hook_event_name", "hookEventName")
normalized = normalize_event(event) if event else ""

# Event -> the pane state it reports. A question or a permission dialog parks
# the turn on the user (blocked); the working/idle edges come from the rest.
# Copilot's notification covers both agent completion and waits, so only its
# wait-shaped types count here; completion is reported by agentStop instead.
state_by_event = {
    "userpromptsubmitted": "working",
    "pretooluse": "working",
    "posttooluse": "working",
    "permissionrequest": "blocked",
    "agentstop": "idle",
}
if normalized == "sessionstart":
    action = "session"
elif normalized in state_by_event:
    action = state_by_event[normalized]
elif normalized == "notification":
    notification_type = (first_text("notification_type", "notificationType") or "").lower()
    if "elicitation" in notification_type or "permission" in notification_type:
        action = "blocked"
    else:
        raise SystemExit(0)
else:
    raise SystemExit(0)

session_id = hook_input.get("session_id")
if not isinstance(session_id, str) or not session_id:
    session_id = hook_input.get("sessionId")

if action == "session":
    if not isinstance(session_id, str) or not session_id:
        raise SystemExit(0)
    request = {
        "id": f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}",
        "method": "pane.report_agent_session",
        "params": {
            "pane_id": pane_id,
            "source": source,
            "agent": "copilot",
            "agent_session_id": session_id,
            "seq": time.time_ns(),
        },
    }
else:
    request = {
        "id": f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}",
        "method": "pane.report_agent",
        "params": {
            "pane_id": pane_id,
            "source": source,
            "agent": "copilot",
            "state": action,
            "seq": time.time_ns(),
        },
    }
    if isinstance(session_id, str) and session_id:
        request["params"]["agent_session_id"] = session_id

try:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(0.5)
    client.connect(socket_path)
    client.sendall((json.dumps(request) + "\n").encode("utf-8"))
    try:
        client.recv(4096)
    except Exception:
        pass
    client.close()
except Exception:
    pass
PY
