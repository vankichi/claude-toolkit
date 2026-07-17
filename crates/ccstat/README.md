# ccstat

Terminal dashboard of how much you actually use each Claude Code **model,
agent, skill, command, and MCP server** — aggregated across every local
session log.

Where [`ccmap`](../ccmap) shows what you *can* use and [`ccwatch`](../ccwatch)
shows live token/cost for the *current* session, `ccstat` answers the
historical question: *what have I been using, how often, when last, and in
which project?* It is **read-only**: it reads your session logs under
`~/.claude/projects/**/*.jsonl`, and — to color agent/skill/command rows by
where each item comes from — the extension and plugin metadata under
`~/.claude` that [`ccmap`](../ccmap) also scans (`agents/`, `skills/`,
`commands/`, and plugin catalogs). It never reads `~/.claude.json`, never
writes anything, and from the logs it extracts only names, counts, token
totals, and timestamps — not message content or secrets.

## Display

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

## Keys

| Key | Action |
|---|---|
| `Tab` / `BackTab` | next / previous category tab |
| `j` / `k` (↓/↑) | move selection |
| `w` | cycle time window (all / 30d / 7d, default 7d) — scopes count/tokens/cost/ranking |
| `s` | cycle sort (count / recency / name) |
| `p` | open the project filter picker (`j`/`k`, `Enter` apply, `Esc` cancel) |
| `g` | toggle a full-screen daily bar chart of the selected row (30-day) |
| `/` | filter rows by name (Esc clears) |
| `R` | rescan logs |
| `q` / `Esc` | quit |

## Provenance colors

Agent/Skill/Command rows are colored by where the item was discovered (via
[`ccmap`](../ccmap)'s extension discovery), mirroring `ccmap`'s own palette:

| Source | Color |
|---|---|
| Local (`~/.claude`) | white |
| Project | cyan |
| Official plugin | yellow |
| Community plugin | magenta |
| built-in / unknown | gray |

The Model tab uses model-family colors instead, and MCP rows are uncolored.

## What each column means

- **count / bar** — invocations within the selected window and project.
- **recency (`last`)** — when the item was last used (all-time, within the
  project filter).
- **trend** — a 30-day daily sparkline of activity (independent of the window).
- **Model tab only** — token breakdown and estimated cost (Anthropic public
  rate card; a snapshot, not a bill).

## How usage is derived

| Category | Source in the JSONL |
|---|---|
| Model | `assistant.message.model` (+ token `usage` → cost) |
| Agent | `tool_use` `Agent` / any `input.subagent_type` (unspecified → `(default)`) |
| Skill | `tool_use` `Skill` → `input.skill` |
| Command | `<command-name>/x</command-name>` markers in user messages |
| MCP | `tool_use` names prefixed `mcp__<server>__…`, aggregated by server |

`Command` and `Skill` overlap by design: `/grill-me` counts as a command
(you typed it) *and* a skill (it ran). MCP servers from claude.ai connectors
show up here (unlike in `ccmap`) because their tool calls appear in the log.

## Live mode (`--watch`)

`ccstat --watch` keeps the dashboard fresh instead of taking a one-shot
snapshot:

- **Auto-refresh.** The full aggregate re-reads every 5 minutes (override the
  interval with `--watch <seconds>`), the same work as pressing `R`.
- **Running indicator.** A Braille spinner (`⠋⠙⠹…`) animates next to every
  model/agent/skill/command/MCP server that is *running now*, and a summary
  line under the tabs lists them across all categories plus the live-session
  count. Idle rows show no spinner; when nothing is active the line reads
  `○ idle`.

An item counts as "running" when it appears on a log line — in a session whose
file was modified within the last 90 seconds — carrying a timestamp inside that
same 90-second window. This is a heuristic over the logs, **not** a hook into a
live process: a skill/agent/command invocation is an instantaneous log event,
so the spinner reflects *recent activity in a live session*, not a guarantee
that something is still executing. Snapshot mode (no `--watch`) is unchanged.

## Install / run

```sh
make install                  # → ~/.local/bin/ccstat
ccstat                        # aggregate ~/.claude/projects (snapshot)
ccstat --projects-dir /path   # override the projects directory
ccstat --watch                # live mode: spinner on running items + 5-min auto-refresh
ccstat --watch 60             # live mode with a 60-second refresh interval
```

## Known limitations

- **Snapshot by default.** Without `--watch`, press `R` to re-read logs. The
  `--watch` live mode auto-refreshes and shows a running indicator, but
  per-session token/cost monitoring remains `ccwatch`'s job.
- **Running indicator is mtime + tail based.** Only sessions whose log changed
  in the last 90 seconds are inspected, and only their last 16 KiB is read; a
  burst of activity older than the window (or a session paused mid-run longer
  than 90s) shows no spinner.
- **No cost attribution beyond models.** Agents/skills/commands/MCP show
  counts only — attributing tokens to them is inherently fuzzy.
- **Per-project by cwd basename.** Projects are keyed by the last path
  component of the session's working directory. Git worktrees with distinct
  paths appear as separate projects; conversely, two different repos that
  share a directory name (e.g. `/work/api` and `/personal/api`) are merged
  under one `api` entry.
- **Historical items stay listed at count 0 in a narrow window.** In the 7d
  and 30d views, an item with no activity in the window still appears (count
  0, empty bar) so its all-time recency stays visible; it sinks to the bottom
  under count sort.
- **MCP is server-level.** Per-tool breakdown is not shown in v1.
- **Day buckets are UTC.** Events (and `today`) are keyed by UTC date, so for
  users far from UTC a late-evening local session can land on the next UTC
  day, shifting counts/recency near day boundaries.
