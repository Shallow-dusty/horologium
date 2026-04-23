# Accepted divergences between `horologium status` and the legacy bash statusline

This file documents every output difference between `horologium status`
(default mode) and `~/.claude/statusline.sh` (the bash predecessor). Each
divergence has been reviewed and **accepted** — i.e. horologium's behavior
is considered correct (or an intentional improvement), and reverting would
regress.

Regenerate the diff report at any time with:

```sh
tests/parity/run.sh --vs-bash
```

## 1. Identity segment spacing — `3 spaces` → `2 spaces`

- **bash**: `${MODEL}   ${DIR}   ${BRANCH}` (three spaces between model/dir/branch)
- **horologium**: `${MODEL}  ${DIR}  ${BRANCH}` (two spaces everywhere)
- **Why accepted**: horologium uses a uniform 2-space separator across all
  segments (identity + usage). The bash heterogeneity (3 spaces in the
  identity block, 2 in the usage block, 1 between 5h/7d) was not a deliberate
  design choice — just an accretion. Uniform spacing reads cleaner under
  powerline/multiline modes too.

## 2. Rate-segment join spacing — `1 space` → `2 spaces`

- **bash**: `${SEG5} ${SEG7}` (single space between 5h and 7d)
- **horologium**: `${SEG5}  ${SEG7}`
- **Why accepted**: same uniformity rationale as above.

## 3. Color reset code — `ESC[0m` → `ESC[39m`

- **bash**: `\033[0m` — full SGR reset (clears fg, bg, bold, etc.)
- **horologium**: `\033[39m` — default foreground only (via `owo-colors`)
- **Why accepted**: horologium never sets bg/bold in plain mode, so the
  narrower reset is sufficient and slightly cheaper. Visually identical in
  every mainstream terminal.

## 4. UTF-8 / whitespace in `workspace.current_dir` basename

Fixture: `07-utf8-dir.json` → `/tmp/项目 space`

- **bash**: renders `项目` (truncated)
- **horologium**: renders `项目 space` (full)
- **Why accepted**: **bash has a bug** — `xargs basename` splits its
  argument on whitespace, so any directory name with a space loses
  everything after the first space. horologium uses Rust's `Path::file_name`
  which handles arbitrary Unicode correctly.

## 5. Branch emitted even when `workspace.current_dir` is missing

Fixture: `01-empty.json` → `{}`

- **bash**: still prints `main` (or whatever branch the current shell cwd is on)
- **horologium**: prints nothing for the branch segment
- **Why accepted**: **bash has a second bug** — `git branch --show-current`
  is invoked against the *invoking process's cwd*, not the `current_dir`
  from the JSON. This means the bash statusline happily reports the wrong
  branch when Claude Code sends a cwd that differs from the shell's cwd
  (common with `/add-dir` or multi-worktree setups). horologium consults
  `.git/HEAD` under `workspace.current_dir`, so it reports the right branch
  or nothing.

## Not diverging (verified parity)

- Context % integer truncation (bash `cut -d. -f1` == Rust `as i64`)
- Cost formatting (`$%.2f`)
- Rate % banker's rounding (matches bash `printf '%.0f'` glibc behavior)
- Countdown formatting (`Xd Yh` / `Xh Ym` / `Ym` / `reset`)
- Color thresholds (<70 green / 70-89 yellow / ≥90 red)
