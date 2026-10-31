#!/usr/bin/env python3
"""校验 SKILL.md 教给 agent 的每条命令，在已装二进制上真的可用。

这份文档是 `hicodeerd --skill` 的输出源（`herdr/src/integration/assets` 之外的
独立文件，由 `include_str!` 编进 CLI），用途是让别的 agent 照着它操作 HiCodeer。
文档写错一处，agent 就会在真实环境里撞墙，所以这里把「文档说的」和「二进制做的」
逐条对上。

四类断言：

  1. 文档里每条 `hicodeerd ...` 示例的子命令都真实存在（`<group> <sub> --help` 退出码 0）。
  2. 示例里出现的每个 `--flag` 都出现在对应子命令的 `--help` 输出里。
  3. 文档提到的每个命令组都能被 `hicodeerd --help` 列出，且裸组名退出码为 0、
     未知子命令退出码为 2（本轮修的行为，见 cli/*.rs 的 `args.first()` 分支）。
  4. `hicodeerd --skill` 的输出与仓库里的 SKILL.md 逐字节一致（防止改了文件忘了编译）。

只读：不建 pane、不起 agent、不装任何东西。
"""

import os
import re
import shlex
import subprocess
import sys

import agent_rpc
from agent_rpc import EXIT_ENV, EXIT_FAIL, EXIT_PASS, Harness, run_guarded

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DEFAULT_SKILL_PATH = os.path.join(REPO_ROOT, "herdr", "skills", "herdr", "SKILL.md")
DEFAULT_BINARY = "hicodeerd"

# Global options that consume a value and may precede the group name.
GLOBAL_OPTIONS_WITH_VALUE = ("--session", "--machine", "--remote")

# Groups whose bare invocation is not a help request. `update` with no argument
# runs the self-update path, so the group probe would sit there downloading.
BARE_GROUP_SKIPPED = {
    "update": "裸调用会触发 self-update，不是打印帮助",
}

# Placeholder tokens the doc uses for caller-supplied values.
PLACEHOLDER = re.compile(r"^[<\"'$]")

# A flag token: two dashes, letters/digits/dashes. Skips the bare `--` separator.
FLAG = re.compile(r"^--[a-z][a-z0-9-]*$")

COMMAND_TIMEOUT = 30


def run(binary, arguments, timeout=COMMAND_TIMEOUT):
    """Run the CLI and return (exit_code, stdout, stderr)."""
    try:
        completed = subprocess.run(
            [binary] + arguments,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        return completed.returncode, completed.stdout, completed.stderr
    except FileNotFoundError:
        raise agent_rpc.EnvironmentNotMet("找不到二进制 %s，先装包或把它放进 PATH" % binary)
    except subprocess.TimeoutExpired:
        return -1, "", "timeout after %ss" % timeout


def fenced_bash_blocks(text):
    """Yield the body of every ```bash fenced block in the document."""
    body = []
    inside = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("```"):
            if inside:
                yield "\n".join(body)
                body = []
                inside = False
            elif stripped in ("```bash", "```sh", "```"):
                inside = True
            continue
        if inside:
            body.append(line)
    if inside:
        yield "\n".join(body)


def command_lines(blocks):
    """Collect the `hicodeerd ...` command lines the document tells an agent to run."""
    lines = []
    for block in blocks:
        for raw in block.splitlines():
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if line.startswith("$ "):
                line = line[2:].strip()
            if not line.startswith("hicodeerd"):
                continue
            # Drop inline shell comments that are not part of the command.
            line = re.sub(r"\s+#\s.*$", "", line)
            lines.append(line)
    return lines


def tokenize(line):
    try:
        return shlex.split(line, posix=True)
    except ValueError:
        return line.split()


def split_target(tokens):
    """Return (group, subcommand, option_tokens) for a `hicodeerd ...` command.

    Global options come before the group name; the doc uses `--machine <label> agent list`.
    """
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token in GLOBAL_OPTIONS_WITH_VALUE:
            index += 2
            continue
        if token.startswith("--"):
            index += 1
            continue
        break
    if index >= len(tokens):
        return None, None, tokens[1:]
    group = tokens[index]
    rest = tokens[index + 1:]
    subcommand = None
    if rest and not rest[0].startswith("-") and not rest[0].startswith("<"):
        subcommand = rest[0]
        rest = rest[1:]
    return group, subcommand, rest


def documented_groups(text):
    """Groups the document tells an agent to ask for help on."""
    groups = set()
    for line in command_lines(fenced_bash_blocks(text)):
        group, subcommand, _ = split_target(tokenize(line))
        if group and not group.startswith("-"):
            groups.add(group)
    return groups


def help_groups(help_text):
    """Groups listed under `Command groups` in `hicodeerd --help`."""
    groups = []
    collecting = False
    for line in help_text.splitlines():
        stripped = line.strip()
        if stripped.startswith("Command groups"):
            collecting = True
            continue
        if collecting:
            if not stripped:
                break
            groups.append(stripped)
    return groups


def check_command(harness, binary, line):
    """Verify one documented command's subcommand and flags against its own --help."""
    tokens = tokenize(line)
    group, subcommand, rest = split_target(tokens)
    if not group or group.startswith("-"):
        # `hicodeerd --help` / `hicodeerd --skill`: top-level form, nothing to resolve.
        return

    help_arguments = [group]
    label = "hicodeerd %s" % group
    if subcommand:
        help_arguments.append(subcommand)
        label = "%s %s" % (label, subcommand)
    help_arguments.append("--help")

    code, stdout, stderr = run(binary, help_arguments)
    text = stdout + stderr
    harness.expect(
        code == 0,
        "%s --help 可用" % label,
        "退出码 0",
        "退出码 %s；输出首行 %r" % (code, (text.strip().splitlines() or [""])[0][:120]),
    )
    if code != 0:
        return

    missing = []
    for token in rest:
        if token == "--":
            break
        if not FLAG.match(token):
            continue
        # The help flags are what we asked for, not something to look up in the output.
        if token in ("--help", "--version"):
            continue
        # Word-boundary match: a plain substring test lets `--line` pass on
        # `--lines`, which defeats the point of checking each flag by name.
        if re.search(r"(?<![\w-])" + re.escape(token) + r"(?![\w-])", text) is None:
            missing.append(token)
    harness.expect(
        not missing,
        "%s 的参数都在 --help 里" % label,
        "全部存在",
        "缺失 %s" % (", ".join(sorted(set(missing))) if missing else "无"),
    )


def build_parser_options(parser):
    parser.add_argument(
        "--skill-path",
        default=DEFAULT_SKILL_PATH,
        help="SKILL.md 路径，默认仓库内 herdr/skills/herdr/SKILL.md",
    )
    parser.add_argument("--binary", default=DEFAULT_BINARY, help="hicodeerd 可执行文件，默认取 PATH")
    parser.add_argument(
        "--skip-skill-match",
        action="store_true",
        help="跳过 `--skill` 输出与文件的一致性断言（改了文档还没重新编译时用）",
    )


def body(harness, args):
    if not os.path.isfile(args.skill_path):
        raise agent_rpc.EnvironmentNotMet("找不到 SKILL.md: %s" % args.skill_path)

    with open(args.skill_path, "r", encoding="utf-8") as handle:
        text = handle.read()

    # 1. 每条示例命令的子命令与参数
    lines = command_lines(fenced_bash_blocks(text))
    harness.expect(len(lines) >= 10, "文档里的命令示例数量", "至少 10 条", "%d 条" % len(lines))
    for line in lines:
        check_command(harness, args.binary, line)

    # 2. 文档提到的命令组都要出现在 --help 里
    code, stdout, stderr = run(args.binary, ["--help"])
    help_text = stdout + stderr
    harness.expect(code == 0, "hicodeerd --help 可用", "退出码 0", "退出码 %s" % code)
    groups = sorted(documented_groups(text))
    listed_groups = help_groups(help_text)
    not_listed = [group for group in groups if group not in listed_groups]
    harness.expect(
        not not_listed,
        "文档提到的命令组都在 --help 里",
        "全部列出",
        "未列出 %s" % (", ".join(not_listed) if not_listed else "无"),
    )

    # 3. 裸组名退出码 0、未知子命令退出码 2。
    #    Runs over every group the daemon advertises, not only the documented
    #    ones: a group missing from SKILL.md still has to behave the same way,
    #    and that gap is exactly how `config` / `channel` / `completion` were
    #    missed the first time.
    harness.expect(
        bool(listed_groups),
        "--help 列出了命令组",
        "至少一组",
        "一组都没有：%s" % help_text.strip()[:200],
    )
    for group in listed_groups:
        if group in BARE_GROUP_SKIPPED:
            harness.warn(
                "跳过 %s 的裸调用：%s" % (group, BARE_GROUP_SKIPPED[group])
            )
            continue
        code, _, _ = run(args.binary, [group])
        harness.expect(
            code == 0,
            "hicodeerd %s（裸组名）退出码" % group,
            "0",
            str(code),
        )
        code, _, _ = run(args.binary, [group, "nosuchsubcommand"])
        harness.expect(
            code == 2,
            "hicodeerd %s nosuchsubcommand 退出码" % group,
            "2",
            str(code),
        )

    # 4. --skill 输出必须与仓库文件一致
    if args.skip_skill_match:
        harness.warn("跳过 --skill 与 SKILL.md 的一致性断言")
    else:
        code, stdout, stderr = run(args.binary, ["--skill"])
        harness.expect(code == 0, "hicodeerd --skill 可用", "退出码 0", "退出码 %s" % code)
        if stdout != text:
            first_diff = next(
                (
                    "第 %d 行" % number
                    for number, (left, right) in enumerate(
                        zip(stdout.splitlines(), text.splitlines()), start=1
                    )
                    if left != right
                ),
                "行数不同（--skill %d 行，文件 %d 行）"
                % (len(stdout.splitlines()), len(text.splitlines())),
            )
            harness.fail("--skill 输出与 SKILL.md 一致", "逐字节相同", first_diff)
        else:
            harness.ok("--skill 输出与 SKILL.md 一致")

    return harness.finish()


if __name__ == "__main__":
    sys.exit(run_guarded("SKILL.md 命令校验", body, build_parser_options))
