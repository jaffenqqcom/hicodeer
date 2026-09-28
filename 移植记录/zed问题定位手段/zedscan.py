#!/usr/bin/env python3
"""Pre-delivery branding gate for Zed-derived build artifacts.

Companion to psrch.py. Rationale and full checklist live in
    移植记录/design/2026-09-26-去zed化清点.md

Usage:
    python3 zedscan.py [artifact] [--samples N] [--quiet]

Default artifact is the OHOS native library. Counts are per matching line
produced by `strings -n 7`, except dimensions listed in UNIQUE_DIMENSIONS,
which count distinct matched names. Exit code is 1 while any dimension still
has hits, so this can be used directly as a release gate.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys

DEFAULT_ARTIFACT = "hap/entry/libs/arm64-v8a/libhicodeer.so"
STRINGS_MIN_LENGTH = "7"

# Measured on DEFAULT_ARTIFACT on 2026-09-26 with `strings -n 7`.
# None means "not yet measured"; the gate still fails on any non-zero count.
BASELINE = {
    "word_Zed": 284,
    "domain_zed.dev": 91,
    "scheme_zed://": 16,
    "http_x-zed-": 13,
    "env_ZED_": 20,
    "copyright_Zed_Industries": 4,
    "path_crates/zed/src/": 15,
}

# Dimensions reported as distinct matched names instead of matching lines.
UNIQUE_DIMENSIONS = {"env_ZED_"}

PATTERNS = {
    "word_Zed": re.compile(r"\bZed\b"),
    "domain_zed.dev": re.compile(r"zed\.dev"),
    "scheme_zed://": re.compile(r"zed://"),
    "http_x-zed-": re.compile(r"x-zed-"),
    "env_ZED_": re.compile(r"\bZED_[A-Z0-9_]{2,}"),
    "copyright_Zed_Industries": re.compile(r"Zed Industries"),
    "path_crates/zed/src/": re.compile(r"crates/zed/src/"),
}

LABELS = {
    "word_Zed": "独立单词 Zed（行数）",
    "domain_zed.dev": "zed.dev 域名（行数）",
    "scheme_zed://": "zed:// scheme（行数）",
    "http_x-zed-": "x-zed-* HTTP 头（行数）",
    "env_ZED_": "ZED_* 环境变量（去重）",
    "copyright_Zed_Industries": "Zed Industries 署名（行数）",
    "path_crates/zed/src/": "crates/zed/src/ 路径串（行数）",
}


def collect(artifact, sample_limit):
    counts = {name: 0 for name in PATTERNS}
    distinct = {name: set() for name in UNIQUE_DIMENSIONS}
    samples = {name: [] for name in PATTERNS}
    seen = {name: set() for name in PATTERNS}

    process = subprocess.Popen(
        ["strings", "-n", STRINGS_MIN_LENGTH, artifact],
        stdout=subprocess.PIPE,
        text=True,
        errors="replace",
    )
    for line in process.stdout:
        # Every tracked pattern contains "zed"; skip early to keep the scan fast.
        if "zed" not in line.lower():
            continue
        for name, pattern in PATTERNS.items():
            match = pattern.search(line)
            if match is None:
                continue
            counts[name] += 1
            if name in UNIQUE_DIMENSIONS:
                distinct[name].add(match.group(0))
            if len(samples[name]) < sample_limit:
                text = line.strip()[:150]
                if text not in seen[name]:
                    seen[name].add(text)
                    samples[name].append(text)
    process.wait()

    for name in UNIQUE_DIMENSIONS:
        counts[name] = len(distinct[name])
    return counts, samples


def render(counts, samples, artifact, quiet):
    size = os.path.getsize(artifact)
    print("== 目标 ==")
    print("   %s (%.1f MB)" % (artifact, size / (1024.0 * 1024.0)))

    print()
    print("== 计数（当前 / 基线 2026-09-26 / 变化）==")
    for name in PATTERNS:
        current = counts[name]
        base = BASELINE.get(name)
        if base is None:
            delta = "-"
            base_text = "未测"
        else:
            delta = "%+d" % (current - base)
            base_text = "%d" % base
        print("   %-28s %6d / %6s / %6s" % (LABELS[name], current, base_text, delta))

    if not quiet:
        for name in PATTERNS:
            if not samples[name]:
                continue
            print()
            print("--- %s 样本 ---" % LABELS[name])
            for text in samples[name]:
                print("   ", text)

    dirty = [name for name in PATTERNS if counts[name] > 0]
    print()
    print("== 判定 ==")
    if dirty:
        print("   失败：%d 个维度仍有命中" % len(dirty))
        for name in dirty:
            print("     - %s：%d" % (LABELS[name], counts[name]))
        print("   判据是全部维度归零，不是「界面看不到 Zed」。")
        return 1
    print("   通过：全部维度为 0")
    print("   注意：这只覆盖商标/品牌线；许可证线见清点文档第 1 节。")
    return 0


def main():
    parser = argparse.ArgumentParser(
        description="扫描构建产物里的 Zed 可见面（交付前门禁）"
    )
    parser.add_argument(
        "artifact",
        nargs="?",
        default=DEFAULT_ARTIFACT,
        help="待扫描的产物路径（默认 %s）" % DEFAULT_ARTIFACT,
    )
    parser.add_argument(
        "--samples",
        type=int,
        default=5,
        help="每个维度打印的样本条数（默认 5，0 表示不打印）",
    )
    parser.add_argument(
        "--quiet",
        action="store_true",
        help="只打印计数与判定，不打印样本（适合构建日志）",
    )
    args = parser.parse_args()

    if shutil.which("strings") is None:
        print("!! 找不到 strings（binutils），无法扫描", file=sys.stderr)
        return 2
    if not os.path.isfile(args.artifact):
        print("!! 产物不存在: %s" % args.artifact, file=sys.stderr)
        return 2

    counts, samples = collect(args.artifact, max(args.samples, 0))
    return render(counts, samples, args.artifact, args.quiet)


if __name__ == "__main__":
    sys.exit(main())
