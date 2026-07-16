# claude-toolkit

A Cargo workspace of small terminal tools for working with Claude Code.

## Tools

| Crate | Description |
|---|---|
| [`ccwatch`](crates/ccwatch) | Real-time TUI monitor for Claude Code session usage (tokens, cost, context, tools). |
| [`ccmap`](crates/ccmap) | Read-only TUI to browse available agents, skills, commands, plugins, and MCP servers. |

## Build & install

```sh
make help                # 全レシピ一覧
make build               # debug build
make release             # release build (lto=fat / panic=abort)
make test                # cargo test --workspace
make ci                  # fmt-check + clippy + test
make install             # release ビルド → ~/.local/bin にコピー
make install PREFIX=/opt/homebrew/bin   # install 先を変える
make uninstall           # 各 binary を $(PREFIX) から削除
```

`~/.local/bin` が PATH に通っていれば `make install` 後すぐ叩ける。通っていなければ
`~/.zshrc` 等に以下を追加:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

## Adding a new tool

1. Create `crates/<new-name>/Cargo.toml`、workspace から各種 metadata を継承
   (`edition.workspace = true` 等)
2. `[lints] workspace = true` で lint 設定も継承
3. このルート README の tools 表に行を追加
4. `make ci` が通ることを確認

`/add-rust-crate` skill が雛形を出すので、それを使えば手作業が減る。
