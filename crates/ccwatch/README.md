# ccwatch

Real-time terminal monitor for Claude Code session usage.

`ccwatch` tails the JSONL session files under `~/.claude/projects/` and renders a
live TUI showing context window usage, token counts, cost, sliding-window burn
rate, tool call breakdown, and an output-token bar chart. Multi-session aware:
`Tab` cycles through every project's sessions.

## Display

```
┌ ccwatch — /Users/me/go/src/github.com/me/myrepo ──────────────────────────┐
│ ● model: claude-opus-4-7  project: myrepo  session: 8cc07f81  elapsed: 23m  msgs: 87  [2/40] │
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
 q quit  ·  r reset  ·  Tab/n next  ·  ⇧Tab/p prev  ·  R rescan
```

Header dot:
- `● ` green = active (latest event within 2 min)
- `● ` yellow = idle

Color coding:

| Element | Color rule |
|---|---|
| active dot | green = recent activity / yellow = idle |
| context bar | <60% green / 60–80% yellow / >80% red |
| cache hit % | >80% green / 50–80% yellow / <50% red |
| burn rate | <$5/hr green / $5–15 yellow / >$15 red |
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
ccwatch                                  # multi-session: auto-detect, Tab to switch
ccwatch --session /path/to/session.jsonl # single-session: pin to one file
ccwatch --projects-dir /custom/path      # override ~/.claude/projects
ccwatch --refresh-ms 500                 # UI redraw interval (default 250ms)
ccwatch --context-window 1000000         # force 1M context window
```

Keys:

| Key | Action | Mode |
|---|---|---|
| `q` / `Esc` | quit | always |
| `r` (lowercase) | reset current session's accumulated stats | always |
| `b` | open `claude.ai/settings/usage` in default browser | always |
| `Tab` / `n` | next session | multi-session |
| `Shift+Tab` / `p` | previous session | multi-session |
| `R` (capital) | re-scan disk for sessions (picks up new sessions) | multi-session |

`--session` pins to one file and disables the navigation keys.

**Why `b` instead of building it in?** Subscription quota / session reset
timers / weekly limits shown on `claude.ai/settings/usage` come from a
private API that's not exposed publicly. Replicating it would require
scraping with a session cookie. `b` just opens the browser as a quick jump.

### Set the context window once via env var

If you're on Claude Code's 1M-context tier, the model name in JSONL drops the
`[1m]` suffix and ccwatch defaults to 200k until it observes >200k usage
(at which point it auto-promotes to 1M with an `auto` marker on the bar).
Skip the temporarily-wrong red bar by exporting:

```sh
# ~/.zshrc or ~/.bashrc
export CCWATCH_CONTEXT_WINDOW=1000000
```

`--context-window` on the command line takes precedence over the env var.

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

It parses `assistant` events and aggregates:
- `message.usage.{input,output,cache_creation_input,cache_read_input}_tokens`
- `message.usage.cache_creation.{ephemeral_5m,ephemeral_1h}_input_tokens`
  (split by TTL for accurate cost — 1hr writes are 2× input rate vs 1.25× for 5min)
- `message.content[].name` for `tool_use` blocks
- `cwd` (project basename for header) and `timestamp` (active detection + sliding window)

The watcher polls each tailed file every 200ms and reads only appended bytes,
so it's cheap to leave running for hours.

## Pricing

Cost is computed from a hardcoded per-model rate table (USD per million tokens)
in `crates/ccwatch/src/pricing.rs`. Adjust if Anthropic updates pricing. Cache
write rates are 1.25× input for 5min ephemeral, 2× input for 1hr ephemeral.

## Known limitations

- "Today's cumulative cost across all sessions" is not implemented. `cost:
  session` reflects the currently displayed session only.
- Context window auto-detect from model name only triggers on `[1m]` / `-1m`
  substrings; the actual JSONL stores the bare model name without that suffix
  on the 1M tier. Use `CCWATCH_CONTEXT_WINDOW=1000000` to set it explicitly.
- Bars for sessions you switched away from aren't updated (only the currently
  displayed session is tailed live).
