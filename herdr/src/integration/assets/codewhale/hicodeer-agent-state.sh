#!/bin/sh
# installed by HiCodeer
# managed by HiCodeer; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HICODEER_AGENT_INTEGRATION_ID=codewhale
# HICODEER_AGENT_INTEGRATION_VERSION=16

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/hicodeer-hook.XXXXXX")" || exit 0
trap 'rm -f "$hook_input_file"' EXIT HUP INT TERM
cat >"$hook_input_file" 2>/dev/null || true

case "$action" in
  session|working|blocked|idle) ;;
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

source = "hicodeer:codewhale"
action = os.environ.get("HICODEER_AGENT_ACTION", "")
pane_id = os.environ.get("HICODEER_AGENT_PANE_ID")
socket_path = os.environ.get("HICODEER_AGENT_SOCKET_PATH")
hook_input_file = os.environ.get("HICODEER_AGENT_HOOK_INPUT_FILE")

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


hook_event_name = str(hook_input.get("hook_event_name") or "")
session_id = hook_input.get("session_id")
agent_session_id = session_id if isinstance(session_id, str) and session_id else None
# Codewhale also exports the session id; prefer the payload but fall back to
# the export so a bare event still binds the pane to its session.
if not agent_session_id:
    agent_session_id = (
        os.environ.get("CODEWHALE_SESSION_ID")
        or os.environ.get("DEEPSEEK_SESSION_ID")
        or None
    )
transcript_path = hook_input.get("transcript_path")
agent_session_path = transcript_path if isinstance(transcript_path, str) and transcript_path else None
request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
report_seq = time.time_ns()

# Subagent hooks carry an agent_id; the pane state belongs to the main agent.
if hook_input.get("agent_id"):
    raise SystemExit(0)

if action == "session":
    # The entry is registered for Codewhale's session-start event only, so the
    # event name is not checked again: Codewhale writes its own spelling there.
    if not agent_session_id:
        raise SystemExit(0)
    session_start_source = hook_input.get("source")
    if not isinstance(session_start_source, str) or not session_start_source:
        session_start_source = None
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "codewhale",
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
        "agent": "codewhale",
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
