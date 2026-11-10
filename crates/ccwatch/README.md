# ccwatch

Real-time terminal monitor for Claude Code session usage: tokens, cost,
context, and tools for each running session.

`ccwatch` tails the JSONL session files under `~/.claude/projects/` for the
agents reported by `claude agents --json` and shows them in two views:

- **Summary** (default): one row per session, with totals on top.
- **Single**: the detail of one session — context gauge, tokens, cost, burn
  rate, tool breakdown, and an output-token sparkline.

Install it with the rest of the toolkit (see the [top README](../../README.md#install)).

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

- By default the summary lists only sessions reported by `claude agents --json`
  (both `interactive` and `background` kinds). The `claude` CLI must be on
  `PATH`; offline-only mode is not supported.
- `--days N` also lists any JSONL touched within the last N days; offline rows
  show status `off`.
- `--session` skips the summary and pins the single view to one file.
- Without `--watch`, the session list refreshes only when you press `R`.

### Keys

| View | Key | Action |
|---|---|---|
| Summary | `↑` / `k`, `↓` / `j` | Select previous / next row |
| Summary | `Enter` / `s` | Open the selected session in the single view |
| Single | `s` | Back to the summary (not with `--session`) |
| Single | `Tab` / `n`, `Shift+Tab` / `p` | Next / previous session (not with `--session`) |
| Single | `r` (lowercase) | Reset the accumulated stats for the current session |
| Both | `R` (capital) | Rescan: re-query `claude agents --json` and re-apply `--days` (not in the single view with `--session`) |
| Both | `b` | Open `claude.ai/settings/usage` in the default browser |
| Both | `q` / `Esc` | Quit |

With `--session`, the single view is pinned to one file, so `s`, `R`,
`Tab` / `n`, and `Shift+Tab` / `p` do nothing there.

Why `b` opens a browser instead of showing quotas: the subscription quota,
session reset timers, and weekly limits on `claude.ai/settings/usage` come
from a private API that is not exposed publicly. Replicating it would require
scraping with a session cookie.

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

The top band shows the live count, total cost, total messages, and the count
per model. The `mode` column maps the `permission-mode` event to a short label:

| Raw value | Label | Meaning |
|---|---|---|
| `default` | `default` | normal permission prompts |
| `plan` | `plan` | plan mode (no edits / actions) |
| `acceptEdits` | `edit` | auto-accept edits |
| `bypassPermissions` | `auto` | bypass all prompts |
| (missing / unknown) | `—` | no `permission-mode` event seen yet |

`ctx` shows the context size (`62k` / `1.2M`) and its percentage of the
assumed window (see [Context window](#context-window)).

### Single

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

Both views drop the `claude-` prefix from model names (`claude-opus-4-7` →
`opus-4-7`).

### Colors

| Element | Color rule |
|---|---|
| header dot | green = active (latest event within 2 min) / yellow = idle |
| context bar | <60% green / 60–80% yellow / >80% red |
| cache hit % | >80% green / 50–80% yellow / <50% red |
| burn rate (single view) | <$5/hr green / $5–15 yellow / >$15 red |
| model name | opus = magenta, sonnet = blue, haiku = cyan |
| project name | green |
| tool names | `Bash`=cyan, `Edit`=yellow, `Read`=blue, `Grep`/`Glob`=magenta, `Write`=green, `Agent`/`Task`=red, `WebFetch`/`WebSearch`=light blue |

## Context window

ccwatch assumes a **1M context for every model family**, because that is the
current Claude Code default and the JSONL does not say which tier a session
uses. On the 200k tier (or to match a specific model), opt down:

```sh
# ~/.zshrc or ~/.bashrc
export CCWATCH_CONTEXT_WINDOW=200000
```

`--context-window <N>` on the command line takes precedence over the env var.

## How the numbers are computed

- **Burn rate** is a sliding-window rate over the last 10 minutes, not a
  session-lifetime average:
  `burn = sum(cost of events in last 10min) / 600s × 3600`.
  Spikes show up at once, the number drops to zero after about 10 minutes
  idle, and replaying a long old session at startup does not inflate it.
- **`rate: <N> tok/min`** uses the same 10-minute window for
  `input + output + cache_creation_input` tokens.
- **Cost** comes from a hardcoded per-model rate table (USD per million tokens)
  in `crates/cctk/src/pricing.rs`; adjust it if Anthropic updates pricing.
  Cache writes cost 1.25× the input rate for 5-minute ephemeral and 2× for
  1-hour ephemeral.

## Data source

ccwatch reads the session files Claude Code writes to
`~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl`:

- `assistant` events: `message.usage.{input,output,cache_creation_input,cache_read_input}_tokens`,
  `message.usage.cache_creation.{ephemeral_5m,ephemeral_1h}_input_tokens`
  (split by TTL for the cache-write cost), `message.content[].name` of
  `tool_use` blocks, `cwd` (project name in the header), and `timestamp`
  (active detection and the sliding window).
- `permission-mode` events: the `permissionMode` field; the latest value wins.

The live session list comes from `claude agents --json`, queried at startup,
on `R`, and on the `--watch [SECONDS]` interval. Each tailed file is polled
every 200ms and only appended bytes are read, so it is cheap to leave running
for hours.

## Running it next to Claude Code in tmux

ccwatch is designed for a small pane below Claude Code.

```sh
# ~/.zshrc or ~/.bashrc: `ccw` opens ccwatch in a 14-line pane below
alias ccw='tmux split-window -v -l 14 ccwatch'
```

```tmux
# ~/.tmux.conf: prefix Ctrl-w opens the same pane
bind-key C-w split-window -v -l 14 'ccwatch'
```

```sh
# a session with Claude Code on top and ccwatch below
tmux new-session -d -s claude 'claude'
tmux split-window -t claude -v -l 14 'ccwatch'
tmux attach -t claude
```

## Known limitations

- There is no "today's cost across all sessions". The summary's
  `total: $X.YY` covers the sessions currently listed; `cost: session` in the
  single view covers that session alone.
- The per-session tier (200k vs 1M context) cannot be read from the JSONL.
  The default is 1M; set `CCWATCH_CONTEXT_WINDOW=200000` to opt down.
- Without `--watch [SECONDS]`, newly started or closed `claude` sessions appear
  only after you press `R`.
