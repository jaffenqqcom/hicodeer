"""JSON-RPC client for the herdr agent API embedded in hicodeerd.

Wire format (see herdr/src/api/server.rs:729-784 and :980-1010):

    request  -> one line of JSON, terminated by "\\n":
                {"id":"1","method":"agent.sessions","params":{}}
    response -> one line of JSON, terminated by "\\n":
                {"id":"1","result":{"type":"agent_sessions","sessions":[...]}}
                {"id":"1","error":{"code":"unknown_method","message":"..."}}

One request per connection. The panel does the same (crates/agent_panel/src/client.rs:6),
and the server reads exactly one initial request line per accepted stream, so this
module opens a fresh socket for every call.

Socket path: `agent_socket_path()` in
crates/gpui_ohos/depend/cmd-agent/hicodeerd/src/main.rs:441-449 builds it as
    $XDG_CONFIG_HOME (default /data/storage/el2/base/haps/entry/files)
      / .hicodeer / agent / agent.sock
and the daemon exports the resolved value as HICODEER_AGENT_SOCKET_PATH
(main.rs:278), which is honoured here as a default.

A fresh connection per call also keeps a slow `agent.start` from stalling an
unrelated assertion.

Standard library only, by requirement: socket / json / os / argparse / subprocess.
"""

import argparse
import json
import os
import socket
import time

DEFAULT_SOCKET_PATH = "/data/storage/el2/base/haps/entry/files/.hicodeer/agent/agent.sock"
SOCKET_PATH_ENV = "HICODEER_AGENT_SOCKET_PATH"

# The ets side records the resolved data root here, and launch_app.rs:83 feeds that
# same value to paths::set_custom_data_dir. Every path below hangs off it.
DEFAULT_FILES_ROOT = "/data/storage/el2/base/haps/entry/files"
DATA_DIR_RECORD_FILE = "custom_data_dir"

# Read timeout for one in-memory answer; the panel uses 3s (client.rs:23) but a
# test on a loaded device deserves a little slack.
DEFAULT_TIMEOUT = 15.0
CONNECT_TIMEOUT = 3.0
MAX_RESPONSE_BYTES = 32 * 1024 * 1024

EXIT_PASS = 0
EXIT_FAIL = 1
EXIT_ENV = 2


class EnvironmentNotMet(Exception):
    """The environment cannot answer this question at all (exit code 2).

    Raised for a missing socket, a refused connection, a method the running
    daemon does not know, or a request the store layout makes impossible.
    """


class RpcError(Exception):
    """The daemon answered with an error body or an unexpected shape (exit code 1)."""

    def __init__(self, method, code, message, raw=""):
        self.method = method
        self.code = code
        self.message = message
        self.raw = raw
        super().__init__("%s failed: %s (%s)" % (method, message, code))


class TransportError(EnvironmentNotMet):
    """Socket level failure: no endpoint, refused, or no answer in time."""


def default_socket_path():
    return os.environ.get(SOCKET_PATH_ENV) or DEFAULT_SOCKET_PATH


def resolve_data_dir(files_root=DEFAULT_FILES_ROOT):
    """Return the app's data root, read from the record launch_app.rs writes.

    Falls back to `<files_root>/.hicodeer` so a caller on a device without the
    record still gets a plausible path instead of a crash.
    """
    record = os.path.join(files_root, DATA_DIR_RECORD_FILE)
    try:
        with open(record, "r", encoding="utf-8") as handle:
            value = handle.read().strip()
    except OSError:
        value = ""
    if value:
        return value
    return os.path.join(files_root, ".hicodeer")


class AgentRpc(object):
    """Thin JSON-RPC client. One connection per call, like the panel."""

    def __init__(self, socket_path=None, timeout=DEFAULT_TIMEOUT):
        self.socket_path = socket_path or default_socket_path()
        self.timeout = timeout
        self._next_id = 0
        self.last_raw = ""

    # -- transport ---------------------------------------------------------

    def _connect(self):
        if not os.path.exists(self.socket_path):
            raise TransportError(
                "socket not found: %s (is hicodeerd running? see README)" % self.socket_path
            )
        client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        client.settimeout(CONNECT_TIMEOUT)
        try:
            client.connect(self.socket_path)
        except OSError as error:
            client.close()
            raise TransportError("cannot connect to %s: %s" % (self.socket_path, error))
        return client

    def _read_line(self, client, deadline):
        chunks = []
        total = 0
        while True:
            remaining = deadline - time.time()
            if remaining <= 0:
                raise TransportError(
                    "no response from %s within %.1fs" % (self.socket_path, self.timeout)
                )
            client.settimeout(remaining)
            try:
                chunk = client.recv(65536)
            except socket.timeout:
                raise TransportError(
                    "no response from %s within %.1fs" % (self.socket_path, self.timeout)
                )
            except OSError as error:
                raise TransportError("read from %s failed: %s" % (self.socket_path, error))
            if not chunk:
                # The daemon closed the stream without a complete line.
                raise TransportError(
                    "connection closed before a full response line arrived (%d bytes)"
                    % total
                )
            newline = chunk.find(b"\n")
            if newline >= 0:
                chunks.append(chunk[:newline])
                return b"".join(chunks).decode("utf-8", "replace")
            chunks.append(chunk)
            total += len(chunk)
            if total > MAX_RESPONSE_BYTES:
                raise TransportError("response exceeded %d bytes" % MAX_RESPONSE_BYTES)

    def call_raw(self, method, params=None, timeout=None):
        """Send one request; return (request, response, raw_text).

        `response` is the decoded JSON envelope, so callers can inspect `error`
        themselves. Raises TransportError when the socket cannot answer at all.
        """
        self._next_id += 1
        request = {"id": str(self._next_id), "method": method, "params": params or {}}
        payload = (json.dumps(request, ensure_ascii=False) + "\n").encode("utf-8")
        budget = self.timeout if timeout is None else timeout
        deadline = time.time() + budget

        client = self._connect()
        try:
            client.settimeout(budget)
            client.sendall(payload)
            raw = self._read_line(client, deadline)
        finally:
            client.close()

        self.last_raw = raw
        try:
            response = json.loads(raw)
        except ValueError as error:
            raise RpcError(method, "bad_json", "response is not JSON: %s" % error, raw)
        if not isinstance(response, dict):
            raise RpcError(method, "bad_response", "response is not a JSON object", raw)
        return request, response, raw

    def call(self, method, params=None, timeout=None):
        """Send one request and return the `result` object, raising on error bodies."""
        _, response, raw = self.call_raw(method, params, timeout)
        error = response.get("error")
        if error:
            code = error.get("code", "unknown")
            message = error.get("message", "")
            if code == "unknown_method":
                raise EnvironmentNotMet(
                    "%s is not implemented by the running daemon: %s" % (method, message)
                )
            raise RpcError(method, code, message, raw)
        result = response.get("result")
        if not isinstance(result, dict):
            raise RpcError(method, "bad_result", "response carried no result object", raw)
        return result

    # -- read-only helpers -------------------------------------------------

    def ping(self):
        """`ping` -> {"type":"pong","version":...,"protocol":...}."""
        return self.call("ping")

    def sessions(self):
        """`agent.sessions` -> list of {agent, session_id, title?, workspace_root?,
        updated_at?, source_path, workspace_id?} (herdr/src/api/schema/response.rs:133)."""
        result = self.call("agent.sessions")
        require_type(result, "agent_sessions", "agent.sessions")
        sessions = result.get("sessions")
        if not isinstance(sessions, list):
            raise RpcError("agent.sessions", "bad_result", "sessions is not a list", self.last_raw)
        return sessions

    def agents(self):
        """`agent.list` -> list of AgentInfo (schema/agents.rs:190)."""
        result = self.call("agent.list")
        require_type(result, "agent_list", "agent.list")
        agents = result.get("agents")
        if not isinstance(agents, list):
            raise RpcError("agent.list", "bad_result", "agents is not a list", self.last_raw)
        return agents

    def panes(self):
        """`pane.list` -> list of PaneInfo (schema/panes.rs:449)."""
        result = self.call("pane.list", {})
        require_type(result, "pane_list", "pane.list")
        return result.get("panes") or []

    def workspaces(self):
        """`workspace.list` -> list of WorkspaceInfo."""
        result = self.call("workspace.list")
        require_type(result, "workspace_list", "workspace.list")
        return result.get("workspaces") or []

    # -- mutating helpers --------------------------------------------------

    def rename(self, target, name):
        """`agent.rename` — renames the *pane's agent*, not a history row.

        Params: {"target": <pane id or agent name>, "name": <slug>|null}.
        `name` must match ^[a-z][a-z0-9_-]{0,31}$ (herdr/src/app/agents.rs:53-58);
        null clears the name. Errors: invalid_agent_name, agent_name_taken,
        agent_not_found, agent_launch_pending, plus target resolution errors
        (agents.rs:423-461). Returns the updated AgentInfo:
        {"type":"agent_info","agent":{...}}.
        """
        result = self.call("agent.rename", {"target": target, "name": name})
        require_type(result, "agent_info", "agent.rename")
        return result.get("agent") or {}

    def start_agent(
        self,
        kind,
        pane_id,
        args=None,
        name=None,
        timeout_ms=30000,
        resume_session=None,
        extra=None,
    ):
        """`agent.start` — launches an agent inside an existing shell pane.

        Params: {"kind","pane_id","args":[...],"name"?,"timeout_ms"?,
        "resume_session"?} (schema/agents.rs:166-183). `timeout_ms` must be
        >3000 and <=300000 (agents.rs:11-12).

        `resume_session` is the resume channel: the server builds the per-agent
        argv tail itself (`resume_args_for` -> `agent_resume::plan`,
        agents.rs:568-590), prefixes `interactive_agent_executable`, and returns
        the whole thing as `argv`. Passing `args` together with
        `resume_session` is rejected with `invalid_agent_argument` — the two
        forms are mutually exclusive (agents.rs:300-310).

        `extra` merges any further fields into the params, so this module does
        not need a patch when the request grows another one.

        Errors: unsupported_agent_kind, invalid_agent_name, agent_name_taken,
        invalid_agent_argument (unencodable args, unusable session id, or both
        fields sent), resume_unsupported (no known resume command),
        invalid_agent_timeout, agent_pane_not_found, agent_pane_busy,
        agent_pane_unavailable, agent_start_input_failed
        (agents.rs:344-388).

        Returns {"type":"agent_started","agent":{...},"argv":[...]} where argv
        already includes the executable and the resume flags.
        """
        params = {"kind": kind, "pane_id": pane_id, "args": list(args or [])}
        if name:
            params["name"] = name
        if timeout_ms is not None:
            params["timeout_ms"] = int(timeout_ms)
        if resume_session:
            params["resume_session"] = resume_session
        for key, value in (extra or {}).items():
            params[key] = value
        # The server waits inside this call, so give the socket more than timeout_ms.
        budget = max(self.timeout, (timeout_ms or 0) / 1000.0 + 15.0)
        result = self.call("agent.start", params, timeout=budget)
        require_type(result, "agent_started", "agent.start")
        return result

    def create_workspace(self, cwd=None, focus=True, reuse_root=False, label=None):
        """`workspace.create` — params per schema/workspaces.rs:8-28.

        `reuse_root=true` is the history-session path: an open space for that
        directory gets a new tab instead of a second space.
        Returns {"type":"workspace_created","workspace":{...},"tab":{...},"root_pane":{...}}.
        """
        params = {"focus": bool(focus), "reuse_root": bool(reuse_root)}
        if cwd:
            params["cwd"] = cwd
        if label:
            params["label"] = label
        result = self.call("workspace.create", params)
        require_type(result, "workspace_created", "workspace.create")
        return result

    def close_workspace(self, workspace_id, close_group=False):
        """`workspace.close` — params {"workspace_id","close_group"?}."""
        return self.call(
            "workspace.close",
            {"workspace_id": workspace_id, "close_group": bool(close_group)},
        )

    def delete_session(self, agent, source_path):
        """`agent.session.delete` — removes one session through its own store.

        Params: {"agent": <label>, "source_path": <path>} (schema/common.rs).
        The daemon routes the request to the store that produced the row, and
        that store checks the path against its own root before removing
        anything, so every agent with a `delete_session` can be deleted this
        way (whether it keeps a `.jsonl` transcript, JSON documents or SQLite
        rows). Returns {"type":"agent_session_deleted"}.
        """
        result = self.call(
            "agent.session.delete", {"agent": agent, "source_path": source_path}
        )
        require_type(result, "agent_session_deleted", "agent.session.delete")
        return result

    def resume(self, agent, session_id, workspace_root, timeout_ms=30000, extra=None):
        """Resume one history session exactly the way the panel does.

        Sequence copied from crates/agent_panel/src/client.rs:185-262:
          1. workspace.create {cwd, focus:true, reuse_root:true}
          2. agent.start {kind, pane_id, resume_session, timeout_ms}

        The resume argv is the server's, not the caller's: `agent.start` maps
        `resume_session` through `resume_args_for` (herdr/src/app/agents.rs:568-590)
        and returns the full argv. `extra` merges further fields into the params.

        Returns {"workspace_id","pane_id","workspace_result","start_result"}.
        """
        created = self.create_workspace(cwd=workspace_root, focus=True, reuse_root=True)
        workspace_id = (created.get("workspace") or {}).get("workspace_id")
        pane_id = (created.get("root_pane") or {}).get("pane_id")
        if not workspace_id or not pane_id:
            raise RpcError(
                "workspace.create",
                "bad_result",
                "response carried no workspace_id or root_pane.pane_id",
                self.last_raw,
            )
        started = self.start_agent(
            kind=agent,
            pane_id=pane_id,
            timeout_ms=timeout_ms,
            resume_session=session_id,
            extra=extra,
        )
        return {
            "workspace_id": workspace_id,
            "pane_id": pane_id,
            "workspace_result": created,
            "start_result": started,
        }


def require_type(result, expected, method):
    actual = result.get("type")
    if actual != expected:
        raise RpcError(
            method,
            "unexpected_result_type",
            "expected result type %r, got %r" % (expected, actual),
            json.dumps(result, ensure_ascii=False)[:2000],
        )


class HistoryState(object):
    """The panel's own history store: `<data_dir>/agent/history.json`.

    There is no RPC for pin / archive / history-title rename. The daemon only
    collects sessions; everything user-visible is kept client side
    (crates/agent_panel/src/session_service.rs:1-6, :106-124) in this file, whose
    location is crates/agent_panel/src/agent_panel.rs:395.

    Shape (session_service.rs:35-50):
        {"version": N,
         "sessions": {"<agent>:<session_id>": {"pinned": bool,
                                               "archived": bool,
                                               "title_override": "<text>"}}}

    Consequences this module is honest about:
      * writes here do not reach the running panel's memory (it loads once at
        startup, session_service.rs:98), so assertions are file-level;
      * the panel saves the whole file on its next pin/archive/rename, which can
        overwrite an edit made here while it runs.
    """

    STATE_FILE_NAME = "history.json"
    SUBDIR = "agent"

    def __init__(self, data_dir=None):
        self.data_dir = data_dir or resolve_data_dir()
        self.path = os.path.join(self.data_dir, self.SUBDIR, self.STATE_FILE_NAME)

    # -- raw file ----------------------------------------------------------

    def exists(self):
        return os.path.exists(self.path)

    def read_raw(self):
        """Original bytes as text, or None when the file does not exist yet."""
        try:
            with open(self.path, "r", encoding="utf-8") as handle:
                return handle.read()
        except FileNotFoundError:
            return None

    def restore_raw(self, text):
        """Put the file back exactly as it was: rewrite the text, or delete it."""
        if text is None:
            try:
                os.unlink(self.path)
            except FileNotFoundError:
                pass
            return
        self._write(text)

    def _write(self, text):
        parent = os.path.dirname(self.path)
        if parent:
            os.makedirs(parent, exist_ok=True)
        temporary = "%s.%d.tmp" % (self.path, os.getpid())
        with open(temporary, "w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, self.path)

    # -- typed state -------------------------------------------------------

    def load(self):
        raw = self.read_raw()
        if not raw:
            return {"version": 0, "sessions": {}}
        try:
            state = json.loads(raw)
        except ValueError as error:
            raise EnvironmentNotMet("%s is not valid JSON: %s" % (self.path, error))
        if not isinstance(state, dict):
            raise EnvironmentNotMet("%s holds %r, expected an object" % (self.path, type(state)))
        if not isinstance(state.get("sessions"), dict):
            state["sessions"] = {}
        if "version" not in state:
            state["version"] = 0
        return state

    @staticmethod
    def key(agent, session_id):
        return "%s:%s" % (agent, session_id)

    def entry(self, agent, session_id):
        return self.load()["sessions"].get(self.key(agent, session_id))

    def set_entry(self, agent, session_id, pinned=None, archived=None, title=None):
        """Update the fields given, leaving the rest untouched."""
        state = self.load()
        entry = state["sessions"].setdefault(self.key(agent, session_id), {})
        if pinned is not None:
            entry["pinned"] = bool(pinned)
        if archived is not None:
            entry["archived"] = bool(archived)
        if title is not None:
            entry["title_override"] = title
        self._write(json.dumps(state, indent=2, ensure_ascii=False) + "\n")
        return entry

    def forget_entry(self, agent, session_id):
        state = self.load()
        if state["sessions"].pop(self.key(agent, session_id), None) is not None:
            self._write(json.dumps(state, indent=2, ensure_ascii=False) + "\n")


class Harness(object):
    """Collects assertions, prints 期望 vs 实际, and owns the exit code."""

    def __init__(self, title):
        self.title = title
        self.checks = 0
        self.failures = []
        self.warnings = []

    def info(self, message):
        print(message)

    def warn(self, message):
        self.warnings.append(message)
        print("WARN  " + message)

    def ok(self, label, detail=""):
        self.checks += 1
        print("ok    %s%s" % (label, (" — " + detail) if detail else ""))

    def expect(self, condition, label, expected, actual):
        self.checks += 1
        if condition:
            print("ok    %s" % label)
            return True
        self.failures.append((label, expected, actual))
        print("FAIL  %s" % label)
        print("      期望: %s" % expected)
        print("      实际: %s" % actual)
        return False

    def equal(self, label, expected, actual):
        return self.expect(actual == expected, label, repr(expected), repr(actual))

    def non_empty(self, label, value):
        return self.expect(
            isinstance(value, str) and value.strip() != "",
            label,
            "非空字符串",
            repr(value),
        )

    def fail(self, label, expected, actual):
        return self.expect(False, label, expected, actual)

    def finish(self, environment_message=None, known_failure=None):
        print("")
        print("%s: %d 项检查，%d 项失败，%d 项告警" % (
            self.title, self.checks, len(self.failures), len(self.warnings)))
        if environment_message:
            print("ENV   " + environment_message)
        # Real assertion failures are the more actionable signal, so they win over
        # an environment problem noticed afterwards.
        if self.failures:
            if known_failure:
                print("已知失败：%s" % known_failure)
                print("退出码 0（按预期失败记录，--expect-known-failure）")
                return EXIT_PASS
            print("退出码 1（断言失败）")
            return EXIT_FAIL
        if environment_message:
            print("退出码 2（环境不满足）")
            return EXIT_ENV
        print("退出码 0（全通过）")
        return EXIT_PASS


def run_guarded(title, body, add_arguments=None):
    """Run `body(harness, args)` translating exceptions into exit codes.

    `add_arguments(parser)` lets each script register its own flags.
    """
    parser = build_common_parser(title)
    if add_arguments is not None:
        add_arguments(parser)
    args = parser.parse_args()
    harness = Harness(title)
    try:
        return body(harness, args)
    except EnvironmentNotMet as error:
        return harness.finish(environment_message=str(error))
    except RpcError as error:
        harness.fail("%s 返回错误" % error.method, "success", "%s: %s" % (error.code, error.message))
        if error.raw:
            print("      原始回包: %s" % error.raw)
        return harness.finish()
    except KeyboardInterrupt:
        print("中断")
        return EXIT_FAIL


def build_common_parser(title):
    parser = argparse.ArgumentParser(description=title)
    parser.add_argument(
        "--socket",
        default=None,
        help="agent.sock 路径，默认取 $%s 或 %s" % (SOCKET_PATH_ENV, DEFAULT_SOCKET_PATH),
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=DEFAULT_TIMEOUT,
        help="单次 RPC 读超时秒数（默认 %(default)s）",
    )
    return parser


def rpc_from_args(args):
    return AgentRpc(socket_path=args.socket, timeout=args.timeout)


def print_raw(label, raw):
    if raw:
        print("%s 原始回包: %s" % (label, raw))
