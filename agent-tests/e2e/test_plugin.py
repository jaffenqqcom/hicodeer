#!/usr/bin/env python3
"""plugin 的完整生命周期：安装 → 运行 → 卸载。

跑的是 daemon 的 plugin 子系统，全程只用 `hicodeerd plugin` 子命令，不碰应用的
GPUI 面板（面板只是它的客户端）。

流程：

  1. 在临时目录造一个最小合法清单 `herdr-plugin.toml`（`id` / `name` / `version` /
     `min_herdr_version` 都是必填；`min_herdr_version` 取当前二进制版本，否则会被
     `plugin_requires_newer_herdr` 拒掉）。
  2. `plugin link <dir>` → 断言返回 `plugin_linked`。
  3. `plugin list` / `plugin list --json` → 断言能看到该 plugin。
  4. `plugin action list` → 断言清单里声明的 action 被识别。
  5. `plugin action invoke <action> --plugin <id>` → **真正跑一次**：清单里那条命令
     会写一个标记文件，脚本读回来验证内容，这是「运行」的唯一硬证据。
  6. `plugin disable` / `plugin enable` → 断言 enabled 标记随之变化。
  7. `plugin unlink` → 断言 list 回到空，plugin 目录与临时目录都清掉。

临时目录放在 `$TMPDIR` 下，结束时无条件删除（`finally`）。
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

import agent_rpc
from agent_rpc import EXIT_ENV, EXIT_FAIL, EXIT_PASS, Harness, run_guarded

CLI = "hicodeerd"
CLI_TIMEOUT = 90

PLUGIN_ID = "plugin-lifecycle-test"
ACTION_ID = "write-marker"
MARKER_TEXT = "plugin-action-ok"

# `plugin action invoke` answers as soon as the action is queued: the log entry
# comes back with status "running" and flips to "succeeded" once the child exits.
ACTION_WAIT_SECONDS = 30.0
ACTION_POLL_INTERVAL = 0.3
# Numeric core of a version string; a channel suffix is dropped because the
# manifest's `min_herdr_version` only accepts three numeric segments.
VERSION_CORE_PATTERN = re.compile(r"\b(\d+\.\d+\.\d+)\b")


# Filled in from `--socket`; `run_cli` needs it on every call, and the flag is
# parsed once in `body`.
CLI_SOCKET_PATH = None


def run_cli(arguments, timeout=CLI_TIMEOUT):
    # The daemon hands its socket path to the shells it starts, so a pane finds
    # the server without guessing. This script runs outside such a shell, so it
    # passes the same path explicitly; without it the CLI falls back to a socket
    # the daemon never binds and every `plugin` call fails with
    # `server_not_running`.
    environment = dict(os.environ)
    environment[agent_rpc.SOCKET_PATH_ENV] = CLI_SOCKET_PATH or agent_rpc.default_socket_path()
    try:
        completed = subprocess.run(
            [CLI] + arguments,
            capture_output=True,
            text=True,
            timeout=timeout,
            env=environment,
        )
        return completed.returncode, completed.stdout, completed.stderr
    except FileNotFoundError:
        raise agent_rpc.EnvironmentNotMet("找不到 %s，先装包或把它放进 PATH" % CLI)
    except subprocess.TimeoutExpired:
        return -1, "", "timeout after %ss" % timeout


def binary_version():
    """Ask the binary its own version; the manifest's minimum must not exceed it.

    Only the numeric core goes into the manifest: a non-stable channel appends
    `-<channel>[.<build>]`, and the server's `Version::parse` rejects anything
    past three numeric segments.
    """
    code, stdout, stderr = run_cli(["--version"])
    if code != 0:
        raise agent_rpc.EnvironmentNotMet("hicodeerd --version 失败：%s" % (stderr or stdout).strip()[:200])
    match = VERSION_CORE_PATTERN.search(stdout)
    if match is None:
        raise agent_rpc.EnvironmentNotMet("无法从 %r 解析出版本号" % stdout.strip())
    return match.group(1)


def write_manifest(directory, version, marker_path):
    manifest = {
        "id": PLUGIN_ID,
        "name": "Plugin Lifecycle Test",
        "version": "0.0.1",
        "description": "Temporary plugin created by agent-tests/e2e/test_plugin.py",
        "min_herdr_version": version,
        "actions": [
            {
                "id": ACTION_ID,
                "title": "Write marker",
                "description": "Writes a marker file so the test can prove the action ran",
                "command": ["sh", "-c", "echo %s > %s" % (MARKER_TEXT, marker_path)],
            }
        ],
    }
    path = os.path.join(directory, "herdr-plugin.toml")
    with open(path, "w", encoding="utf-8") as handle:
        # Hand-rolled TOML keeps the file readable when the test fails and someone opens it.
        handle.write('id = "%s"\n' % manifest["id"])
        handle.write('name = "%s"\n' % manifest["name"])
        handle.write('version = "%s"\n' % manifest["version"])
        handle.write('description = "%s"\n' % manifest["description"])
        handle.write('min_herdr_version = "%s"\n' % manifest["min_herdr_version"])
        handle.write("\n[[actions]]\n")
        action = manifest["actions"][0]
        handle.write('id = "%s"\n' % action["id"])
        handle.write('title = "%s"\n' % action["title"])
        handle.write('description = "%s"\n' % action["description"])
        handle.write("command = [%s]\n" % ", ".join('"%s"' % part for part in action["command"]))
    return path


def plugin_entries():
    """Return the parsed `plugin list --json` plugin array."""
    code, stdout, stderr = run_cli(["plugin", "list", "--json"])
    if code != 0:
        raise agent_rpc.EnvironmentNotMet(
            "plugin list --json 失败（%s）：%s" % (code, (stderr or stdout).strip()[:200])
        )
    try:
        payload = json.loads(stdout)
    except ValueError:
        raise agent_rpc.RpcError("plugin list --json", "invalid_json", stdout[:200])
    result = payload.get("result") or payload
    return result.get("plugins") or []


def find_entry(entries, plugin_id):
    for entry in entries:
        if entry.get("plugin_id") == plugin_id or entry.get("id") == plugin_id:
            return entry
    return None


def latest_action_log(plugin_id):
    """Return the newest action log entry for a plugin, or None."""
    code, stdout, stderr = run_cli(["plugin", "log", "list", "--plugin", plugin_id, "--limit", "1"])
    if code != 0:
        return None
    try:
        payload = json.loads(stdout)
    except ValueError:
        return None
    logs = ((payload.get("result") or {}).get("logs")) or []
    return logs[0] if logs else None


def add_arguments(parser):
    parser.add_argument("--keep", action="store_true", help="保留临时插件目录，便于排查")


def body(harness, args):
    global CLI_SOCKET_PATH
    if args.socket:
        CLI_SOCKET_PATH = args.socket
    version = binary_version()
    harness.info("二进制版本：%s" % version)

    workdir = tempfile.mkdtemp(prefix="hicodeerd-plugin-test-")
    marker_path = os.path.join(workdir, "marker.txt")
    try:
        manifest_path = write_manifest(workdir, version, marker_path)
        harness.expect(os.path.isfile(manifest_path), "清单已生成", manifest_path, "缺失")

        # 0. 前置：确保没有同名 plugin 残留
        existing = find_entry(plugin_entries(), PLUGIN_ID)
        if existing is not None:
            run_cli(["plugin", "unlink", PLUGIN_ID])
            harness.warn("发现同名 plugin 残留，已先 unlink")

        # 1. 安装
        code, stdout, stderr = run_cli(["plugin", "link", workdir])
        harness.expect(
            code == 0,
            "plugin link 退出码",
            "0",
            "%s；%s" % (code, (stderr or stdout).strip()[:200]),
        )
        harness.expect(
            "plugin_linked" in stdout,
            "plugin link 返回 plugin_linked",
            "含 plugin_linked",
            stdout.strip()[:200],
        )

        # 2. 列表
        entries = plugin_entries()
        entry = find_entry(entries, PLUGIN_ID)
        harness.expect(entry is not None, "plugin list 能看到该 plugin", PLUGIN_ID, str([e.get("plugin_id") for e in entries]))
        harness.expect(
            bool(entry and entry.get("enabled")),
            "link 之后默认 enabled",
            "true",
            str(entry.get("enabled") if entry else None),
        )

        # 3. action 识别
        code, stdout, stderr = run_cli(["plugin", "action", "list"])
        harness.expect(code == 0, "plugin action list 退出码", "0", str(code))
        harness.expect(
            ACTION_ID in stdout,
            "action list 识别出清单里的 action",
            ACTION_ID,
            stdout.strip()[:200],
        )

        # 4. 运行：真正执行一次 action，用标记文件 + 执行日志双证据
        if os.path.exists(marker_path):
            os.remove(marker_path)
        code, stdout, stderr = run_cli(
            ["plugin", "action", "invoke", ACTION_ID, "--plugin", PLUGIN_ID]
        )
        harness.expect(
            code == 0,
            "plugin action invoke 退出码",
            "0",
            "%s；%s" % (code, (stderr or stdout).strip()[:200]),
        )

        # invoke 只保证入队，子进程结束才会写标记文件并翻成 succeeded。
        deadline = time.time() + ACTION_WAIT_SECONDS
        marker = ""
        log_entry = None
        while time.time() < deadline:
            if os.path.isfile(marker_path):
                with open(marker_path, "r", encoding="utf-8", errors="ignore") as handle:
                    marker = handle.read().strip()
            log_entry = latest_action_log(PLUGIN_ID)
            if marker == MARKER_TEXT and log_entry and log_entry.get("status") == "succeeded":
                break
            time.sleep(ACTION_POLL_INTERVAL)

        harness.expect(
            marker == MARKER_TEXT,
            "action 真的执行了（标记文件内容）",
            MARKER_TEXT,
            marker or "文件不存在或为空",
        )
        harness.expect(
            bool(log_entry) and log_entry.get("status") == "succeeded",
            "action 执行日志为 succeeded",
            "succeeded",
            str((log_entry or {}).get("status")),
        )
        harness.expect(
            bool(log_entry) and log_entry.get("exit_code") == 0,
            "action 子进程退出码",
            "0",
            str((log_entry or {}).get("exit_code")),
        )

        # 5. 禁用 / 启用
        code, _, _ = run_cli(["plugin", "disable", PLUGIN_ID])
        harness.expect(code == 0, "plugin disable 退出码", "0", str(code))
        entry = find_entry(plugin_entries(), PLUGIN_ID)
        harness.expect(
            bool(entry) and entry.get("enabled") is False,
            "disable 后 enabled 为 false",
            "false",
            str(entry.get("enabled") if entry else None),
        )
        code, _, _ = run_cli(["plugin", "enable", PLUGIN_ID])
        harness.expect(code == 0, "plugin enable 退出码", "0", str(code))
        entry = find_entry(plugin_entries(), PLUGIN_ID)
        harness.expect(
            bool(entry) and entry.get("enabled") is True,
            "enable 后 enabled 为 true",
            "true",
            str(entry.get("enabled") if entry else None),
        )

        # 6. 卸载
        code, stdout, stderr = run_cli(["plugin", "unlink", PLUGIN_ID])
        harness.expect(
            code == 0,
            "plugin unlink 退出码",
            "0",
            "%s；%s" % (code, (stderr or stdout).strip()[:200]),
        )
        harness.expect(
            find_entry(plugin_entries(), PLUGIN_ID) is None,
            "unlink 后 list 里不再出现",
            "不存在",
            "仍存在",
        )

        # 7. 卸载后 action 不再可用（子命令仍可解析，但不应找到该 action）
        code, stdout, stderr = run_cli(["plugin", "action", "list"])
        harness.expect(
            code == 0,
            "unlink 后 action list 退出码",
            "0",
            "%s；%s" % (code, (stderr or stdout).strip()[:200]),
        )
        harness.expect(
            ACTION_ID not in stdout,
            "unlink 后 action 随之消失",
            "不含 %s" % ACTION_ID,
            stdout.strip()[:200],
        )
    finally:
        # A failure before step 6 leaves the plugin linked, and the registry is
        # persisted, so the next run would start from a dangling entry.
        code, stdout, stderr = run_cli(["plugin", "unlink", PLUGIN_ID])
        if code != 0 and "not found" not in (stderr or stdout).lower():
            harness.warn("清理残留 plugin 失败：%s" % (stderr or stdout).strip()[:200])
        if args.keep:
            harness.warn("保留临时目录：%s" % workdir)
        else:
            shutil.rmtree(workdir, ignore_errors=True)

    return harness.finish()


if __name__ == "__main__":
    raise SystemExit(run_guarded("plugin 安装 / 运行 / 卸载校验", body, add_arguments))
