# ccstat

Terminal dashboard of how much you use each Claude Code **model, agent, skill,
command, and MCP server**, aggregated across every local session log.

[`ccmap`](../ccmap) shows what you *can* use, and [`ccwatch`](../ccwatch) shows
live tokens and cost for the *current* sessions. `ccstat` answers the
historical question: *what have I been using, how often, when last, and in
which project?*

It is **read-only**. It reads your session logs under
`~/.claude/projects/**/*.jsonl`, and the extension and plugin metadata under
`~/.claude` that [`ccmap`](../ccmap) also scans (`agents/`, `skills/`,
`commands/`, and plugin catalogs) to color rows by where each item comes from.
It never reads `~/.claude.json`, never writes anything, and takes only names,
counts, token totals, and timestamps from the logs — not message content or
secrets.

Install it with the rest of the toolkit (see the [top README](../../README.md#install)).

## Run

```sh
ccstat                        # aggregate ~/.claude/projects (snapshot)
ccstat --projects-dir /path   # override the projects directory
ccstat --watch                # live mode: spinner on running items + 5-min auto-refresh
ccstat --watch 60             # live mode with a 60-second refresh interval
ccstat --watch 0              # live mode off (same as no --watch)
```

```
┌ ccstat · window: 7d · project: (all) · sort: count ─────────────────┐
│  Model   Agents   Skills   Commands   MCP                           │
├──────────────────────────────┬──────────────────────────────────────┤
│    24  ██████████  brainstorm │ Skills: brainstorm                   │
│    11  ████·······  improve-… │ count 24 · last today · first 6/12   │
│     8  ███·······  artifact-… │ trend (30d): ▁▂▅▇▆▃▂▁▁▃▅▇…           │
│                               │ by project: claude-toolkit 14 · …    │
└──────────────────────────────┴──────────────────────────────────────┘
 q quit · Tab tabs · j/k move · w window · s sort · p project · g graph · / filter · R rescan
```

### Keys

| Key | Action |
|---|---|
| `Tab` / `BackTab` | Next / previous category tab |
| `j` / `k` (↓/↑) | Move the selection |
| `w` | Cycle the time window (all / 30d / 7d, default 7d); it scopes count, tokens, cost, and the ranking |
| `s` | Cycle the sort (count / recency / name) |
| `p` | Open the project filter (`j`/`k`, `Enter` applies, `Esc` cancels) |
| `g` | Toggle a full-screen 30-day daily bar chart of the selected row |
| `m` | Switch the Trends graph between tokens, cost, and messages |
| `/` | Filter rows by name (`Esc` clears) |
| `R` | Rescan the logs |
| `q` / `Esc` | Quit |

## What the screen shows

- **count / bar**: uses within the selected window and project.
- **`last`**: when the item was last used (all time, within the project filter).
- **trend**: a 30-day daily sparkline (independent of the window).
- **Model tab only**: the token breakdown and an estimated cost (from
  Anthropic's public rate card; a snapshot, not a bill).

Agent, skill, and command rows are colored by where the item was found (through
[`ccmap`](../ccmap)'s discovery), with `ccmap`'s palette:

| Source | Color |
|---|---|
| Local (`~/.claude`) | white |
| Project | cyan |
| Official plugin | yellow |
| Community plugin | magenta |
| built-in / unknown | gray |

The Model tab uses model-family colors instead, and MCP rows are not colored.

## How usage is counted

| Category | Source in the JSONL |
|---|---|
| Model | `assistant.message.model` (+ token `usage` → cost) |
| Agent | `tool_use` `Agent` / any `input.subagent_type` (unspecified → `(default)`) |
| Skill | `tool_use` `Skill` → `input.skill` |
| Command | `<command-name>/x</command-name>` markers in user messages |
| MCP | `tool_use` names prefixed `mcp__<server>__…`, aggregated by server |

`Command` and `Skill` overlap by design: `/grill-me` counts as a command (you
typed it) *and* a skill (it ran). MCP servers from claude.ai connectors show up
here (unlike in `ccmap`) because their tool calls appear in the log.

## Live mode (`--watch`)

- **Auto-refresh.** The whole aggregate is re-read every 5 minutes (or every
  `--watch <seconds>`), the same work as pressing `R`.
- **Running indicator.** A Braille spinner (`⠋⠙⠹…`) animates next to every
  model, agent, skill, command, and MCP server that is *running now*. A line
  under the tabs lists them across all categories with the live-session count;
  with nothing active it reads `○ idle`.

An item counts as "running" when it appears on a log line with a timestamp in
the last 90 seconds, in a session whose file changed in the last 90 seconds.
This is a heuristic over the logs, **not** a hook into a live process: a skill,
agent, or command call is a single log event, so the spinner means *recent
activity in a live session*, not that something is still executing. Snapshot
mode (no `--watch`) is unchanged.

## Known limitations

- **Snapshot by default.** Without `--watch`, press `R` to re-read the logs.
  Per-session token and cost monitoring stays `ccwatch`'s job.
- **The running indicator reads mtime and the tail.** Only sessions whose log
  changed in the last 90 seconds are inspected, and only their last 16 KiB is
  read; activity older than that window (or a session paused mid-run for more
  than 90s) shows no spinner.
- **Cost is for models only.** Agents, skills, commands, and MCP show counts
  only; attributing tokens to them is inherently fuzzy.
- **Projects are keyed by the cwd's basename.** Git worktrees with different
  paths show up as separate projects, and two repos with the same directory
  name (e.g. `/work/api` and `/personal/api`) merge into one `api` entry.
- **Old items stay listed at count 0 in a narrow window.** In the 7d and 30d
  views, an item with no activity in the window still appears (count 0, empty
  bar) so its all-time recency stays visible; it sinks to the bottom under the
  count sort.
- **MCP is per server.** There is no per-tool breakdown in v1.
- **Days are UTC.** Events (and `today`) are keyed by UTC date, so for users far
  from UTC a late-evening local session can land on the next UTC day, shifting
  counts and recency near day boundaries.
