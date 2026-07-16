# ccmap

Terminal explorer for the Claude Code agents, skills, commands, plugins, and
MCP servers you can actually use right now.

`ccmap` scans the definitions scattered across `~/.claude/`, the current
project's `.claude/`, every enabled plugin, and `~/.claude.json`, then lists
them in one place — name, description, definition-file location, provenance,
and plugin/enable state — so you don't have to `grep`/`find` frontmatter by
hand. It is **read-only**: it never installs, enables, edits, or deletes
anything (`e` only launches your `$EDITOR`).

## Display

```
┌ ccmap ──────────────────────────────────────────────────────────────────────┐
│  Agents   Skills   Commands   Plugins   MCP                                   │
├───────────────────────────────┬───────────────────────────────────────────────┤
│ / rev                         │┌ provenance ──────────────────────────────────┐│
│ ● self-review-changes         ││ ★ Official                                   ││
│ ● api-design-review           ││ superpowers @ claude-plugins-official        ││
│ ★ superpowers:requesting-cod… │└──────────────────────────────────────────────┘│
│ ★ superpowers:receiving-code… │┌ detail ──────────────────────────────────────┐│
│ ✦ superpowers:brainstorming   ││ Use when completing tasks or before merging… ││
│ ◆ my-project-skill            ││                                              ││
│                               ││ Path: ~/.claude/plugins/cache/…/SKILL.md     ││
│                               ││ allowed-tools: Read, Grep, Bash              ││
│                               │└──────────────────────────────────────────────┘│
└───────────────────────────────┴───────────────────────────────────────────────┘
 q quit · Tab/⇧Tab tabs · j/k select · / filter (Esc clear) · e edit · y copy · R rescan
```

- **Top tabs** — one category each: `Agents` / `Skills` / `Commands` /
  `Plugins` / `MCP`. `Tab` / `BackTab` cycles between them.
- **Left list** — the items in the current tab, each prefixed with a
  provenance icon and colored by provenance. Selection moves with `j`/`k` or
  the arrow keys; the right side updates instantly (no explicit "open").
- **Provenance panel** (top right) — where the selected item comes from: the
  provenance label and its origin (`~/.claude`, `<project>/.claude`, or
  `<plugin> @ <marketplace>`).
- **Detail panel** (bottom right) — full description, `Path` (if the item has
  a backing file), a `State` badge for plugins, and extra frontmatter rows
  (`tools` / `allowed-tools` / `model`) for agents and commands.

### Provenance

Each item is classified into one of four origins, shown as an icon + color in
the list and spelled out in the provenance panel:

| Icon | Color | Provenance | Source |
|---|---|---|---|
| `●` | white | Local | `~/.claude/{agents,skills,commands}` |
| `◆` | cyan | Project | `<project>/.claude/{agents,skills,commands}` |
| `★` | yellow | Official | plugin from the `claude-plugins-official` marketplace |
| `✦` | magenta | Community | plugin from any other marketplace |

The focus highlight is a **green background** (with black text) so the selected
row stays legible regardless of its provenance color — the icon still marks the
provenance on the highlighted row.

## Install

From the workspace root:

```sh
make install                            # → ~/.local/bin/ccmap
make install PREFIX=/opt/homebrew/bin   # → /opt/homebrew/bin/ccmap
```

`make install` runs a release build (with LTO + `panic=abort`) once, then copies
the resulting binary to `$(PREFIX)`. Default `PREFIX` is `~/.local/bin`. On
macOS the binary is re-signed (`codesign --force --sign -`) after copy to
satisfy macOS 26+ hardened-runtime checks.

To remove: `make uninstall PREFIX=<same-dir>`.

## Run

```sh
ccmap                                   # browse ~/.claude + the current project
ccmap --claude-dir /custom/.claude      # override ~/.claude
ccmap --project-dir /path/to/repo       # override the project scanned for .claude / .mcp.json
```

Keys:

| Key | Action |
|---|---|
| `Tab` / `BackTab` | next / previous category tab |
| `j` / `↓`, `k` / `↑` | select next / previous item |
| `/` | start an incremental case-insensitive substring filter over name + description (within the current tab) |
| `Esc` (while filtering) | clear the filter and leave filter mode |
| `Enter` (while filtering) | keep the filter, leave edit mode |
| `e` | open the selected item's definition file in `$EDITOR` (falls back to `vi`); disabled for MCP (no backing file) |
| `y` | copy the selected item's invocation string to the clipboard |
| `R` (capital) | rescan all sources |
| `q` / `Esc` (when not filtering) | quit |

### Copy (`y`) formats

`y` copies a paste-ready invocation string, shaped per kind:

| Kind | Copied string |
|---|---|
| Agent | `code-refactor-advisor` (bare name) |
| Skill | `/api-design-review` — plugin-provided: `/superpowers:brainstorming` |
| Command | `/my-command` — plugin-provided: `/plugin:my-command` |
| Plugin | `superpowers@claude-plugins-official` |
| MCP | the server name |

The clipboard write shells out to `pbcopy` (macOS) or `xclip -selection
clipboard` (Linux) via stdin; on other platforms it is a no-op.

## Data source

`ccmap` reads only local files and extracts names/metadata — never secrets:

| Kind | Where it looks |
|---|---|
| Agent / Skill / Command | `~/.claude/{agents,skills,commands}` and `<project>/.claude/…`; symlinked directories are followed. Skills are `<name>/SKILL.md`; agents/commands are `<name>.md`. |
| Plugin | `~/.claude/plugins/known_marketplaces.json` + `settings.json`'s `extraKnownMarketplaces` → each marketplace's `.claude-plugin/marketplace.json` (Available), merged with `installed_plugins.json` (Installed) and `enabledPlugins` (Enabled). |
| Plugin-provided Agent/Skill/Command | scanned from each **enabled** plugin's install path (installed-but-not-enabled plugins do not contribute here). |
| MCP | server **names only** from `~/.claude.json` (top-level `mcpServers` = user scope, and the current project entry's `mcpServers` = local scope) plus `<project>/.mcp.json` (project scope). `enabledMcpjsonServers` / `disabledMcpjsonServers` apply only to the `.mcp.json` set. |

`~/.claude.json` holds OAuth tokens and MCP command/env values. `ccmap`
deserializes it through a partial typed view whose `mcpServers` values are
discarded on parse (`serde::de::IgnoredAny`); `oauthAccount` and any
command/args/env/token value is never read into memory or printed.

Frontmatter is parsed by a small built-in flat `key: value` reader (no YAML
dependency). Broken or missing frontmatter falls back to `name = file stem`,
`description = "(no description)"`.

## Known limitations

- **Empty tabs are normal.** With no `~/.claude/commands` directory the
  Commands tab is empty; with no MCP servers configured for the current
  project the MCP tab is empty.
- **No name-collision resolution.** If the same name exists as both a user and
  a project (or plugin) item, both are shown with their own provenance — ccmap
  does not decide which one Claude Code would actually pick.
- **claude.ai connectors are out of scope.** MCP servers provided by claude.ai
  connectors (Slack / Notion / Gmail, …) aren't reachable from local files and
  are not listed.
- **Manual refresh.** There is no live polling — press `R` to pick up changes.
- **Frontmatter is assumed flat.** Valid-but-non-flat YAML (block scalars, list
  values) can be mis-rendered rather than erroring; the flat reader matches the
  frontmatter shape observed across current agents/skills/commands.
