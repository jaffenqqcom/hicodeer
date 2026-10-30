#!/bin/sh
# installed by HiCodeer
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=codebuddy
# HERDR_INTEGRATION_VERSION=6

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/herdr-codebuddy-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session|working|blocked|idle|notification) ;;
  *) exit 0 ;;
esac

[ "${HERDR_ENV:-}" = "1" ] || exit 0
[ -n "${HICODEER_AGENT_SOCKET_PATH:-}" ] || exit 0
[ -n "${HERDR_PANE_ID:-}" ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0

HERDR_ACTION="$action" HERDR_HOOK_INPUT_FILE="$hook_input_file" python3 - <<'PY'
import json
import os
import random
import socket
import time

source = "herdr:codebuddy"
action = os.environ.get("HERDR_ACTION", "")
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
            hook_input = json.loads(content)
    except Exception:
        hook_input = {}


# A Notification carries a wait type. Permission prompts park the turn on the
# user (blocked); the idle prompt is just "no input lately" -- that is idle,
# not blocked, or the panel shows a waiting icon while the agent sits at its
# prompt.
if action == "notification":
    notification_type = str(hook_input.get("notification_type") or hook_input.get("notificationType") or "").lower()
    if "permission" in notification_type or "elicitation" in notification_type:
        action = "blocked"
    elif "idle" in notification_type:
        action = "idle"
    else:
        raise SystemExit(0)
hook_event_name = str(hook_input.get("hook_event_name") or "")
session_id = hook_input.get("session_id")
agent_session_id = session_id if isinstance(session_id, str) and session_id else None
transcript_path = hook_input.get("transcript_path")
agent_session_path = transcript_path if isinstance(transcript_path, str) and transcript_path else None
request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
report_seq = time.time_ns()

# Subagent hooks carry an agent_id; the pane state belongs to the main agent.
if hook_input.get("agent_id"):
    raise SystemExit(0)

if action == "session":
    if hook_event_name != "SessionStart" or not agent_session_id:
        raise SystemExit(0)
    session_start_source = hook_input.get("source")
    if not isinstance(session_start_source, str) or not session_start_source:
        session_start_source = None
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "codebuddy",
        "seq": report_seq,
        "agent_session_id": agent_session_id,
    }
    if agent_session_path:
        params["agent_session_path"] = agent_session_path
    if session_start_source:
        params["session_start_source"] = session_start_source
    request = {
        "id": request_id,
        "method": "pane.report_agent_session",
        "params": params,
    }
else:
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "codebuddy",
        "state": action,
        "seq": report_seq,
    }
    if agent_session_id:
        params["agent_session_id"] = agent_session_id
    if agent_session_path:
        params["agent_session_path"] = agent_session_path
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
