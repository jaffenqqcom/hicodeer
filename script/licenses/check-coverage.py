#!/usr/bin/env python3
"""Assert that everything shipped has a license entry in `assets/licenses.md`.

`generate-licenses` calls this after rendering. It covers the artifacts the
cargo reports cannot prove on their own:

  1. every extension id in `assets/extensions/*.tar.gz` has an entry,
  2. every font directory under `assets/fonts/` has an entry,
  3. the CA bundle (shipped in the resfile) is declared,
  4. the daemon dependency graph was actually scanned — probed with crates only
     that graph can contribute,
  5. `hicodeerd` still subsumes `herdr`: if it ever stops doing so, the single
     daemon `cargo about` run silently stops covering herdr and this fails.

Usage: check-coverage.py assets/licenses.md
"""

import glob
import os
import re
import sys
import tarfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

HERDR_LOCK = os.path.join(ROOT, "herdr", "Cargo.lock")
DAEMON_LOCK = os.path.join(ROOT, "crates", "gpui_ohos", "depend", "cmd-agent", "hicodeerd", "Cargo.lock")

NAME = re.compile(r'^name = "([^"]+)"', re.MULTILINE)


def lock_names(path):
    with open(path, encoding="utf-8") as handle:
        return set(NAME.findall(handle.read()))


def extension_ids():
    ids = []
    for archive_path in sorted(glob.glob(os.path.join(ROOT, "assets", "extensions", "*.tar.gz"))):
        with tarfile.open(archive_path, "r:gz") as archive:
            for member in archive.getmembers():
                if not member.isfile() or not member.name.endswith("/extension.toml"):
                    continue
                if member.name.count("/") != 2:
                    continue
                handle = archive.extractfile(member)
                if handle is None:
                    continue
                match = re.search(r'^id = "([^"]+)"', handle.read().decode("utf-8", "replace"), re.MULTILINE)
                if match and match.group(1) not in ids:
                    ids.append(match.group(1))
    return ids


def main(report_path):
    with open(report_path, encoding="utf-8") as handle:
        report = handle.read()
    failures = []

    for ext_id in extension_ids():
        if f"<!-- extension: {ext_id} -->" not in report:
            failures.append(f"extension {ext_id!r} ships without a license entry")

    # The generator emits this marker for extensions whose license could not be
    # resolved. A shipped extension with unknown terms must block the release.
    if "UNRESOLVED" in report:
        failures.append(
            "EXTENSION LICENSES contains UNRESOLVED entries; "
            "resolve them (assets/extensions/licenses.json) before releasing"
        )

    for font_dir in sorted(glob.glob(os.path.join(ROOT, "assets", "fonts", "*"))):
        if not os.path.isdir(font_dir):
            continue
        name = os.path.basename(font_dir)
        if f"## [{name}]" not in report:
            failures.append(f"font {name!r} ships without a license entry")

    if "ca-bundle.crt" not in report:
        failures.append("the CA bundle (resfile/ca-bundle.crt) is not declared in BUNDLED ASSET LICENSES")

    # The franken stack is filtered out of cargo-about's own output (custom
    # expression, text only in `license-file`), so its terms are contributed by
    # the CUSTOM LICENSE TERMS section. When a dependency carries the rider, that
    # section has to be there and has to carry the text.
    if os.path.isfile(DAEMON_LOCK) and "fsqlite" in lock_names(DAEMON_LOCK):
        if "with OpenAI/Anthropic Rider" not in report:
            failures.append(
                "the franken stack ships in the daemon graph but its rider text is missing "
                "from CUSTOM LICENSE TERMS"
            )
        if "<!-- rider: fsqlite " not in report:
            failures.append("the franken stack rider section does not list fsqlite")

    # Prove both cargo runs landed in the merged report. These probes are
    # hard-coded on purpose: a lock-file difference set is not usable here
    # because Cargo.lock lists target-specific and optional dependencies that
    # cargo-about deliberately filters out of its graph.
    for crate in ("tree-sitter",):
        if f"* [{crate} " not in report:
            failures.append(
                f"application cargo report is missing (probe crate {crate!r} not found)"
            )
    for crate in ("russh", "franken-agent-detection"):
        if f"* [{crate} " not in report:
            failures.append(
                f"daemon cargo report is missing (probe crate {crate!r} not found); "
                "did the hicodeerd `cargo about` run in generate-licenses succeed?"
            )

    if os.path.isfile(HERDR_LOCK) and os.path.isfile(DAEMON_LOCK):
        uncovered = lock_names(HERDR_LOCK) - lock_names(DAEMON_LOCK)
        if uncovered:
            sample = ", ".join(sorted(uncovered)[:8])
            failures.append(
                "hicodeerd no longer covers every herdr dependency "
                f"({len(uncovered)} crate(s) missing, e.g. {sample}); "
                "add a separate `cargo about` run for herdr/ to generate-licenses"
            )

    if failures:
        print("check-coverage: license report is incomplete:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(f"check-coverage: {os.path.basename(report_path)} covers every shipped artifact")
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    sys.exit(main(sys.argv[1]))
