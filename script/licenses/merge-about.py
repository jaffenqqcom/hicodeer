#!/usr/bin/env python3
"""Merge several `cargo about generate` reports into one de-duplicated report.

The repository contains three independent cargo dependency graphs that all end
up in the shipped HAP:

  * the top-level workspace (the editor itself),
  * `herdr/` (vendored agent server, standalone workspace) and
  * `crates/gpui_ohos/depend/cmd-agent/hicodeerd/` (standalone workspace that
    pulls `herdr` in as a path dependency, so it covers `herdr/` as well).

`cargo about` can only see one lock file at a time, so it has to run once per
workspace and the results are merged here. Crates shared by several graphs show
up repeatedly; this keeps one entry per (license, crate, version), which is what
a reader expects and what the coverage check asserts.

Usage: merge-about.py REPORT [REPORT...] > merged.md
"""

import re
import sys
from collections import OrderedDict

# `#### MIT` starts a license block; the overview above the first block is
# recomputed from the merged data and therefore ignored on input.
BLOCK_START = re.compile(r"^#### (?P<name>.+?)\s*$", re.MULTILINE)
# `* [crate 1.2.3](https://...)` — the shape rendered by template.md.hbs.
USED_BY = re.compile(r"^\* \[(?P<crate>.+?) (?P<version>[^\]\s]+)\]\((?P<url>.*?)\)\s*$")
# The template ends every block with a horizontal rule.
RULE = re.compile(r"^-{10,}\s*$", re.MULTILINE)


def parse_report(path):
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except OSError as err:
        sys.exit(f"merge-about: cannot read {path}: {err}")

    parts = BLOCK_START.split(text)
    blocks = []
    # split() with one capture group yields [preamble, name, body, name, body, ...]
    for index in range(1, len(parts) - 1, 2):
        blocks.append((parts[index].strip(), parts[index + 1]))
    return blocks


def split_body(body):
    """Return ([(crate, version, url)], license_text) for one license block."""
    body = RULE.sub("", body)
    lines = body.splitlines()

    try:
        header = next(i for i, line in enumerate(lines) if line.strip() == "##### Used by:")
    except StopIteration:
        return [], "\n".join(lines).strip()

    cursor = header + 1
    while cursor < len(lines) and not lines[cursor].strip():
        cursor += 1

    used = []
    while cursor < len(lines):
        match = USED_BY.match(lines[cursor].strip())
        if not match:
            break
        used.append((match["crate"], match["version"], match["url"]))
        cursor += 1

    text = "\n".join(lines[cursor:]).strip()
    return used, text


def main(paths):
    merged = OrderedDict()  # license name -> {"used": {(crate, version): url}, "text": str}
    differing = set()

    for path in paths:
        for name, body in parse_report(path):
            used, text = split_body(body)
            entry = merged.setdefault(name, {"used": OrderedDict(), "text": text})
            if entry["text"] and text and entry["text"] != text:
                # The same license can be rendered from different upstream
                # LICENSE files (an Apache-2.0 copy with the appendix left as a
                # template is shorter than one with it filled in). Keep the
                # longer text, which carries at least everything the other has.
                differing.add(name)
                if len(text) > len(entry["text"]):
                    entry["text"] = text
            for crate, version, url in used:
                entry["used"].setdefault((crate, version), url)

    if not merged:
        sys.exit("merge-about: no license blocks found in the given reports")

    if differing:
        print(
            "merge-about: kept the longest text for " + ", ".join(sorted(differing)),
            file=sys.stderr,
        )

    total = sum(len(entry["used"]) for entry in merged.values())
    out = ["## Overview of licenses:", ""]
    for name, entry in sorted(merged.items(), key=lambda item: (-len(item[1]["used"]), item[0])):
        out.append(f"* {name} ({len(entry['used'])})")
    out.extend(["", f"Total crate entries: {total}", "", "### All license texts:", ""])

    for name, entry in sorted(merged.items()):
        out.extend(["#### " + name, "", "##### Used by:", ""])
        for (crate, version), url in sorted(entry["used"].items()):
            out.append(f"* [{crate} {version}]({url})")
        out.extend(["", entry["text"], "", "-" * 80, ""])

    sys.stdout.write("\n".join(out))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1:])
