# ccmap

Terminal explorer for the Claude Code agents, skills, commands, plugins, and
MCP servers you can use right now.

`ccmap` scans the definitions spread across `~/.claude/`, the current
project's `.claude/`, every enabled plugin, and `~/.claude.json`, and lists
them in one place: name, description, definition file, where it comes from,
and plugin / enable state. You no longer `grep` / `find` frontmatter by hand.
It is **read-only**: it never installs, enables, edits, or deletes anything
(`e` only launches your `$EDITOR`).

Install it with the rest of the toolkit (see the [top README](../../README.md#install)).

## Run

```sh
ccmap                                   # browse ~/.claude + the current project
ccmap --claude-dir /custom/.claude      # override ~/.claude
ccmap --project-dir /path/to/repo       # override the project scanned for .claude / .mcp.json
```

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

- **Tabs** (top): `Agents` / `Skills` / `Commands` / `Plugins` / `MCP`.
- **List** (left): the items in the current tab, each with a provenance icon
  and color. The right side follows the selection at once.
- **Provenance** (top right): where the selected item comes from — the label
  and its origin (`~/.claude`, `<project>/.claude`, or
  `<plugin> @ <marketplace>`).
- **Detail** (bottom right): the full description, `Path` (if the item has a
  file), a `State` badge for plugins, and extra frontmatter rows (`tools` /
  `allowed-tools` / `model`) for agents and commands.

### Keys

| Key | Action |
|---|---|
| `Tab` / `BackTab` | Next / previous tab |
| `j` / `↓`, `k` / `↑` | Select next / previous item |
| `/` | Filter the current tab by name and description (incremental, case-insensitive substring) |
| `Enter` (while filtering) | Keep the filter and stop editing it |
| `Esc` (while filtering) | Clear the filter and stop filtering |
| `e` | Open the item's definition file in `$EDITOR` (falls back to `vi`); not for MCP (no file) |
| `y` | Copy the item's invocation string to the clipboard |
| `R` (capital) | Rescan all sources |
| `q` / `Esc` (when not filtering) | Quit |

`y` copies a string you can paste as is:

| Kind | Copied string |
|---|---|
| Agent | `code-refactor-advisor` (bare name) |
| Skill | `/api-design-review` — plugin-provided: `/superpowers:brainstorming` |
| Command | `/my-command` — plugin-provided: `/plugin:my-command` |
| Plugin | `superpowers@claude-plugins-official` |
| MCP | the server name |

The copy pipes the string to `pbcopy` (macOS) or `xclip -selection clipboard`
(Linux); on other platforms it does nothing.

## Provenance

| Icon | Color | Provenance | Source |
|---|---|---|---|
| `●` | white | Local | `~/.claude/{agents,skills,commands}` |
| `◆` | cyan | Project | `<project>/.claude/{agents,skills,commands}` |
| `★` | yellow | Official | plugin from the `claude-plugins-official` marketplace |
| `✦` | magenta | Community | plugin from any other marketplace |

The selected row has a **green background** with black text so it stays
readable whatever its provenance color; the icon still shows the provenance.

## Data source

`ccmap` reads only local files and takes names and metadata — never secrets:

| Kind | Where it looks |
|---|---|
| Agent / Skill / Command | `~/.claude/{agents,skills,commands}` and `<project>/.claude/…`; symlinked directories are followed. Skills are `<name>/SKILL.md`; agents and commands are `<name>.md`. |
| Plugin | `~/.claude/plugins/known_marketplaces.json` + `settings.json`'s `extraKnownMarketplaces` → each marketplace's `.claude-plugin/marketplace.json` (Available), merged with `installed_plugins.json` (Installed) and `enabledPlugins` (Enabled). |
| Plugin-provided Agent / Skill / Command | Each **enabled** plugin's install path (installed but not enabled plugins add nothing here). |
| MCP | Server **names only** from `~/.claude.json` (top-level `mcpServers` = user scope, and the current project entry's `mcpServers` = local scope) plus `<project>/.mcp.json` (project scope). `enabledMcpjsonServers` / `disabledMcpjsonServers` apply only to the `.mcp.json` set. |

`~/.claude.json` holds OAuth tokens and MCP command / env values. `ccmap` reads
it through a partial typed view whose `mcpServers` values are discarded on
parse (`serde::de::IgnoredAny`); `oauthAccount` and any command / args / env /
token value is never read into memory or printed.

Frontmatter is read by a small built-in flat `key: value` reader (no YAML
dependency). Broken or missing frontmatter falls back to `name = file stem`
and `description = "(no description)"`.

## Known limitations

- **Empty tabs are normal.** With no `~/.claude/commands` directory the
  Commands tab is empty; with no MCP servers configured for the current project
  the MCP tab is empty.
- **No name-collision resolution.** If the same name exists as a user item and a
  project (or plugin) item, both are shown with their own provenance; ccmap
  does not decide which one Claude Code would pick.
- **claude.ai connectors are out of scope.** MCP servers provided by claude.ai
  connectors (Slack / Notion / Gmail, …) are not in local files and are not
  listed.
- **Manual refresh.** There is no live polling; press `R` to pick up changes.
- **Frontmatter is assumed flat.** Valid but non-flat YAML (block scalars, list
  values) can be shown wrong rather than raising an error; the flat reader
  matches the frontmatter seen in current agents, skills, and commands.
