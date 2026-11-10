# claude-toolkit

[English](README.md) | [日本語](README.ja.md)

[![CI](https://github.com/vankichi/claude-toolkit/actions/workflows/ci.yml/badge.svg)](https://github.com/vankichi/claude-toolkit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/vankichi/claude-toolkit?sort=semver)](https://github.com/vankichi/claude-toolkit/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Small terminal tools for watching your Claude Code sessions: usage, history,
configuration, and which session needs you next. One Cargo workspace, five
binaries.

## Tools

| Tool | What it does |
|---|---|
| [`cctop`](crates/cctop) | One dashboard with all of the below: live usage, the usage ranking, the config map, and the focus board. Start here. |
| [`ccwatch`](crates/ccwatch) | Live tokens, cost, context, and tools for each running session. |
| [`ccstat`](crates/ccstat) | Ranking of model / agent / skill / command / MCP usage across all your session logs. `--watch` marks what is running now. |
| [`ccmap`](crates/ccmap) | Browse the agents, skills, commands, plugins, and MCP servers you can use. Read-only. |
| [`ccfocus`](crates/ccfocus) | A hook that records every session's status, and a board that shows which session to look at next. |

## Install

### Prebuilt binaries (recommended)

macOS (Apple Silicon) and Linux (x86_64):

```sh
curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh | sh
```

The script downloads the latest release, verifies its SHA-256 checksum against
`SHA256SUMS`, and installs the tools into `~/.local/bin`, replacing older
copies.

To read the script before running it:

```sh
curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh -o install.sh
less install.sh
sh install.sh
```

| Variable | Default | Purpose |
|---|---|---|
| `PREFIX` | `$HOME/.local/bin` | Install destination |
| `VERSION` | latest release | Release tag to install, e.g. `v0.1.0`. A tool that is not in that release is skipped. |

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

On macOS, `make install` re-signs each copied binary
(`codesign --force --sign -`) so that macOS 26 and later will run it.

## Quick start

`cctop`, `ccwatch`, `ccstat`, and `ccmap` read your Claude Code logs under
`~/.claude/` by themselves: run them with no setup or arguments. Each is a
full-screen TUI; `q` quits. Every tool accepts `--help` and `--version`.

### `cctop` — start here

```sh
cctop
```

One screen, four live panels:

| Panel | What it shows |
|---|---|
| **Now** `[1]` | Every active session added up: tokens, cost, context %, and a tokens/min chart. `Enter` splits it per session. |
| **Top usage** `[2]` | Ranking over the last 30 min per model / agent / skill / command / MCP. |
| **Config map** `[3]` | How many agents / skills / commands / plugins / MCP servers you have, and which are running now. |
| **Focus** `[4]` | The [`ccfocus`](crates/ccfocus) board in brief: parallel sessions against the limit, today's numbers, the next session to look at, and the first few sessions. `Enter` opens the full board; `q` / `Esc` returns to the overview. Needs the ccfocus hook ([below](#ccfocus)). It reads `$CCFOCUS_STATE_DIR`, else `~/.claude/focus` (`--claude-dir` does not move it). |

| Key | Action |
|---|---|
| `1`-`4` (or `j` / `k`, `↓` / `↑`, `Tab` / `BackTab`) | Select a panel |
| `Enter` / `e` | Open the selected panel's full view |
| `c` | On **Top usage**, cycle model → agent → skill → command → MCP |
| `/` | Fuzzy filter (`Enter` applies, `Esc` clears) |
| `q` / `Esc` | Quit on the overview; in a full view, back to the overview |

`cctop --refresh-ms 500` redraws every 0.5 s instead of 0.25 s.
`cctop --rescan-secs 10` re-scans logs and config every 10 s (Now stays live).

### The single-view tools

```sh
ccwatch          # live monitor of your active sessions
ccwatch --watch  # also refresh the session list automatically
ccstat           # usage ranking across sessions (one snapshot)
ccstat --watch   # live: auto-refresh and a spinner on what is running now
ccmap            # browse agents / skills / commands / plugins / MCP servers
```

`--projects-dir`, `--claude-dir`, and `--project-dir` override the log and
config locations where a tool supports them. Each tool's README has the rest.

### `ccfocus`

ccfocus needs one setup step: Claude Code must run it as a hook. The hook
records each session's status; prompt and message text are never stored.
From the session's transcript it reads only the custom title and the start
time (details in the [ccfocus README](crates/ccfocus/README.md#privacy)).

1. Register `ccfocus hook <event>` for these seven events in your Claude Code
   settings (for example `~/.claude/settings.json`). Use the exec form
   (`command` + `args`) so the recorded `pid` is Claude's rather than a
   wrapping shell's, and give the absolute path to the binary:

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

2. Open the board:

   ```sh
   ccfocus
   ```

The board's layout, keys, settings, container sessions, and what leaves your
machine are in the [ccfocus README](crates/ccfocus/README.md).

## Development

### Releasing

Pushing a `v*` tag runs [`.github/workflows/release.yml`](.github/workflows/release.yml).
It builds every binary on its native runner (macOS / Linux / Windows), bundles
the tools per target, generates `SHA256SUMS`, and publishes a GitHub Release
with the archives attached.

```sh
git tag v0.1.0
git push origin v0.1.0
```

### Adding a new tool

1. Create `crates/<new-name>/Cargo.toml`, inheriting workspace metadata
   (`edition.workspace = true`, etc.)
2. Inherit lints with `[lints] workspace = true`
3. Add a row to the tools table in this README **and** `README.ja.md`
4. Confirm `make ci` passes
