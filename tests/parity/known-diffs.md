# Accepted divergences between `horologium status` and the legacy bash statusline

This file documents every output difference between `horologium status`
(default mode) and `~/.claude/statusline.sh` (the bash predecessor). Each
divergence has been reviewed and **accepted** — i.e. horologium's behavior
is considered correct (or an intentional improvement), and reverting would
regress.

The divergences split into two categories:

- **Sanctioned normalization** (§A) — intentional unification of spacing
  and escape-code idioms. Not bash bugs; the bash statusline's choices
  accreted over time without a design brief.
- **Bash-side bugs horologium avoids** (§B) — cases where the bash
  implementation is demonstrably wrong and horologium's behavior is the
  correct one.

Regenerate the diff report at any time:

```sh
tests/parity/run.sh --vs-bash
```

## §A. Sanctioned normalization

### A1. Identity segment spacing — 3 spaces → 2 spaces

- **bash**: `${MODEL}   ${DIR}   ${BRANCH}` (three spaces between fields)
- **horologium**: `${MODEL}  ${DIR}  ${BRANCH}` (two spaces uniformly)
- **Rationale**: horologium uses a uniform 2-space separator across all
  segments (identity + usage). The bash heterogeneity (3 in identity,
  2 in usage, 1 between 5h/7d) was not a design decision — just accretion.
  Uniform spacing also reads better under powerline/multiline modes.

### A2. Rate-segment join spacing — 1 space → 2 spaces

- **bash**: `${SEG5} ${SEG7}` (single space)
- **horologium**: `${SEG5}  ${SEG7}`
- **Rationale**: same uniformity argument as A1.

### A3. Color reset code — `ESC[0m` → `ESC[39m`

- **bash**: `\033[0m` — full SGR reset (clears fg, bg, bold, italics…)
- **horologium**: `\033[39m` — default foreground only (via `owo-colors`)
- **Rationale**: horologium never sets bg or text attributes in plain
  mode, so the narrower reset is sufficient. Visually identical in every
  mainstream terminal.

## §B. Bash-side bugs horologium avoids

### B1. `xargs basename` truncates directory names at whitespace

Fixture: `07-utf8-dir.json` → `workspace.current_dir = "/tmp/项目 space"`

- **bash**: renders `项目` (truncated at the first space)
- **horologium**: renders `项目 space` (full)
- **Cause**: `echo ... | xargs basename` word-splits its stdin, so any
  path containing whitespace loses everything after the first space.
- **Fix**: horologium uses Rust `Path::file_name`, which is agnostic to
  whitespace and handles arbitrary Unicode.

### B2. `git branch --show-current` reads the wrong repo

Fixtures: `01-empty.json` (no cwd), `02-08` (cwd points at a non-git
dir or a stub repo whose branch is `demo`).

- **bash**: prints the branch of the *invoking shell's cwd*, which is
  typically whatever repo the statusline script was launched from
  (e.g. always `main` during these tests because the harness runs under
  the Horologium repo itself). Unrelated to the JSON.
- **horologium**: consults `.git/HEAD` under `workspace.current_dir`;
  returns nothing when the dir isn't a git repo, returns the stub's
  `demo` branch for fixture 08.
- **Cause**: `git branch --show-current` takes no path; it operates on
  `$PWD`. The bash statusline never changes directories before calling
  it.
- **Impact**: severe under `/add-dir` or multi-worktree sessions — bash
  happily reports a wrong branch name with no visual clue.
- **Fix**: horologium calls its own `current_branch(Path)` rooted at
  `workspace.current_dir`, so it reports what Claude Code actually sent.

## Not diverging (verified parity in snapshot tests)

- Context % integer truncation (`jq | cut -d. -f1` ≡ Rust `as i64`)
- Cost formatting (`$%.2f`)
- Rate % banker's rounding (matches glibc `printf '%.0f'`, via
  `f64::round_ties_even` — see fixtures `05-rate-red` and `06-ties-rounding`)
- Countdown formatting (`Xd Yh` / `Xh Ym` / `Ym` / `reset`) — unit-tested
  in `src/status.rs::tests::countdown_formats`
- Rate-block 5h gate (renders both 5h+7d iff 5h present, skips entirely
  otherwise — fixtures `09-only-5h`, `10-only-7d`)
- Color thresholds (<70 green / 70-89 yellow / ≥90 red — fixtures
  `03/04/05-rate-*`)
