#!/bin/sh
# managed by HiCodeer; reinstalling the integration replaces this file.
# HICODEER_AGENT_INTEGRATION_ID=qwen
# HICODEER_AGENT_INTEGRATION_VERSION=7

[ "${1:-}" = "session" ] || exit 0
[ "${HICODEER_AGENT_ENV:-}" = "1" ] || exit 0
[ -n "${HICODEER_AGENT_PANE_ID:-}" ] || exit 0
[ -n "${HICODEER_AGENT_SOCKET_PATH:-}" ] || exit 0
if [ -n "${HICODEER_AGENT_BIN_PATH:-}" ]; then
    [ -x "$HICODEER_AGENT_BIN_PATH" ] || exit 0
else
    command -v hicodeer-agent >/dev/null 2>&1 || exit 0
fi
command -v python3 >/dev/null 2>&1 || exit 0

python3 -c '
import json
import os
import subprocess
import sys
import time

try:
    payload = json.load(sys.stdin)
    session_id = payload.get("session_id")
    source = payload.get("source")
    if not isinstance(session_id, str) or not session_id:
        raise ValueError
    command = os.environ.get("HICODEER_AGENT_BIN_PATH") or "hicodeer-agent"
    args = [
        command, "pane", "report-agent-session", os.environ["HICODEER_AGENT_PANE_ID"],
        "--source", "hicodeer:qwen", "--agent", "qwen",
        "--agent-session-id", session_id, "--seq", str(time.time_ns()),
    ]
    if source in ("startup", "resume", "clear", "compact", "branch"):
        args.extend(["--session-start-source", source])
    subprocess.run(
        args,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=1,
        check=False,
    )
except Exception:
    pass
' 2>/dev/null || true
