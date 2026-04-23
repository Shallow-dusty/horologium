#!/bin/bash
# Parity regression harness for `horologium status`.
#
# Modes:
#   ./run.sh                 — compare each fixture's current output against
#                              the committed snapshot; exit 1 on any diff.
#   ./run.sh --update        — regenerate all snapshots from current binary.
#                              Use after intentional output changes; review
#                              the snapshot diff before committing.
#   ./run.sh --vs-bash       — additionally diff horologium output against
#                              the legacy bash statusline (informational;
#                              known-accepted divergences exist — see
#                              known-diffs.md).
#
# Determinism note: fixtures deliberately omit `resets_at` so the countdown
# branch is not exercised here (its format is unit-tested separately in
# status.rs::tests). This keeps snapshots independent of wallclock.
#
# Renderer modes covered per fixture: default, powerline, multiline,
# hyperlinks, all (= powerline + multiline + hyperlinks).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
FIXTURES="$SCRIPT_DIR/fixtures"
SNAPSHOTS="$SCRIPT_DIR/snapshots"
HOROLOGIUM="${HOROLOGIUM:-horologium}"
BASH_STATUSLINE="${BASH_STATUSLINE:-$HOME/.backups/claude/statusline.sh.bash-v1.20260423.bak}"

mode="check"
vs_bash=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --update) mode="update" ;;
        --vs-bash) vs_bash=1 ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

if ! command -v "$HOROLOGIUM" >/dev/null 2>&1 && [[ ! -x "$HOROLOGIUM" ]]; then
    echo "horologium binary not found: $HOROLOGIUM" >&2
    echo "Set HOROLOGIUM=/path/to/binary or install to PATH." >&2
    exit 2
fi

mkdir -p "$SNAPSHOTS"

render_modes=(default powerline multiline hyperlinks all)

mode_args() {
    case "$1" in
        default) echo "" ;;
        powerline) echo "--powerline" ;;
        multiline) echo "--multiline" ;;
        hyperlinks) echo "--hyperlinks" ;;
        all) echo "--powerline --multiline --hyperlinks" ;;
    esac
}

pass=0
fail=0
miss=0
failed_cases=()

for fixture in "$FIXTURES"/*.json; do
    [[ -e "$fixture" ]] || { echo "no fixtures found" >&2; exit 2; }
    name=$(basename "$fixture" .json)
    for rmode in "${render_modes[@]}"; do
        args=$(mode_args "$rmode")
        actual=$("$HOROLOGIUM" status $args < "$fixture")
        snap="$SNAPSHOTS/$name.$rmode.snap"

        if [[ "$mode" == "update" ]]; then
            printf '%s\n' "$actual" > "$snap"
            echo "[update] $name.$rmode"
        elif [[ -f "$snap" ]]; then
            expected=$(cat "$snap")
            if [[ "$actual" == "$expected" ]]; then
                pass=$((pass + 1))
            else
                fail=$((fail + 1))
                failed_cases+=("$name.$rmode")
                echo "--- FAIL: $name.$rmode ---"
                diff <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") | sed 's/^/  /' || true
            fi
        else
            miss=$((miss + 1))
            echo "[miss]  $name.$rmode (no snapshot; run with --update)"
        fi
    done
done

if [[ "$mode" == "check" ]]; then
    echo
    echo "=== summary ==="
    echo "pass:    $pass"
    echo "fail:    $fail"
    echo "missing: $miss"
    if (( fail > 0 || miss > 0 )); then
        exit 1
    fi
fi

if (( vs_bash == 1 )); then
    echo
    echo "=== vs bash reference (default mode only, diffs may include accepted divergence) ==="
    if [[ ! -x "$BASH_STATUSLINE" ]]; then
        echo "bash statusline not executable: $BASH_STATUSLINE" >&2
        exit 2
    fi
    for fixture in "$FIXTURES"/*.json; do
        name=$(basename "$fixture" .json)
        h=$("$HOROLOGIUM" status < "$fixture")
        b=$("$BASH_STATUSLINE" < "$fixture" 2>/dev/null || echo "[bash execution failed]")
        if [[ "$h" != "$b" ]]; then
            echo "--- $name ---"
            diff <(printf '%s\n' "$b") <(printf '%s\n' "$h") | sed 's/^/  /' || true
        fi
    done
fi
