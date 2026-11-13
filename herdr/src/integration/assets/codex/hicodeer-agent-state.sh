#!/bin/sh
# installed by HiCodeer
# managed by HiCodeer; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HICODEER_INTEGRATION_ID=codex
# HICODEER_INTEGRATION_VERSION=17

set -eu

action="${1:-}"
hook_input_file="$(mktemp "${TMPDIR:-/tmp}/hicodeer-codex-hook.XXXXXX")" || exit 0
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

source = "hicodeer:codex"
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

hook_event_name = str(hook_input.get("hook_event_name") or "")
session_id = hook_input.get("session_id")
agent_session_id = session_id if isinstance(session_id, str) and session_id else None
transcript_path = hook_input.get("transcript_path")
if not isinstance(transcript_path, str) or not transcript_path.strip():
    transcript_path = None
inherited_session_id = os.environ.get("CODEX_THREAD_ID")
if inherited_session_id and inherited_session_id != agent_session_id:
    raise SystemExit(0)

request_id = f"{source}:{int(time.time() * 1000)}:{random.randrange(1_000_000):06d}"
report_seq = time.time_ns()

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

requests = []

if action == "session":
    if not agent_session_id or not transcript_path:
        raise SystemExit(0)
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "codex",
        "seq": report_seq,
        "agent_session_id": agent_session_id,
        "agent_session_path": transcript_path,
    }
    session_start_source = hook_input.get("source")
    if hook_event_name == "SessionStart" and isinstance(session_start_source, str) and session_start_source:
        params["session_start_source"] = session_start_source
    requests.append(("pane.report_agent_session", params))
else:
    params = {
        "pane_id": pane_id,
        "source": source,
        "agent": "codex",
        "seq": report_seq,
        "state": action,
    }
    # Only the two states that need attention carry text: a finished turn
    # has its final reply, a wait has the question and its options. A working
    # report is a tool call, and its command is not a reply.
    if action in ("idle", "blocked"):
        prompt = describe_prompt(hook_input)
        if prompt:
            params["message"] = prompt
    requests.append(("pane.report_agent", params))
    # Stop carries the transcript too; refreshing the path here keeps the
    # session record current even when SessionStart ran before the rollout
    # file existed.
    if agent_session_id and transcript_path:
        requests.append((
            "pane.report_agent_session",
            {
                "pane_id": pane_id,
                "source": source,
                "agent": "codex",
                "seq": report_seq + 1,
                "agent_session_id": agent_session_id,
                "agent_session_path": transcript_path,
            },
        ))

try:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(0.5)
    client.connect(socket_path)
    for index, (method, params) in enumerate(requests):
        request = {
            "id": f"{request_id}:{index}",
            "method": method,
            "params": params,
        }
        client.sendall((json.dumps(request) + "\n").encode())
        try:
            client.recv(4096)
        except Exception:
            pass
    client.close()
except Exception:
    pass
PY
