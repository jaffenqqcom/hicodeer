#!/usr/bin/env bash
# Builds the herdr unit tests, signs the test binary, and runs it.
#
# Why signing is needed: this device refuses to exec a large unsigned ELF
# (EPERM straight from execve; a stripped copy then fails with EACCES because
# the `.codesign` section is gone). Small test binaries such as the icons crate
# run unsigned, so the restriction is size-related and only the herdr binary
# trips it. Stripping first keeps the signed copy at ~130 MB instead of ~1.1 GB.
#
# Usage:
#   agent-tests/run-unit-tests.sh                  # every unit test in the lib
#   agent-tests/run-unit-tests.sh 'sessions::'     # one module path filter
#   agent-tests/run-unit-tests.sh -- --nocapture   # extra test-binary arguments
#
# The filter is passed to the test binary, not to cargo, so the binary is built
# once and the same artifact can be re-run with different filters.

set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HERDR_ROOT="$PROJECT_ROOT/herdr"
CRATE="hicodeer-agent"
WORK_DIR="${TMPDIR:-/tmp}/herdr-unit-tests"

FILTER=""
BINARY_ARGS=()
while [ $# -gt 0 ]; do
    case "$1" in
        --)
            shift
            BINARY_ARGS+=("$@")
            break
            ;;
        *)
            if [ -z "$FILTER" ]; then
                FILTER="$1"
            else
                BINARY_ARGS+=("$1")
            fi
            shift
            ;;
    esac
done

for tool in cargo strip binary-sign-tool; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: $tool not found in PATH" >&2
        exit 2
    fi
done

mkdir -p "$WORK_DIR"

echo "== building test binary ($CRATE)"
BUILD_LOG="$WORK_DIR/build.log"
if ! (cd "$HERDR_ROOT" && cargo test -p "$CRATE" --lib --no-run >"$BUILD_LOG" 2>&1); then
    echo "error: the test binary did not build; full log at $BUILD_LOG" >&2
    grep -E '^error' -A 6 "$BUILD_LOG" >&2 || tail -40 "$BUILD_LOG" >&2
    exit 1
fi

UNSIGNED="$(sed -n 's/^  Executable unittests .*(\(.*\))$/\1/p' "$BUILD_LOG" | tail -1)"
# cargo reports the path relative to the directory it ran in.
case "$UNSIGNED" in
    /*) ;;
    *) UNSIGNED="$HERDR_ROOT/$UNSIGNED" ;;
esac
if [ -z "$UNSIGNED" ] || [ ! -f "$UNSIGNED" ]; then
    echo "error: could not find the test binary in $BUILD_LOG" >&2
    tail -20 "$BUILD_LOG" >&2
    exit 1
fi

STAGED="$WORK_DIR/$(basename "$UNSIGNED")"
SIGNED="$WORK_DIR/$(basename "$UNSIGNED").signed"
if [ ! -f "$SIGNED" ] || [ "$UNSIGNED" -nt "$SIGNED" ]; then
    echo "== staging the test binary"
    # Both files are removed first: the signing tool appends its block to the
    # output and fails when an older signature is already there.
    rm -f "$STAGED" "$SIGNED"
    cp -f "$UNSIGNED" "$STAGED"
    strip "$STAGED"
    binary-sign-tool sign -inFile "$STAGED" -outFile "$SIGNED" -selfSign 1 >"$WORK_DIR/sign.log" 2>&1
    chmod 775 "$STAGED" "$SIGNED"
    rm -f "$STAGED"
fi

echo "== running $SIGNED ${FILTER:-<all tests>}"
set +e
"$SIGNED" ${FILTER:+"$FILTER"} "${BINARY_ARGS[@]}"
STATUS=$?
set -e
echo "== exit status: $STATUS"
exit "$STATUS"
