# claude-toolkit

[English](README.md) | [日本語](README.ja.md)

[![CI](https://github.com/vankichi/claude-toolkit/actions/workflows/ci.yml/badge.svg)](https://github.com/vankichi/claude-toolkit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/vankichi/claude-toolkit?sort=semver)](https://github.com/vankichi/claude-toolkit/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Claude Code の session を見るための小さなターミナルツール群。使用量・利用の履歴・
設定・次に見るべき session が分かる。1 つの Cargo workspace に 5 つの binary が入っている。

## Tools

| Tool | できること |
|---|---|
| [`cctop`](crates/cctop) | 下の 4 つを 1 画面にまとめた dashboard。live の使用量・利用ランキング・config map・focus board を見られる。まずはこれ。 |
| [`ccwatch`](crates/ccwatch) | 動いている session ごとの token・cost・context・tool を live で表示する。 |
| [`ccstat`](crates/ccstat) | 全 session のログを集計し、model / agent / skill / command / MCP の利用をランキングで表示する。`--watch` で今動いているものに印が付く。 |
| [`ccmap`](crates/ccmap) | 使える agent・skill・command・plugin・MCP server を一覧する。read-only。 |
| [`ccfocus`](crates/ccfocus) | 全 session の状態を記録する hook と、次に見る session を示す board。 |

## Install

### ビルド済みバイナリ (推奨)

macOS (Apple Silicon) と Linux (x86_64):

```sh
curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh | sh
```

script は最新の release をダウンロードし、`SHA256SUMS` で SHA-256 checksum を検証して、
ツールを `~/.local/bin` に入れる (古いバイナリは置き換える)。

実行前に script を読みたい場合:

```sh
curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh -o install.sh
less install.sh
sh install.sh
```

| 変数 | 既定値 | 用途 |
|---|---|---|
| `PREFIX` | `$HOME/.local/bin` | インストール先 |
| `VERSION` | 最新 release | インストールする release tag (例 `v0.1.0`)。その release に無いツールは入らない。 |

インストール先が `PATH` に通っていることを確認する:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

**Windows**: [最新 release](https://github.com/vankichi/claude-toolkit/releases/latest)
から `*-x86_64-pc-windows-msvc.zip` を取得し、`SHA256SUMS` と照合してから、
バイナリを `PATH` の通った場所へ展開する。

### ソースからビルド

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

macOS では、`make install` がコピーした binary に署名し直す
(`codesign --force --sign -`)。macOS 26 以降で起動できるようにするため。

## Quick start

`cctop`・`ccwatch`・`ccstat`・`ccmap` は `~/.claude/` 配下の Claude Code のログを
自分で読むので、設定も引数も要らない。いずれも全画面の TUI で、`q` で終了する。
どのツールも `--help` と `--version` に対応している。

### `cctop` — まずはこれ

```sh
cctop
```

1 画面に 4 つのパネルを live で出す:

| パネル | 表示内容 |
|---|---|
| **Now** `[1]` | active な全 session の合計: token・cost・context %・tokens/分のチャート。`Enter` で session ごとに分ける。 |
| **Top usage** `[2]` | 直近 30 分のランキング (model / agent / skill / command / MCP 別)。 |
| **Config map** `[3]` | agent / skill / command / plugin / MCP server の数と、今動いているもの。 |
| **Focus** `[4]` | [`ccfocus`](crates/ccfocus) の board の要約: 並行の数と上限・今日の数字・次に見る session・先頭の数 session。`Enter` で board を全画面表示、`q` / `Esc` で overview に戻る。ccfocus の hook が要る ([下記](#ccfocus))。読む場所は `$CCFOCUS_STATE_DIR`、無ければ `~/.claude/focus` (`--claude-dir` では動かない)。 |

| キー | 動作 |
|---|---|
| `1`-`4` (または `j` / `k`、`↓` / `↑`、`Tab` / `BackTab`) | パネルを選ぶ |
| `Enter` / `e` | 選んだパネルの全画面表示を開く |
| `c` | **Top usage** でカテゴリを切り替える: model → agent → skill → command → MCP |
| `/` | fuzzy filter (`Enter` で確定、`Esc` でクリア) |
| `q` / `Esc` | overview では終了。全画面表示の中では overview に戻る |

`cctop --refresh-ms 500` で再描画を 0.25 秒ごとから 0.5 秒ごとにする。
`cctop --rescan-secs 10` でログと config を 10 秒ごとに読み直す (Now は常に live)。

### 単体のツール

```sh
ccwatch          # active な session の live モニタ
ccwatch --watch  # session の一覧も自動で更新する
ccstat           # session を横断した利用ランキング (スナップショット)
ccstat --watch   # live: 自動更新と、今動いているものに spinner
ccmap            # agent / skill / command / plugin / MCP server を一覧
```

ログと config の場所は `--projects-dir`・`--claude-dir`・`--project-dir` で上書きできる
(対応しているツールのみ)。詳しくは各ツールの README にある。

### `ccfocus`

ccfocus は、Claude Code に hook として登録する設定が 1 回要る。hook は session ごとの
状態を記録する。prompt や message の本文は保存しない。session の transcript から
読むのは custom title と開始時刻だけ (詳しくは [ccfocus の README](crates/ccfocus/README.md#privacy))。

1. Claude Code の settings (例: `~/.claude/settings.json`) に、次の 7 つの event で
   `ccfocus hook <event>` を登録する。記録する `pid` が shell ではなく Claude のものに
   なるように exec form (`command` + `args`) で書き、binary は絶対 path で指す:

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

2. board を開く:

   ```sh
   ccfocus
   ```

board の画面・key・設定・container の session・外に出る情報は
[ccfocus の README](crates/ccfocus/README.md) (英語) にある。

## Development

### Releasing

`v*` tag を push すると [`.github/workflows/release.yml`](.github/workflows/release.yml)
が動く。各バイナリをそれぞれの native runner (macOS / Linux / Windows) でビルドし、
target ごとに全ツールをまとめ、`SHA256SUMS` を作って、archive を添付した
GitHub Release を公開する。

```sh
git tag v0.1.0
git push origin v0.1.0
```

### Adding a new tool

1. `crates/<new-name>/Cargo.toml` を作り、workspace から metadata を継承する
   (`edition.workspace = true` など)
2. `[lints] workspace = true` で lint の設定も継承する
3. この README **と** `README.md` の tools 表に行を足す
4. `make ci` が通ることを確認する
