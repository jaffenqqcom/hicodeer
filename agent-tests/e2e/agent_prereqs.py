"""Local services and environment variables an agent needs before it can run.

Some agents cannot complete a turn on this machine without a local bridge:

  gemini  -> gem-proxy on 127.0.0.1:8000 (Gemini API shape -> DeepSeek)
  codex   -> codex-relay on 127.0.0.1:4446 (Responses -> chat completions)
  qwen    -> model provider credentials from the caller's environment

The services are started on demand, reused when something already listens on
their port, and stopped again when the test process exits. That keeps a second
run from colliding with the first one's leftovers.
"""

import atexit
import os
import signal
import socket
import subprocess
import time

HOME = os.path.expanduser("~")

GEMINI_PROXY_DIR = os.path.join(HOME, "tmp", "gem-proxy")
GEMINI_PROXY_PORT = 8000
# The proxy maps Gemini model names onto the upstream provider's models.
GEMINI_PROXY_MODEL = "gemini-2.5-pro"

CODEX_RELAY_BINARY = os.path.join(HOME, ".cargo", "bin", "codex-relay")
CODEX_RELAY_PORT = 4446
CODEX_RELAY_UPSTREAM = "https://api.deepseek.com/v1"

# Polling budget for a freshly started service. Both bridges bind immediately;
# the wait only covers interpreter start-up.
SERVICE_START_SECONDS = 20.0
SERVICE_POLL_SECONDS = 0.2

# Processes this run started. Services that were already listening are not
# added here, so the script never kills something it did not start.
_STARTED = []


def _port_open(port, host="127.0.0.1"):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.settimeout(0.3)
        return sock.connect_ex((host, port)) == 0


def _wait_for_port(port, timeout=SERVICE_START_SECONDS):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if _port_open(port):
            return True
        time.sleep(SERVICE_POLL_SECONDS)
    return False


def _spawn(command, cwd=None, env=None):
    # start_new_session puts the service in its own process group so the
    # cleanup below can take the whole group down, including any helper it
    # forked.
    process = subprocess.Popen(
        command,
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    _STARTED.append(process)
    return process


def _stop(process):
    if process.poll() is not None:
        return
    try:
        os.killpg(os.getpgid(process.pid), signal.SIGTERM)
    except OSError:
        process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(os.getpgid(process.pid), signal.SIGKILL)
        except OSError:
            process.kill()


def cleanup():
    """Stops every service this process started. Safe to call twice."""
    while _STARTED:
        _stop(_STARTED.pop())


atexit.register(cleanup)


def gemini_proxy_available():
    return os.path.isfile(
        os.path.join(GEMINI_PROXY_DIR, "gemini_proxy_for_kimi.py")
    ) and os.path.isfile(os.path.join(GEMINI_PROXY_DIR, ".venv", "bin", "python"))


def ensure_gemini_proxy():
    """Makes the Gemini bridge reachable. Returns True when it is up."""
    if _port_open(GEMINI_PROXY_PORT):
        return True
    if not gemini_proxy_available():
        return False
    _spawn(
        [
            os.path.join(GEMINI_PROXY_DIR, ".venv", "bin", "python"),
            os.path.join(GEMINI_PROXY_DIR, "gemini_proxy_for_kimi.py"),
        ],
        cwd=GEMINI_PROXY_DIR,
    )
    return _wait_for_port(GEMINI_PROXY_PORT)


def codex_relay_available():
    return os.path.isfile(CODEX_RELAY_BINARY) and bool(os.environ.get("DEEPSEEK_API_KEY"))


def ensure_codex_relay():
    """Makes the Codex bridge reachable. Returns True when it is up."""
    if _port_open(CODEX_RELAY_PORT):
        return True
    if not codex_relay_available():
        return False
    _spawn(
        [
            CODEX_RELAY_BINARY,
            "--port",
            str(CODEX_RELAY_PORT),
            "--upstream",
            CODEX_RELAY_UPSTREAM,
            "--api-key",
            os.environ["DEEPSEEK_API_KEY"],
        ]
    )
    return _wait_for_port(CODEX_RELAY_PORT)


# Environment each agent needs. Values are fixed for the local bridges; qwen
# reads its provider from the caller instead, because the endpoint is not
# implied by a local port.
FIXED_ENV = {
    "gemini": {
        "GOOGLE_GEMINI_BASE_URL": "http://127.0.0.1:%d" % GEMINI_PROXY_PORT,
        "GEMINI_API_KEY": "dummy",
    },
}

PASSTHROUGH_ENV = {
    "qwen": ("OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_MODEL"),
}

REQUIRED_ENV = {
    "qwen": ("OPENAI_API_KEY",),
}


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
                GEMINI_PROXY_DIR,
                GEMINI_PROXY_PORT,
            )
    if agent == "codex":
        if not ensure_codex_relay():
            return {}, "Codex 本地桥未就绪（%s 或 DEEPSEEK_API_KEY 缺失）" % CODEX_RELAY_BINARY

    return environment_for(agent), None
