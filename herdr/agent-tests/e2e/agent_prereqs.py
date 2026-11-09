"""Environment variables an agent needs before the tests can drive it.

Gemini CLI only speaks the Gemini protocol, so this machine runs one local
bridge in front of DeepSeek:

  gemini -> gem-proxy on 127.0.0.1:8000 (Gemini API shape -> DeepSeek)

The bridge is shared with the interactive `gemini` command: ~/.zshrc starts it
and exports the two variables the CLI reads. A test run therefore finds it
listening and only has to re-inject the variables, because a pane gets the
shell's startup files plus the environment passed to workspace.create, never
the environment of this process. When nothing listens on the port the bridge's
own starter is invoked; it reuses a listening port and returns as soon as the
port accepts connections. The bridge deliberately outlives the test run -
stopping it would break the next interactive `gemini`.

The other agents read their own configuration file, so nothing has to be
started for them:

  codex   -> ~/.codex/config.toml talks to DeepSeek directly (no local bridge)
  qwen    -> provider and key from ~/.qwen/settings.json
"""

import os
import socket
import subprocess
import sys

HOME = os.path.expanduser("~")

GEMINI_PROXY_DIR = os.path.join(HOME, ".local", "share", "gem-proxy")
GEMINI_PROXY_PORT = 8000
GEMINI_PROXY_STARTER = os.path.join(GEMINI_PROXY_DIR, "start_proxy.py")

# The starter itself waits up to 15 seconds for the port; the margin here only
# covers interpreter start-up before that wait begins.
STARTER_TIMEOUT_SECONDS = 30.0


def _port_open(port, host="127.0.0.1"):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(0.3)
        return sock.connect_ex((host, port)) == 0


def gemini_proxy_available():
    return os.path.isfile(GEMINI_PROXY_STARTER)


def ensure_gemini_proxy():
    """Makes the shared Gemini bridge reachable. Returns True when it is up."""
    if _port_open(GEMINI_PROXY_PORT):
        return True
    if not gemini_proxy_available():
        return False
    try:
        subprocess.run(
            [sys.executable, GEMINI_PROXY_STARTER],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=STARTER_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired:
        return False
    return _port_open(GEMINI_PROXY_PORT)


# Environment each agent needs. Values are fixed for the local bridge; qwen
# reads its provider from the caller instead, because the endpoint is not
# implied by a local port.
FIXED_ENV = {
    "gemini": {
        "GOOGLE_GEMINI_BASE_URL": "http://127.0.0.1:%d" % GEMINI_PROXY_PORT,
        "GEMINI_API_KEY": "dummy",
    },
}

# Credentials copied from the caller when present. codex names its provider
# because the model is not implied by a local port; qwen reads the same values
# from ~/.qwen/settings.json, so those overrides are optional.
PASSTHROUGH_ENV = {
    "codex": ("DEEPSEEK_API_KEY",),
    "qwen": ("OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_MODEL"),
}

REQUIRED_ENV = {}


def environment_for(agent):
    env = dict(FIXED_ENV.get(agent, {}))
    for name in PASSTHROUGH_ENV.get(agent, ()):
        if os.environ.get(name):
            env[name] = os.environ[name]
    return env


def missing_environment(agent):
    return [name for name in REQUIRED_ENV.get(agent, ()) if not os.environ.get(name)]


def prepare(agent):
    """Brings up what the agent needs. Returns (env, reason); reason is None on success."""
    missing = missing_environment(agent)
    if missing:
        return {}, "缺少环境变量 %s（export 后重跑）" % ", ".join(missing)

    if agent == "gemini":
        if not ensure_gemini_proxy():
            return {}, "Gemini 本地桥未就绪（%s 或端口 %d 不可用）" % (
                GEMINI_PROXY_STARTER,
                GEMINI_PROXY_PORT,
            )

    return environment_for(agent), None
