# ccwatch

Real-time terminal monitor for Claude Code session usage.

`ccwatch` tails every JSONL session file under `~/.claude/projects/` for the
agents reported by `claude agents --json`, aggregates per-session usage, and
renders two view modes:

- **Summary** (default): one-row-per-session table (status / project / name /
  model / mode / msgs / ctx / session) with an aggregate band at the top
  (live count, total cost, total messages, per-model breakdown). Use ↑↓ to
  select, `Enter` (or `s`) to drill into the detail view.
- **Single**: the detail view of one session — context gauge, tokens, cost,
  burn rate, tool breakdown, output-token sparkline.

## Display

### Summary

```
┌ ccwatch — summary ────────────────────────────────────────────────────────┐
│  live: 3  total: $4.21  msgs: 412  opus:2 sonnet:1                        │
├───────────────────────────────────────────────────────────────────────────┤
│ status   project          name              model       mode    msgs ctx        session│
│ ● busy   alpha-service    rfc-draft         opus-4-7    plan    87   388k 39%   8cc07f81│
│ ● idle   beta-platform    bugfix #42        sonnet-4-6  default 91   220k 22%   769a900c│
│ ○ off    gamma-cli        —                 opus-4-7    edit    234  610k 61%   f465bfe7│
└───────────────────────────────────────────────────────────────────────────┘
 q quit  ·  ↑↓ select  ·  Enter open  ·  s single view  ·  R rescan  ·  b browser
```

`mode` column maps the raw `permission-mode` JSONL event values to short labels:

| Raw value | Label | Meaning |
|---|---|---|
| `default` | `default` | normal permission prompts |
| `plan` | `plan` | plan mode (no edits / actions) |
| `acceptEdits` | `edit` | auto-accept edits |
| `bypassPermissions` | `auto` | bypass all prompts |
| (missing / unknown) | `—` | no `permission-mode` event seen yet |

`ctx` column shows the absolute context size (`62k` / `1.2M`) followed by the
percentage of the assumed window (1M by default, see below).

### Single (detail)

```
┌ ccwatch — /Users/me/code/alpha-service ───────────────────────────────────┐
│ ● model: opus-4-7  project: alpha-service  session: 8cc07f81  elapsed: 23m  msgs: 87  [2/40]  name: rfc-draft │
├──────────────────────────────────────────────────────────────────────────┤
│ context  █ █ █ █ █ █ █ ░ ░ ░ ░ ░ ░ ░ ░ ░  388,582 / 1,000,000  (38.9%)   │
├──────────────────────────────────────────────────────────────────────────┤
│ tokens   in: 12,453   out: 8,120                                          │
│          cache_w: 45,210   cache_r: 132,778   hit: 91.4%                 │
├──────────────────────────────────────────────────────────────────────────┤
│ cost     session: $2.34   rate: 1,240 tok/min   burn: $6.10/hr           │
├──────────────────────────────────────────────────────────────────────────┤
│ tools    Bash:24  Edit:11  Read:38  Grep:7  Agent:2                      │
├ output tokens / message  (latest 25 of 87) ────────────────────────────────┤
│              █ █ █ ▆ █ ▃ █ █ ▆ ▆ █ █ █ ▆ █ ▃ █ █ █ ▆ █ █ █ █ █          │
└──────────────────────────────────────────────────────────────────────────┘
 q quit  ·  r reset  ·  b browser  ·  s summary  ·  Tab/n next  ·  ⇧Tab/p prev  ·  R rescan
```

The `claude-` prefix is stripped from model names in both views for brevity
(`claude-opus-4-7` → `opus-4-7`).

Header dot:
- `● ` green = active (latest event within 2 min)
- `● ` yellow = idle

Color coding:

| Element | Color rule |
|---|---|
| active dot | green = recent activity / yellow = idle |
| context bar | <60% green / 60–80% yellow / >80% red |
| cache hit % | >80% green / 50–80% yellow / <50% red |
| burn rate (single view) | <$5/hr green / $5–15 yellow / >$15 red |
| model name | opus = magenta, sonnet = blue, haiku = cyan |
| project name | green |
| tool names | `Bash`=cyan, `Edit`=yellow, `Read`=blue, `Grep`/`Glob`=magenta, `Write`=green, `Agent`/`Task`=red, `WebFetch`/`WebSearch`=light blue |

## Install

From the workspace root:

```sh
make install                            # → ~/.local/bin/ccwatch
make install PREFIX=/opt/homebrew/bin   # → /opt/homebrew/bin/ccwatch
```

`make install` runs a release build (with LTO + `panic=abort`) once, then copies
the resulting binary to `$(PREFIX)`. Default `PREFIX` is `~/.local/bin`. On
macOS the binary is re-signed (`codesign --force --sign -`) after copy to
satisfy macOS 26+ hardened-runtime checks.

To remove: `make uninstall PREFIX=<same-dir>`.

## Run

```sh
ccwatch                                  # summary of live agents (claude agents --json)
ccwatch --watch                          # auto-rescan every 5s (alone = default interval)
ccwatch --watch 10                       # auto-rescan every 10s
ccwatch --days 7                         # also include sessions touched in the last 7 days
ccwatch --session /path/to/session.jsonl # pin to one file (single-view, no toggle)
ccwatch --projects-dir /custom/path      # override ~/.claude/projects
ccwatch --refresh-ms 500                 # UI redraw interval (default 250ms)
ccwatch --context-window 1000000         # force 1M context window
```

By default the summary lists only sessions reported by `claude agents --json`
(both `interactive` and `background` kinds). `--days N` widens the list to
also include any JSONL touched within the last N days; offline rows show
status `off`. `--session` bypasses summary altogether and pins the single
detail view to one file. The `claude` CLI must be on `PATH` for live
discovery; offline-only mode is not supported.

Keys:

| Mode | Key | Action |
|---|---|---|
| **Summary** | `↑` / `k` | select previous row |
| Summary | `↓` / `j` | select next row |
| Summary | `Enter` / `s` | drill into selected session (Single view) |
| Summary | `R` (capital) | rescan: re-query `claude agents --json` + re-apply `--days` |
| Summary | `b` | open `claude.ai/settings/usage` in default browser |
| Summary | `q` / `Esc` | quit |
| **Single** | `r` (lowercase) | reset accumulated stats for current session |
| Single | `s` | back to Summary (unless `--session` pinned) |
| Single | `Tab` / `n` | next session |
| Single | `Shift+Tab` / `p` | previous session |
| Single | `R` (capital) | rescan |
| Single | `b` | open browser |
| Single | `q` / `Esc` | quit |

**Why `b` instead of building it in?** Subscription quota / session reset
timers / weekly limits shown on `claude.ai/settings/usage` come from a
private API that's not exposed publicly. Replicating it would require
scraping with a session cookie. `b` just opens the browser as a quick jump.

### Context window default

ccwatch assumes **1M context for every model family** because that's the
current Claude Code default and the JSONL doesn't carry per-session tier
info. If you're on the 200k tier (or want the gauge denominator to match
a specific model variant), opt down:

```sh
# ~/.zshrc or ~/.bashrc
export CCWATCH_CONTEXT_WINDOW=200000
```

`--context-window <N>` on the command line takes precedence over the env var.

## Tmux integration (recommended)

`ccwatch` is designed to run in a small bottom pane next to Claude Code.

### Shell alias

Add to `~/.zshrc` or `~/.bashrc`:

```sh
# Open ccwatch in a 14-line pane below the current one
alias ccw='tmux split-window -v -l 14 ccwatch'
```

Then from inside Claude Code (running in a tmux pane), open a new shell and
type `ccw` — a `ccwatch` pane appears below.

### Tmux key binding

For one-keystroke access, bind it in `~/.tmux.conf`:

```tmux
bind-key C-w split-window -v -l 14 'ccwatch'
```

After `tmux source-file ~/.tmux.conf`, press `prefix Ctrl-w` to spawn the pane.

### Persistent layout via tmux session

If you always want Claude Code on top and `ccwatch` below:

```sh
tmux new-session -d -s claude 'claude'
tmux split-window -t claude -v -l 14 'ccwatch'
tmux attach -t claude
```

## How burn rate is computed

`burn` is a **sliding-window rate over the last 10 minutes**, not a session
lifetime average. Specifically:

```
burn = sum(cost of events in last 10min) / 600s × 3600
```

This means:
- Spikes show up immediately when activity ramps up.
- The number drops to zero after ~10 minutes of session idle.
- Replaying a long old session at startup doesn't blow up the rate.

`rate: <N> tok/min` uses the same 10-minute window for `input + output +
cache_creation_input` tokens.

## Data source

`ccwatch` reads the session JSONL files Claude Code writes to:

```
~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl
```

It parses two event types:

`assistant` events (per-turn metrics):
- `message.usage.{input,output,cache_creation_input,cache_read_input}_tokens`
- `message.usage.cache_creation.{ephemeral_5m,ephemeral_1h}_input_tokens`
  (split by TTL for accurate cost — 1hr writes are 2× input rate vs 1.25× for 5min)
- `message.content[].name` for `tool_use` blocks
- `cwd` (project basename for header) and `timestamp` (active detection + sliding window)

`permission-mode` events (mode column in summary):
- `permissionMode` field — latest value wins

The live session list comes from `claude agents --json` (queried at startup,
on `R` rescan, and on the `--watch [SECONDS]` interval when enabled). The
watcher polls each tailed file every 200ms and reads only appended bytes,
so it's cheap to leave running for hours.

## Pricing

Cost is computed from a hardcoded per-model rate table (USD per million tokens)
in `crates/ccwatch/src/pricing.rs`. Adjust if Anthropic updates pricing. Cache
write rates are 1.25× input for 5min ephemeral, 2× input for 1hr ephemeral.

## Known limitations

- "Today's cumulative cost across all sessions" is not implemented. The
  summary's aggregate `total: $X.YY` is across currently-visible sessions
  only; `cost: session` in the single view is for that session alone.
- Per-session tier (200k vs 1M context) cannot be inferred from JSONL.
  Default is 1M; use `CCWATCH_CONTEXT_WINDOW=200000` to opt down.
- Session list refresh is manual (`R`) by default. Use `--watch [SECONDS]`
  to auto-refresh; without it, newly started / closed `claude` sessions
  only appear after pressing `R`.
