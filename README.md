# claude-toolkit

[English](README.md) | [日本語](README.ja.md)

[![CI](https://github.com/vankichi/claude-toolkit/actions/workflows/ci.yml/badge.svg)](https://github.com/vankichi/claude-toolkit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/vankichi/claude-toolkit?sort=semver)](https://github.com/vankichi/claude-toolkit/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A Cargo workspace of small terminal tools for working with Claude Code.

## Tools

| Crate | Description |
|---|---|
| [`ccwatch`](crates/ccwatch) | Real-time TUI monitor for Claude Code session usage (tokens, cost, context, tools). |
| [`ccmap`](crates/ccmap) | Read-only TUI to browse available agents, skills, commands, plugins, and MCP servers. |
| [`ccstat`](crates/ccstat) | Read-only TUI ranking model/agent/skill/command/MCP usage aggregated across all session logs; `--watch` adds a live view with a running indicator. |
| [`cctop`](crates/cctop) | Unified bottom-style dashboard combining live usage (Now), the cross-session ranking, the config map, and the Focus panel (the `ccfocus` board) in one screen, with braille-dot bars, fuzzy filtering, and drill-down into each tool's full view. |
| [`ccfocus`](crates/ccfocus) | Claude Code hook (`ccfocus hook <event>`) that records every session's status (idle / working / waiting / done / failed), project name, repo, worktree, and tmux pane as one JSON file per session under `~/.claude/focus/`, read through `cctk::focus`. Never stores prompt or message text. Also a TUI: `ccfocus` (no arguments) or `ccfocus tui` opens the focus board, which shows what to look at next, every session, worktrees left to finish, and today's numbers (see [below](#ccfocus--focus-board-and-session-status-hook)). |

## Install

### Prebuilt binaries (recommended)

macOS / Linux — one command downloads the latest release, verifies its SHA-256
checksum, and installs the tools included in that release into `~/.local/bin`,
replacing any older copies (a release pinned with `VERSION` that predates a
tool simply doesn't install it):

```sh
curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh | sh
```

Prefer to read the script before running it? Download, inspect, then execute:

```sh
curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh -o install.sh
less install.sh
sh install.sh
```

Environment variables:

| Variable | Default | Purpose |
|---|---|---|
| `PREFIX` | `$HOME/.local/bin` | Install destination |
| `VERSION` | latest release | Release tag to install, e.g. `v0.1.0` |

Make sure the install destination is on your `PATH`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

**Windows**: download the `*-x86_64-pc-windows-msvc.zip` asset from the
[latest release](https://github.com/vankichi/claude-toolkit/releases/latest),
verify it against `SHA256SUMS`, and extract the binaries onto your `PATH`.

### From source

```sh
make help                # list all recipes
make build               # debug build
make release             # release build (lto=fat / panic=abort)
make test                # cargo test --workspace
make ci                  # fmt-check + clippy + test
make install             # release build → copy into ~/.local/bin
make install PREFIX=/opt/homebrew/bin   # change install destination
make uninstall           # remove each binary from $(PREFIX)
```

If `~/.local/bin` is on your `PATH`, the tools are runnable right after
`make install`. Otherwise add it (see the export above).

## Usage

The TUI tools (`cctop`, `ccwatch`, `ccstat`, `ccmap`) read your Claude Code
logs under `~/.claude/` automatically — just run them, no setup or arguments
needed. Each is a full-screen TUI; press `q` to quit. `ccfocus` is both a hook
and a TUI: Claude Code runs it as a hook, which you register once, and the
focus board it opens is built from what the hook records (see
[below](#ccfocus--focus-board-and-session-status-hook)).

### `cctop` — the all-in-one dashboard

Start here. One screen, four live panels:

```sh
cctop
```

| Panel | What it shows |
|---|---|
| **Now** `[1]` | Live rollup of every active session — total tokens, cost, context %, and a real-time tokens/min chart. `Enter` breaks it down per session. |
| **Top usage** `[2]` | Cross-session ranking over the last 30 min, per model / agent / skill / command / MCP. |
| **Config map** `[3]` | How many agents / skills / commands / plugins / MCP you have, and which are running right now. |
| **Focus** `[4]` | The [`ccfocus`](#ccfocus--focus-board-and-session-status-hook) board in brief: parallel sessions against the limit, today's numbers, the next session to look at, and the first few sessions. `Enter` opens the full board; `q` / `Esc` returns to the overview. It reads the same directory the hook writes to: `$CCFOCUS_STATE_DIR`, else `~/.claude/focus` (`--claude-dir` does not move it). |

Keys:

| Key | Action |
|---|---|
| `1` `2` `3` `4` | Focus a panel (or `j` / `k` / `Tab` to cycle) |
| `Enter` | Drill into the focused panel's full view |
| `c` | On **Top usage**, cycle the category: model → agent → skill → command → MCP |
| `/` | Fuzzy-filter (type to narrow, `Enter` to apply, `Esc` to clear) |
| `Esc` | Back out of a drill-down |
| `q` | Quit |

Tuning (run `cctop --help` for every flag):

```sh
cctop --refresh-ms 500     # redraw every 0.5s instead of the 0.25s default
cctop --rescan-secs 10     # re-scan logs / config every 10s (Now stays live)
```

### The focused tools

Prefer one view at a time? Each panel also ships as its own tool:

```sh
ccwatch          # live monitor of your active session(s)
ccwatch --watch  # keep the session list auto-refreshing

ccstat           # cross-session usage ranking (one-shot snapshot)
ccstat --watch   # live mode: auto-refresh + a spinner on what's running now

ccmap            # browse every agent / skill / command / plugin / MCP server
```

Every tool accepts `--help` (full flag list) and `--version`. Log and config
locations can be overridden with `--projects-dir` / `--claude-dir` /
`--project-dir` where they apply.

### `ccfocus` — focus board and session status hook

`ccfocus` has two parts that share one state directory. The hook
(`ccfocus hook <event>`) records every Claude Code session's status as Claude
runs; the focus board (`ccfocus` or `ccfocus tui`) reads those records and shows
which session needs you next. Register the hook first: the board has nothing to
show without it.

#### Register the hook

`ccfocus hook <event>` records each session's status (`idle` / `working` /
`waiting` / `done` / `failed`) in `~/.claude/focus/sessions/<session_id>.json`
and appends one line to `~/.claude/focus/events/<YYYYMMDD>.tsv` for each event
it records — including one whose state was superseded by a later overlapping
hook. `SessionEnd` (which removes the session file) and ignored events (other
`Notification` types, and `PostToolBatch` unless it ends a wait) add no line.
Prompt, message, and tool bodies are never stored. Override the location with
`--state-dir` or `CCFOCUS_STATE_DIR`.

Register it for these seven events in your Claude Code settings (for example
`~/.claude/settings.json`). Use the exec form (`command` + `args`) so the
recorded `pid` is Claude's rather than a wrapping shell's, and give the
absolute path to the binary:

```json
{
  "hooks": {
    "SessionStart":     [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "SessionStart"] }] }],
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "UserPromptSubmit"] }] }],
    "Notification":     [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "Notification"] }] }],
    "PostToolBatch":    [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "PostToolBatch"] }] }],
    "Stop":             [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "Stop"] }] }],
    "StopFailure":      [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "StopFailure"] }] }],
    "SessionEnd":       [{ "hooks": [{ "type": "command", "command": "/path/to/ccfocus", "args": ["hook", "SessionEnd"] }] }]
  }
}
```

The hook always exits 0 and prints nothing to stdout, so a failure never
blocks Claude; problems go to stderr as a single line.

#### Focus board

```sh
ccfocus                          # same as `ccfocus tui`
ccfocus tui --state-dir DIR      # read DIR instead of the default
```

The state directory is `--state-dir`, else `$CCFOCUS_STATE_DIR`, else
`~/.claude/focus`. This is a synthetic example (120 columns wide):

```text
focus · main:0                                                              parallel 5 / 4 · next round 14:30 (in 6 min)
┌ next up · waiting 3m ────────────────────────────────────────────────────────────────────┐ ┌ today ──────────────────┐
│[1] repo-a / tmux status line · feat/tmux · dev:2.1                                       │ │tasks done             1 │
└──────────────────────────────────────────────────────────────────────────────────────────┘ │switches              12 │
 # status   repo         project                               worktree       start updated  │yesterday             56 │
 1 waiting  repo-a       tmux status line                      feat-tmux      01/02      3m  │parallel avg/max 2.4 / 5 │
 2 done     repo-b       e2e 安定化                            e2e-388        12/31     12m  │week (switches/day)      │
 3 working  repo-c       token auth                            guard-token    01/03     40s  │▄▇█▃▆█▂                  │
 4 working  repo-a       -                                     main           09:12      2m  └─────────────────────────┘
 - stopped  repo-d       renovate batching                     renovate-2     12/29      6h
   2 sessions done or stopped 2+ days ago









to finish (ahead of base · no PR)
   repo-a       guard-egress   ↑1 ↓5 clean
   repo-c       guard-token    ↑3 ↓0 2 uncommitted [3]
1-9 jump · j/k select · d diff (hunk) · p name window · r reload · q quit
```

The screen has four parts, plus a header and a key help line:

- **Header**: `parallel N / L` is the number of sessions with an event in the
  last 30 minutes (N) against your limit (L). It turns yellow at the limit and
  red above it; it only warns and never stops anything. Next to it is the time
  of the next check-in (`next round`), on a grid of `round_minutes` from local
  midnight.
- **Next up** (`next up`): the session to look at next. The order is waiting
  for approval (`waiting`, longest wait first), then failed (`failed`), then
  finished but not yet visited (`done`), then working (`working`). A finished
  session counts as visited once you jump to it from the board.
- **List**: every session as `#`, status, repo, project, worktree, start time,
  and time since the last update. Rows are in the same order as above, then idle
  (`idle`), then stopped (`stopped`). Rows `1`-`9` can be jumped to with the
  number key; stopped rows have no number.
- **Worktrees to finish** (`to finish`): worktrees that are ahead of
  `origin/HEAD` and whose branch has no open or merged PR of yours. A PR that
  was closed without merging does not hide the branch (the work is not
  finished, so it stays a candidate). Each line shows the commits ahead (`↑`)
  and behind (`↓`), the number of uncommitted entries (`N uncommitted`, or
  `clean`), and the numbers of the sessions using it.
- **Today** (`today`): `tasks done` is the number of your PRs created or merged
  today; `switches` counts prompts sent to a different session than the previous
  prompt; `yesterday` is yesterday's `switches`; `parallel avg/max` is the mean
  and maximum number of sessions active in one of today's 30-minute windows; the
  bars (`week`) show switches per day for the last seven days, ending today.
  Below 110 columns the box becomes a single line above the list
  (`today  done 1 · switches …`).

Session status:

- A session on the same side as the board (same host or container, and the same
  hostname) shows `stopped` once its `pid` is gone.
- Otherwise, silence is the only signal: `stopped` appears after 30 minutes
  without an event. This applies to a session on the other side (a container
  session seen from the host, or the reverse), whose `pid` can't be checked,
  and to a same-side session whose liveness can't be determined (no recorded
  `pid`, no `kill` command, Windows, and so on).
- A finished or stopped session untouched for 2 days or more is folded into one
  count line (`N sessions done or stopped 2+ days ago`).

Keys:

| Key | Action |
|---|---|
| `1`-`9` | Switch to that session's tmux pane (`tmux switch-client`). A container session goes to the `container_target` window. |
| `j` / `k` (`↓` / `↑`) | Move the selection |
| `Enter` | Switch to the selected row |
| `d` | Open `hunk diff <merge-base> --watch` in a tmux popup over the board, in the selected row's worktree (`<merge-base>` is `git merge-base HEAD origin/HEAD`). Quitting hunk closes the popup; the window's layout is left as it was |
| `p` | Rename the selected row's tmux window to its project name |
| `r` | Re-read now (including `config.toml`) |
| `q` / `Esc` | Close the board |

Outside tmux, the keys that move or rename (`1`-`9`, `Enter`, `d`, `p`) only
print the reason on the bottom line.

Tools the board calls:

- `tmux`: the `1`-`9`, `Enter`, `d`, and `p` keys.
- `hunk`: the `d` key.
- `gh`: `tasks done` and the PR check for worktrees to finish. When `gh` is
  missing or fails, `tasks done` shows `n/a` and worktrees to finish are not
  narrowed by PR.

The board never runs `git fetch`; it reads local refs only. Session files and
events are re-read every 2 seconds, worktrees every 30 seconds, and `gh` results
are cached for 10 minutes (`r` does not refresh `gh`).

Settings go in `<state dir>/config.toml`. Only flat `key = value` lines are
read (no tables, no escapes). `config.toml` may be a symlink (it is followed),
so you can distribute it from your dotfiles. If the file is broken, the board
runs with the defaults and shows the reason on the bottom line.

```toml
limit = 4                # parallel-session limit (1-99)
round_minutes = 30       # check-in interval; 0 hides it (0-1440)
container_target = "dev" # tmux target for jumping to a container session
```

Housekeeping: the board (`ccfocus` and cctop's Focus panel) runs a cleanup at
start-up and every hour. It deletes event logs (`events/*.tsv`) older than 90
days and hook temp files (`.<id>.json.<pid>.<nanos>.tmp`) older than 10 minutes.

What leaves your machine: `gh` asks GitHub only for a search of your own
(`@me`) PRs and, in each session's repo, `gh pr list --author @me`. The board
never reads prompts or transcripts.

## Releasing

Pushing a `v*` tag triggers [`.github/workflows/release.yml`](.github/workflows/release.yml),
which builds every binary on its native runner (macOS / Linux / Windows),
bundles the tools per target, generates `SHA256SUMS`, and publishes a
GitHub Release with the archives attached.

```sh
git tag v0.1.0
git push origin v0.1.0
```

## Adding a new tool

1. Create `crates/<new-name>/Cargo.toml`, inheriting workspace metadata
   (`edition.workspace = true`, etc.)
2. Inherit lints with `[lints] workspace = true`
3. Add a row to the tools table in this README **and** `README.ja.md`
4. Confirm `make ci` passes

The `/add-rust-crate` skill scaffolds this, so most of the manual work is
handled for you.
