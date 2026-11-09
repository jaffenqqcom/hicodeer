#!/usr/bin/env python3
"""hook 集成的完整生命周期：安装 → 运行 → 卸载，三段都验。

`test_hooks.py` 验的是「装好之后能不能上报」；本脚本补的是两头：
装的时候有没有把文件和配置写对，卸的时候有没有清干净。卸载残留是本轮修过的
缺陷（claude 的 `PreToolUse(blocked)` / `Notification` 漏删、kimi 的旧品牌块
认不出），所以这里是关键断言。

每个 target 的流程：

  1. `integration uninstall <t>`，断言 hook 文件消失、配置文件里不再出现 `agent-state`。
  2. `integration install <t>`，断言 hook 文件回来、版本号等于 `integration status`
     报的版本、脚本里带 `hicodeer:<t>` 前缀。
  3. 可选 `--run`：起一个 TUI、发一轮提示，断言上报过 `working`、收尾回 `idle`、
     `agent_session.source` 是 `hicodeer:<t>`。
  4. 收尾把状态还原成跑之前的样子（原来装了装回去，原来没装卸掉）。

默认只跑第 1、2、4 步（快，几秒一个 target）；`--run` 才做第 3 步（每个 target
要一分钟上下，默认只对 deveco / atomcode / codewhale / zcode 跑，与 test_hooks 对齐）。
"""

import argparse
import os
import re
import subprocess
import sys
import time

import agent_rpc
import test_hooks
from agent_rpc import EXIT_ENV, EXIT_FAIL, EXIT_PASS, Harness, run_guarded

CLI = "hicodeerd"
CLI_TIMEOUT = 60
COMMAND_TIMEOUT = 20

VERSION_PATTERN = re.compile(r"INTEGRATION_VERSION=(\d+)")
SOURCE_PATTERN = re.compile(r"hicodeer:([a-z0-9_-]+)")

# Config files each target registers the hook in, relative to $HOME.
# A target with an empty list keeps its registration somewhere the daemon owns
# (opencode / deveco write into the sandbox data root); those are reported as a
# warning instead of a silent pass.
HOOK_CONFIG_FILES = {
    "claude": [".claude/settings.json"],
    "codex": [".codex/hooks.json"],
    "kimi": [".kimi-code/config.toml"],
    "qwen": [".qwen/settings.json"],
    "codebuddy": [".codebuddy/settings.json"],
    "atomcode": [".atomcode/hooks.json"],
    "codewhale": [".codewhale/config.toml"],
    "zcode": [".zcode/cli/setting.json"],
    "hermes": [".hermes/config.yaml"],
    "pi": [],
    "opencode": [],
    "deveco": [],
}

# Targets the run stage can launch.
RUNNABLE_AGENTS = ["deveco", "atomcode", "codewhale", "zcode"]

DEFAULT_PROMPT = "用 bash 执行 echo hi，然后只回复两个字：成功"
PROMPT_POLL_SECONDS = 90.0
POLL_INTERVAL = 0.5
# How long a fresh pane may answer agent_pane_busy before it counts as a failure.
SHELL_READY_WAIT_SECONDS = 20.0
# Window for answering the start-up prompts a TUI shows before its input box.
STARTUP_PROMPT_SECONDS = 30.0
# Some TUIs drop text typed before their first frame is painted: zcode leaves
# the prompt box empty, and the retried Enter then lands nowhere. deveco is
# slower when a full sweep starts it right after another target. The values are
# what the run stage was measured to need on this device.
STARTUP_SETTLE_SECONDS = {"zcode": 8.0, "deveco": 10.0}


def run_cli(arguments, timeout=CLI_TIMEOUT):
    try:
        completed = subprocess.run(
            [CLI] + arguments, capture_output=True, text=True, timeout=timeout
        )
        return completed.returncode, completed.stdout, completed.stderr
    except FileNotFoundError:
        raise agent_rpc.EnvironmentNotMet("找不到 %s，先装包或把它放进 PATH" % CLI)
    except subprocess.TimeoutExpired:
        return -1, "", "timeout after %ss" % timeout


def parse_status():
    """Return {target: {"state", "version", "path"}} from `integration status`."""
    code, stdout, stderr = run_cli(["integration", "status"])
    if code != 0:
        raise agent_rpc.EnvironmentNotMet(
            "integration status 失败（退出码 %s）：%s" % (code, (stderr or stdout).strip()[:200])
        )
    result = {}
    pattern = re.compile(r"^([a-z0-9_-]+)(?:\s+\(experimental\))?: ([a-z ]+?)(?: \(v(\d+)\))? \(([^)]*)\)$")
    for line in stdout.splitlines():
        match = pattern.match(line.strip())
        if not match:
            continue
        target, state, version, path = match.groups()
        result[target] = {
            "state": state.strip(),
            "version": int(version) if version else None,
            "path": path,
        }
    if not result:
        raise agent_rpc.EnvironmentNotMet("integration status 没解析出任何目标，输出格式变了？")
    return result


def hook_version(path):
    try:
        with open(path, "r", encoding="utf-8", errors="ignore") as handle:
            text = handle.read()
    except OSError:
        return None
    match = VERSION_PATTERN.search(text)
    return int(match.group(1)) if match else None


def hook_source(path):
    try:
        with open(path, "r", encoding="utf-8", errors="ignore") as handle:
            text = handle.read()
    except OSError:
        return None
    for match in SOURCE_PATTERN.finditer(text):
        return match.group(1)
    return None


def config_residue(target):
    """Count `agent-state` mentions across the target's config files."""
    files = HOOK_CONFIG_FILES.get(target, [])
    if not files:
        return None
    home = os.path.expanduser("~")
    total = 0
    missing = []
    for relative in files:
        path = os.path.join(home, relative)
        if not os.path.isfile(path):
            missing.append(relative)
            continue
        with open(path, "r", encoding="utf-8", errors="ignore") as handle:
            total += handle.read().count("agent-state")
    return total, missing


def uninstall(harness, target):
    code, stdout, stderr = run_cli(["integration", "uninstall", target])
    harness.expect(
        code == 0,
        "%s uninstall 退出码" % target,
        "0",
        "%s；%s" % (code, (stderr or stdout).strip().splitlines()[-1][:120] if (stderr or stdout).strip() else ""),
    )
    return code == 0


def install(harness, target):
    code, stdout, stderr = run_cli(["integration", "install", target])
    harness.expect(
        code == 0,
        "%s install 退出码" % target,
        "0",
        "%s；%s" % (code, (stderr or stdout).strip().splitlines()[-1][:120] if (stderr or stdout).strip() else ""),
    )
    return code == 0


def check_removed(harness, target, path):
    harness.expect(
        not os.path.exists(path),
        "%s 卸载后 hook 文件已删除" % target,
        "不存在",
        path,
    )
    residue = config_residue(target)
    if residue is None:
        harness.warn("%s 的配置注册位置由 daemon 持有，只校验了 hook 文件" % target)
        return
    count, missing = residue
    if missing:
        harness.warn("%s 的配置文件不存在，跳过配置残留检查：%s" % (target, ", ".join(missing)))
        return
    harness.expect(
        count == 0,
        "%s 卸载后配置无残留" % target,
        "0 处 agent-state",
        "%d 处" % count,
    )


def check_installed(harness, target, path, expected_version):
    harness.expect(os.path.isfile(path), "%s 安装后 hook 文件存在" % target, "存在", path)
    if not os.path.isfile(path):
        return
    version = hook_version(path)
    harness.expect(
        version == expected_version,
        "%s 安装后 hook 版本号" % target,
        str(expected_version),
        str(version),
    )
    source = hook_source(path)
    harness.expect(
        source == target,
        "%s 安装后上报前缀" % target,
        "hicodeer:%s" % target,
        "hicodeer:%s" % source if source else "未找到",
    )


def wait_for_agent(rpc, pane_id, target, timeout=60.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        agents = rpc.agents()
        for agent in agents:
            if agent.get("pane_id") == pane_id and agent.get("agent") == target:
                return agent
        time.sleep(POLL_INTERVAL)
    return None


def start_agent_when_shell_ready(rpc, target, pane_id):
    """Start the agent, retrying until the new pane hands over a usable shell.

    A freshly created space's root pane answers `agent_pane_busy` until its
    shell is up. That is start-up ordering, not a defect in the target.
    """
    deadline = time.time() + SHELL_READY_WAIT_SECONDS
    # The name is unique per run: re-running the script must not collide with a
    # pane an earlier `--keep` run left behind (`agent_name_taken`).
    name = "lifecycle-%s-%s" % (target, time.strftime("%H%M%S"))
    while True:
        try:
            return rpc.start_agent(
                kind=target,
                pane_id=pane_id,
                name=name,
                timeout_ms=90000,
            )
        except agent_rpc.RpcError as error:
            if error.code != "agent_pane_busy" or time.time() >= deadline:
                raise
            time.sleep(1.0)


def run_once(harness, rpc, target, prompt, keep_workspace):
    """Start the TUI once, send one prompt, assert the hook reported it."""
    workspace = rpc.create_workspace(cwd=os.path.expanduser("~"), focus=False)
    pane_id = workspace["root_pane"]["pane_id"]
    try:
        started = start_agent_when_shell_ready(rpc, target, pane_id)
        harness.expect(bool(started.get("argv")), "%s 运行：agent.start 返回 argv" % target, "有 argv", str(started.get("argv"))[:120])
        agent = wait_for_agent(rpc, pane_id, target)
        harness.expect(agent is not None, "%s 运行：pane 识别出 agent" % target, target, "未识别")

        # Reuse `test_hooks`' prompt round: it answers the start-up prompts
        # (deveco asks for directory trust, then offers an update) and resubmits
        # an Enter the TUI swallowed while it was still laying out the window.
        settle = STARTUP_SETTLE_SECONDS.get(target)
        if settle:
            time.sleep(settle)
        test_hooks.dismiss_startup_prompt(rpc, pane_id, target, STARTUP_PROMPT_SECONDS)
        round_result = test_hooks.run_prompt_round(
            rpc, pane_id, prompt, PROMPT_POLL_SECONDS, agent=target
        )
        harness.expect(
            round_result["seen_working"],
            "%s 运行：上报过 working" % target,
            "出现过",
            "未出现（末态 %s）" % round_result["last_status"],
        )
        harness.expect(
            round_result["idle_after_working"],
            "%s 运行：收尾回到 idle/done" % target,
            "idle 或 done",
            round_result["last_status"] or "未收尾",
        )
        settled = next(
            (entry for entry in rpc.agents() if entry.get("pane_id") == pane_id), None
        )
        if settled:
            session = settled.get("agent_session") or {}
            harness.expect(
                session.get("source") == "hicodeer:%s" % target,
                "%s 运行：agent_session.source 来自 hook" % target,
                "hicodeer:%s" % target,
                str(session.get("source")),
            )
    finally:
        if not keep_workspace:
            try:
                rpc.close_workspace(workspace["workspace"]["workspace_id"], close_group=True)
            except (agent_rpc.RpcError, agent_rpc.EnvironmentNotMet):
                # Cleanup must not mask the failure that brought us here, and a
                # transport timeout is as plausible as a rejected request.
                pass


def add_arguments(parser):
    parser.add_argument(
        "--agents",
        default="",
        help="逗号分隔的 target 名单，默认取 integration status 里所有已装的",
    )
    parser.add_argument("--run", action="store_true", help="额外跑运行阶段（起 TUI 发一轮提示）")
    parser.add_argument(
        "--run-agents",
        default=",".join(RUNNABLE_AGENTS),
        help="--run 覆盖的 target，默认 %s" % ",".join(RUNNABLE_AGENTS),
    )
    parser.add_argument("--prompt", default=DEFAULT_PROMPT, help="运行阶段发的提示")
    parser.add_argument("--keep", action="store_true", help="运行阶段结束后不关本轮新建的 workspace")


def body(harness, args):
    status = parse_status()
    installed = [name for name, info in status.items() if info["state"] == "current"]
    if args.agents:
        targets = [name.strip() for name in args.agents.split(",") if name.strip()]
    else:
        targets = sorted(installed)
    if not targets:
        raise agent_rpc.EnvironmentNotMet("没有已安装的 hook target，先跑 integration install")

    harness.info("参与校验的 target：%s" % ", ".join(targets))
    if args.run:
        harness.info("运行阶段只覆盖：%s" % args.run_agents)

    run_targets = [name.strip() for name in args.run_agents.split(",") if name.strip()]
    rpc = agent_rpc.AgentRpc(args.socket, args.timeout) if args.run else None

    for target in targets:
        harness.info("")
        harness.info("== %s" % target)
        info = status.get(target)
        if info is None:
            harness.warn("%s 不在 integration status 里，跳过" % target)
            continue
        path = info["path"]
        original_version = info["version"]
        was_installed = info["state"] == "current"

        # 1. 卸载
        if not uninstall(harness, target):
            continue
        check_removed(harness, target, path)

        # 2. 安装
        if not install(harness, target):
            if was_installed:
                harness.fail(
                    "%s 卸载后装不回来" % target,
                    "恢复安装",
                    "停在未安装状态",
                )
            continue
        # The path and version to compare against are the ones this install just
        # reported: a target picked with `--agents` may not have been installed
        # before, and then the pre-uninstall values are empty or stale.
        fresh = parse_status().get(target, {})
        check_installed(
            harness,
            target,
            fresh.get("path") or path,
            fresh.get("version") or original_version,
        )
        after = parse_status().get(target, {})
        harness.expect(
            after.get("state") == "current",
            "%s 重装后状态" % target,
            "current",
            str(after.get("state")),
        )

        # 3. 运行
        if args.run and target in run_targets:
            run_once(harness, rpc, target, args.prompt, args.keep)

        # 4. 还原：a target that was not installed before this run must not be
        # left installed by it.
        if not was_installed:
            uninstall(harness, target)

    return harness.finish()


if __name__ == "__main__":
    raise SystemExit(run_guarded("hook 安装 / 运行 / 卸载校验", body, add_arguments))
