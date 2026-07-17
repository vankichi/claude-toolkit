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
| [`ccstat`](crates/ccstat) | Read-only TUI ranking model/agent/skill/command/MCP usage aggregated across all session logs. |

## Install

### Prebuilt binaries (recommended)

macOS / Linux — one command downloads the latest release, verifies its SHA-256
checksum, and installs all three tools into `~/.local/bin`, replacing any older
copies:

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

## Releasing

Pushing a `v*` tag triggers [`.github/workflows/release.yml`](.github/workflows/release.yml),
which builds every binary on its native runner (macOS / Linux / Windows),
bundles the three tools per target, generates `SHA256SUMS`, and publishes a
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
