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

## Install / run

```sh
make install                  # → ~/.local/bin/ccstat
ccstat                        # aggregate ~/.claude/projects
ccstat --projects-dir /path   # override the projects directory
```

## Known limitations

- **Snapshot, not live.** Press `R` to re-read logs (live monitoring is
  `ccwatch`'s job).
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
