"""Hermes plugin installed by Herdr to report resumable session identity."""

# HERDR_INTEGRATION_ID=hermes
# HERDR_INTEGRATION_VERSION=9

from __future__ import annotations

import json
import os
import socket
import time

_SOURCE = "hicodeer:hermes"
_AGENT = "hermes"
_INTERACTIVE_PLATFORMS = {"cli", "tui", "desktop", "acp"}


_GATEWAY_PROCESS: bool | None = None


def _reads_as_gateway_process() -> bool:
    """Looks for the gateway subcommand in this process's own command line.

    The gateway is a long-lived service and inherits the environment of
    whatever launched it, so its `HERDR_PANE_ID` names an unrelated pane.
    Hooks fire inside the gateway for every turn, which would report another
    pane's state and session. Hermes binds those from its status line instead.
    """
    try:
        with open("/proc/self/cmdline", "rb") as handle:
            argv = handle.read().decode("utf-8", "replace").split("\0")
    except OSError:
        return False
    return any(
        token == "gateway" and index + 1 < len(argv) and argv[index + 1] == "run"
        for index, token in enumerate(argv)
    )


def _gateway_owns_the_turn() -> bool:
    global _GATEWAY_PROCESS
    if _GATEWAY_PROCESS is None:
        _GATEWAY_PROCESS = _reads_as_gateway_process()
    return _GATEWAY_PROCESS


def _pane_id() -> str | None:
    if os.environ.get("HERDR_ENV") != "1":
        return None
    if _gateway_owns_the_turn():
        return None
    return os.environ.get("HERDR_PANE_ID", "").strip() or None


def _request(method: str, params: dict) -> None:
    """Sends one request to the agent socket the pane was started with.

    The herdr CLI resolves its own socket path, which does not exist inside a
    pane sandbox, so every CLI call here failed silently. The socket path the
    pane inherits is the one the daemon listens on, so requests go there
    directly.
    """
    socket_path = os.environ.get("HICODEER_AGENT_SOCKET_PATH", "").strip()
    if not socket_path:
        return
    request = {"id": f"{_SOURCE}:{time.time_ns()}", "method": method, "params": params}
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


def _send_session(session_id: str, start_source: str) -> None:
    pane_id = _pane_id()
    if pane_id is None:
        return
    _request(
        "pane.report_agent_session",
        {
            "pane_id": pane_id,
            "source": _SOURCE,
            "agent": _AGENT,
            "seq": time.time_ns(),
            "agent_session_id": session_id,
            "session_start_source": start_source,
        },
    )


def _report_state(state: str) -> None:
    """Reports one lifecycle state for the pane this turn runs in.

    The plugin otherwise only records session identity, which leaves the pane
    stuck on whatever screen detection saw. Reporting working before the model
    call and idle after the turn is what produces the completion transition
    the panel and the IM bridge rely on.
    """
    pane_id = _pane_id()
    if pane_id is None:
        return
    _request(
        "pane.report_agent",
        {
            "pane_id": pane_id,
            "source": _SOURCE,
            "agent": _AGENT,
            "state": state,
            "seq": time.time_ns(),
        },
    )


def _report_session(start_source: str, **kwargs) -> None:
    if kwargs.get("platform") not in _INTERACTIVE_PLATFORMS:
        return
    session_id = kwargs.get("session_id")
    if not isinstance(session_id, str) or not session_id:
        return
    _send_session(session_id, start_source)


def _session_started(**kwargs) -> None:
    _report_session("startup", **kwargs)


def _session_reset(**kwargs) -> None:
    _report_session("new", **kwargs)


def _session_observed(**kwargs) -> None:
    if kwargs.get("platform") == "cli":
        _report_session("resume", **kwargs)


def _turn_started(**kwargs) -> None:
    _session_observed(**kwargs)
    _report_state("working")


def _turn_finished(**_kwargs) -> None:
    # post_llm_call fires once per successful turn; interrupted turns do not
    # call it, so an interrupt never looks like a completion.
    _report_state("idle")


def register(ctx):
    ctx.register_hook("on_session_start", _session_started)
    ctx.register_hook("on_session_reset", _session_reset)
    ctx.register_hook("pre_llm_call", _turn_started)
    ctx.register_hook("post_llm_call", _turn_finished)
