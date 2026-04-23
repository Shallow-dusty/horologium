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
# Determinism: fixtures deliberately omit `resets_at` so the countdown
# branch stays wallclock-independent (its format is unit-tested separately
# in src/status.rs::tests). Fixtures that need a git branch use the
# `__STUB_GIT_REPO__` placeholder, which the harness rewrites to a locally
# generated stub repo on disk (./runtime-gitrepo/, gitignored).
#
# Requires: bash 4+ (arrays, `[[ ]]`, process substitution), `diff`, `sed`.
# `jq` is not required.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
FIXTURES="$SCRIPT_DIR/fixtures"
SNAPSHOTS="$SCRIPT_DIR/snapshots"
RUNTIME_REPO="$SCRIPT_DIR/runtime-gitrepo"
HOROLOGIUM="${HOROLOGIUM:-horologium}"
BASH_STATUSLINE="${BASH_STATUSLINE:-$HOME/.backups/claude/statusline.sh.bash-v1.20260423.bak}"

mode="check"
vs_bash=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --update) mode="update" ;;
        --vs-bash) vs_bash=1 ;;
        -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
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

# Generate a stub git repo sufficient for `horologium`'s HEAD parser.
# Only .git/HEAD is needed — no objects or refs. Idempotent so repeated
# runs don't rewrite the file.
ensure_stub_gitrepo() {
    local head="$RUNTIME_REPO/.git/HEAD"
    local expected='ref: refs/heads/demo'
    mkdir -p "$RUNTIME_REPO/.git"
    if [[ ! -f "$head" ]] || [[ "$(head -n1 "$head" 2>/dev/null)" != "$expected" ]]; then
        printf '%s\n' "$expected" > "$head"
    fi
}
ensure_stub_gitrepo

render_modes=(default powerline multiline hyperlinks all)

# Feed a fixture to horologium with the right flags, substituting the
# __STUB_GIT_REPO__ placeholder if present. Returns horologium's stdout on
# success; propagates its exit code on failure so the caller can diagnose.
# Builds argv as an array so future flag values containing spaces won't be
# silently split by word-splitting.
render_one() {
    local fixture="$1" rmode="$2"
    local args=()
    case "$rmode" in
        default)     args=() ;;
        powerline)   args=(--powerline) ;;
        multiline)   args=(--multiline) ;;
        hyperlinks)  args=(--hyperlinks) ;;
        all)         args=(--powerline --multiline --hyperlinks) ;;
    esac
    # $RUNTIME_REPO is built from $SCRIPT_DIR, which is realpath'd; it
    # won't contain `|`, so using `|` as the sed delimiter is safe.
    sed "s|__STUB_GIT_REPO__|$RUNTIME_REPO|g" "$fixture" \
        | "$HOROLOGIUM" status "${args[@]}"
}

# Print a diff block with two-space indentation without `diff | sed`
# pipeline masking. Captures diff's output (exit 1 on differences is
# expected and absorbed with `|| true`); sed then runs standalone.
print_indented_diff() {
    local a="$1" b="$2"
    local out
    out=$(diff <(printf '%s\n' "$a") <(printf '%s\n' "$b") || true)
    printf '%s\n' "$out" | sed 's/^/  /'
}

pass=0
fail=0
miss=0

for fixture in "$FIXTURES"/*.json; do
    [[ -e "$fixture" ]] || { echo "no fixtures found" >&2; exit 2; }
    name=$(basename "$fixture" .json)
    for rmode in "${render_modes[@]}"; do
        # Use `if !` so `set -e` doesn't silently kill the loop on a
        # non-zero exit from horologium; we want the error surfaced.
        if ! actual=$(render_one "$fixture" "$rmode"); then
            echo "ERROR: $name.$rmode — horologium exited non-zero" >&2
            fail=$((fail + 1))
            continue
        fi
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
                echo "--- FAIL: $name.$rmode ---"
                print_indented_diff "$expected" "$actual"
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
        # Substitute placeholder once; both runners consume the same payload.
        payload=$(sed "s|__STUB_GIT_REPO__|$RUNTIME_REPO|g" "$fixture")
        if ! h=$(printf '%s' "$payload" | "$HOROLOGIUM" status); then
            echo "--- $name --- (horologium errored)"; continue
        fi
        b=$(printf '%s' "$payload" | "$BASH_STATUSLINE" 2>/dev/null || echo "[bash execution failed]")
        if [[ "$h" != "$b" ]]; then
            echo "--- $name ---"
            print_indented_diff "$b" "$h"
        fi
    done
fi
