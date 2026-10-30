#!/usr/bin/env python3
"""Emit the license section for dependencies that carry custom terms.

`cargo about` cannot render these: the franken stack (the agent-session scanner
vendored through `herdr`) ships `MIT License (with OpenAI/Anthropic Rider)` and
half of those crates declare it via `license-file` only, so cargo-about reports
"has no license file" and silently drops the group from its output. The
expression is accepted through `hicodeer-licenses.toml`'s `accepted` list, but the
text has to be contributed here or it never reaches the report.

The crate list comes from `cargo metadata` rather than a hard-coded list, so a
version bump or a new dependency with the same terms shows up automatically.

Usage: generate-rider-licenses.py > section.md
"""

import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MANIFEST = os.path.join(
    ROOT, "crates", "gpui_ohos", "depend", "cmd-agent", "hicodeerd", "Cargo.toml"
)
TEXT_FILE = os.path.join(ROOT, "script", "licenses", "franken-rider.txt")
RIDER_MARKER = "OpenAI/Anthropic Rider"
CUSTOM_EXPRESSION = "LicenseRef-MIT-OpenAI-Anthropic-Rider"
RULE = "-" * 80


def metadata():
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--manifest-path", MANIFEST],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        sys.exit("generate-rider-licenses: cargo metadata failed: " + result.stderr.strip()[:400])
    return json.loads(result.stdout)


def carries_rider(package):
    if package.get("license") == CUSTOM_EXPRESSION:
        return True
    license_file = package.get("license_file")
    if not license_file:
        return False
    # `license_file` comes back as written in the manifest (usually a relative
    # path like "LICENSE"), not as an absolute one.
    if not os.path.isabs(license_file):
        license_file = os.path.join(os.path.dirname(package["manifest_path"]), license_file)
    if not os.path.isfile(license_file):
        return False
    with open(license_file, encoding="utf-8", errors="replace") as handle:
        return RIDER_MARKER in handle.read(400)


def main():
    with open(TEXT_FILE, encoding="utf-8") as handle:
        text = handle.read().strip()

    riders = sorted(
        (p for p in metadata()["packages"] if carries_rider(p)),
        key=lambda p: (p["name"], p["version"]),
    )
    if not riders:
        return 0

    lines = [
        "#### MIT License (with OpenAI/Anthropic Rider)",
        "",
        "##### Used by:",
        "",
    ]
    for package in riders:
        repository = package.get("repository") or "https://crates.io/crates/" + package["name"]
        lines.append(
            f"* [{package['name']} {package['version']}]({repository}) "
            f"<!-- rider: {package['name']} {package['version']} -->"
        )
    lines.extend([
        "",
        "This is the MIT text plus a rider denying use to OpenAI, Anthropic, their",
        "affiliates and anyone acting on their behalf. It is not an OSI-approved",
        "license; the rider is reproduced verbatim below.",
        "",
        text,
        "",
        RULE,
        "",
    ])
    sys.stdout.write("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
