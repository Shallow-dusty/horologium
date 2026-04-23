# `horologium status` parity harness

Snapshot-based regression harness for the statusline renderer. Guards against
unintentional output changes when refactoring `status.rs`.

## Layout

```
tests/parity/
├── README.md           # this file
├── known-diffs.md      # documented divergences from the legacy bash statusline
├── run.sh              # harness entrypoint
├── fixtures/*.json     # hand-curated Claude Code stdin payloads
└── snapshots/*.snap    # expected outputs (one per fixture × render mode)
```

Each fixture is rendered in 5 modes: `default`, `powerline`, `multiline`,
`hyperlinks`, `all` (= powerline + multiline + hyperlinks). That yields
one `.snap` file per mode per fixture (7 × 5 = 35 cases today).

## Usage

```sh
# From the repo root:
tests/parity/run.sh              # check mode — exit 1 if any diff
tests/parity/run.sh --update     # regenerate all snapshots (use after a
                                 # deliberate output change; commit the
                                 # snapshot diff alongside the code change)
tests/parity/run.sh --vs-bash    # also diff against the bash statusline
                                 # (informational; see known-diffs.md)
```

Env vars:

- `HOROLOGIUM` — override the binary path (default: `horologium` on PATH)
- `BASH_STATUSLINE` — path to the legacy `statusline.sh` backup
  (default: `~/.backups/claude/statusline.sh.bash-v1.20260423.bak`)

## Why no countdown in fixtures

Fixtures deliberately omit `rate_limits.*.resets_at`. The countdown segment
(`⏳Xh Ym`) depends on `SystemTime::now()`, which would make snapshots
wallclock-dependent. Countdown *formatting* is covered by unit tests
(`fmt_countdown` in `src/status.rs`), so the integration layer only needs
to verify the non-countdown rendering path.

## Adding a fixture

1. Drop a new JSON file in `fixtures/` (name it `NN-description.json`).
2. Run `./run.sh --update` to generate its snapshots.
3. Inspect the new `.snap` files — they must match your intent.
4. Commit fixture + snapshots together.

## Failure workflow

When a check-mode run fails:

1. Read the inline diff.
2. If the new output is correct: `./run.sh --update`, review `git diff`
   on the snapshot files, commit with a note explaining the behavior change.
3. If the old output was correct: fix the regression in `src/status.rs`,
   re-run `./run.sh` until it passes.

Never auto-update snapshots in the same commit as unrelated work.
