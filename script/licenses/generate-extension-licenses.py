#!/usr/bin/env python3
"""Regenerate `assets/extensions/LICENSES` for the bundled extensions.

The extensions are not cargo dependencies, so `cargo about` never sees them.
The two inputs are:

  * `assets/extensions/*.tar.gz` — the archive that actually ships. Only the
    extension ids in it are listed, so a stale `licenses.json` cannot make the
    report claim coverage for something that is not distributed.
  * `assets/extensions/licenses.json` — per-id repository, SPDX identifier and
    copyright line, as captured from each upstream repository.

Every shipped id gets an `<!-- extension: <id> -->` marker; the ids without a
resolved license are listed in a trailing UNRESOLVED section and the script
exits 2, so the build fails until a human decides what to do with them.

Run after changing the archive:  python3 script/licenses/generate-extension-licenses.py
"""

import glob
import json
import os
import re
import sys
import tarfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
EXTENSIONS_DIR = os.path.join(ROOT, "assets", "extensions")
OUTPUT = os.path.join(EXTENSIONS_DIR, "LICENSES")

ID_PATTERN = re.compile(r'^id = "([^"]+)"', re.MULTILINE)
RULE = "*" * 80

MIT_TEXT = """MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""


def shipped_ids():
    """Extension ids present in the bundled archives, in archive order."""
    ids = []
    for archive_path in sorted(glob.glob(os.path.join(EXTENSIONS_DIR, "*.tar.gz"))):
        with tarfile.open(archive_path, "r:gz") as archive:
            for member in archive.getmembers():
                if not member.isfile() or not member.name.endswith("/extension.toml"):
                    continue
                if member.name.count("/") != 2:
                    continue  # only `<id>/extension.toml`, not a nested copy
                handle = archive.extractfile(member)
                if handle is None:
                    continue
                match = ID_PATTERN.search(handle.read().decode("utf-8", "replace"))
                if match and match.group(1) not in ids:
                    ids.append(match.group(1))
    if not ids:
        sys.exit("generate-extension-licenses: no extension.toml found in assets/extensions/*.tar.gz")
    return ids


def license_text(spdx):
    """Full text for an SPDX id, from the repository's own license files."""
    sources = {
        "MIT": None,  # inline above: the text is identical everywhere
        "Apache-2.0": "LICENSE-APACHE",
        "GPL-3.0": "LICENSE-GPL",
    }
    if spdx == "MIT":
        return MIT_TEXT
    source = sources.get(spdx)
    if source is None:
        return None
    path = os.path.join(ROOT, source)
    if not os.path.isfile(path):
        sys.exit(f"generate-extension-licenses: missing {source} in the repository root")
    with open(path, encoding="utf-8") as handle:
        return handle.read().strip()


def main():
    ids = shipped_ids()
    with open(os.path.join(EXTENSIONS_DIR, "licenses.json"), encoding="utf-8") as handle:
        metadata = {item["id"]: item for item in json.load(handle)}

    missing = [ext_id for ext_id in ids if ext_id not in metadata]
    if missing:
        sys.exit(
            "generate-extension-licenses: no license metadata for "
            + ", ".join(missing)
            + " (add them to assets/extensions/licenses.json)"
        )

    groups = {}          # spdx -> [id]
    unresolved = []      # [id]
    for ext_id in ids:
        spdx = metadata[ext_id].get("license") or "NONE"
        if spdx in ("NONE", "NOASSERTION"):
            unresolved.append(ext_id)
        else:
            groups.setdefault(spdx, []).append(ext_id)

    lines = [
        "Bundled extensions. Each entry names the upstream repository and the",
        "copyright line of its LICENSE file; the full license text follows the",
        "group heading. GPL-licensed extensions also ship their corresponding",
        "source at the repository URL listed here.",
        "",
    ]

    order = ["MIT", "Apache-2.0", "GPL-3.0"] + sorted(set(groups) - {"MIT", "Apache-2.0", "GPL-3.0"})
    for spdx in order:
        if spdx not in groups:
            continue
        lines.extend(["#### " + spdx, "", "##### Used by:", ""])
        for ext_id in groups[spdx]:
            item = metadata[ext_id]
            copyright_line = item.get("copyright") or "copyright not stated in the upstream LICENSE"
            lines.append(
                f"* [{ext_id}]({item['repository']}) — {copyright_line} <!-- extension: {ext_id} -->"
            )
        text = license_text(spdx)
        if text is None:
            sys.exit(f"generate-extension-licenses: no license text available for {spdx}")
        lines.extend(["", text, "", RULE, ""])

    if unresolved:
        lines.extend([
            "#### UNRESOLVED — manual review required",
            "",
            "These extensions ship but their license could not be resolved from",
            "the metadata (GitHub reports NOASSERTION/NONE). Open the repository,",
            "copy its LICENSE text, and extend this section before releasing.",
            "",
            "##### Used by:",
            "",
        ])
        for ext_id in unresolved:
            item = metadata[ext_id]
            lines.append(
                f"* [{ext_id}]({item['repository']}) — UNRESOLVED <!-- extension: {ext_id} -->"
            )
        lines.extend(["", RULE, ""])

    with open(OUTPUT, "w", encoding="utf-8") as handle:
        handle.write("\n".join(lines))

    print(f"generate-extension-licenses: wrote {OUTPUT} ({len(ids)} extensions)")
    if unresolved:
        print(
            "generate-extension-licenses: UNRESOLVED licenses: " + ", ".join(unresolved),
            file=sys.stderr,
        )
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
