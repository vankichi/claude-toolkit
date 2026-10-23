# claude-toolkit

[English](README.md) | [日本語](README.ja.md)

[![CI](https://github.com/vankichi/claude-toolkit/actions/workflows/ci.yml/badge.svg)](https://github.com/vankichi/claude-toolkit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/vankichi/claude-toolkit?sort=semver)](https://github.com/vankichi/claude-toolkit/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Claude Code を扱うための小さなターミナルツール群を集めた Cargo workspace。

## Tools

| Crate | 説明 |
|---|---|
| [`ccwatch`](crates/ccwatch) | Claude Code のセッション使用状況 (token / cost / context / tool) をリアルタイム監視する TUI。 |
| [`ccmap`](crates/ccmap) | 利用可能な agent / skill / command / plugin / MCP server を一覧する read-only TUI。 |
| [`ccstat`](crates/ccstat) | 全セッションログを横断集計し、model / agent / skill / command / MCP の利用状況をランキング表示する read-only TUI。`--watch` で実行中インジケータ付きの live 表示。 |
| [`cctop`](crates/cctop) | live 使用状況 (Now)・横断ランキング・config map を 1 画面に束ねる bottom 風統合ダッシュボード。braille ドットバー / fuzzy filter / 各ツールのフルビューへの drill-down を備える。 |
| [`ccfocus`](crates/ccfocus) | Claude Code の hook (`ccfocus hook <event>`)。全 session の状態 (idle / working / waiting / done / failed)・project 名・repo・worktree・tmux の pane を、session ごとに 1 つの JSON file として `~/.claude/focus/` に記録する。読む側は `cctk::focus` を使う。prompt や message の本文は保存しない。TUI ではなく、登録した hook から動く ([下記](#ccfocus--session-の状態を記録する-hook))。 |

## Install

### ビルド済みバイナリ (推奨)

macOS / Linux はコマンド 1 発で、最新 release のダウンロード → SHA-256 checksum
検証 → 4 ツールを `~/.local/bin` へ配置 (既存の古いバイナリは置換) まで実行:

```sh
curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh | sh
```

実行前にスクリプトを確認したい場合は、ダウンロード → 中身確認 → 実行の手順で:

```sh
curl -fsSL https://raw.githubusercontent.com/vankichi/claude-toolkit/main/install.sh -o install.sh
less install.sh
sh install.sh
```

環境変数:

| 変数 | default | 用途 |
|---|---|---|
| `PREFIX` | `$HOME/.local/bin` | インストール先 |
| `VERSION` | 最新 release | インストールする release tag (例 `v0.1.0`) |

インストール先が `PATH` に通っていることを確認:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

**Windows**: [最新 release](https://github.com/vankichi/claude-toolkit/releases/latest)
から `*-x86_64-pc-windows-msvc.zip` を取得し、`SHA256SUMS` と照合のうえ、
バイナリを `PATH` の通った場所へ展開。

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

`~/.local/bin` が PATH に通っていれば `make install` 後すぐ叩ける。通っていなければ
上記 export を `~/.zshrc` 等に追加。

## 使い方

TUI のツール (`cctop`・`ccwatch`・`ccstat`・`ccmap`) は `~/.claude/` 配下の
Claude Code ログを自動で読むので、引数なしで起動するだけ。いずれも全画面 TUI で、
`q` で終了。`ccfocus` だけは違い、Claude Code が hook として呼ぶので、最初に 1 回
登録する ([下記](#ccfocus--session-の状態を記録する-hook))。

### `cctop` — 全部入りダッシュボード

まずはこれ。1 画面に 3 パネルを live 表示:

```sh
cctop
```

| パネル | 表示内容 |
|---|---|
| **Now** `[1]` | active な全セッションを合算 — 合計 token / cost / context % + tokens/分のリアルタイムチャート。`Enter` でセッション別に分解。 |
| **Top usage** `[2]` | 直近 30 分の横断ランキング (model / agent / skill / command / MCP 別)。 |
| **Config map** `[3]` | agent / skill / command / plugin / MCP の数と、今まさに動いているもの。 |

キー操作:

| キー | 動作 |
|---|---|
| `1` `2` `3` | パネル選択 (`j` / `k` / `Tab` でも移動) |
| `Enter` | 選択中パネルのフルビューへ drill-down |
| `c` | **Top usage** でカテゴリ切替: model → agent → skill → command → MCP |
| `/` | fuzzy filter (入力で絞り込み、`Enter` で確定、`Esc` でクリア) |
| `Esc` | drill-down から戻る |
| `q` | 終了 |

調整 (全 flag は `cctop --help`):

```sh
cctop --refresh-ms 500     # 再描画を 0.25s default → 0.5s ごとに
cctop --rescan-secs 10     # ログ / config の再スキャンを 10s ごとに (Now は常時 live)
```

### 個別ツール

1 つのビューに集中したいとき用に、各パネルは単体ツールとしても使える:

```sh
ccwatch          # active セッションの live モニタ
ccwatch --watch  # セッション一覧を自動更新し続ける

ccstat           # 横断使用状況ランキング (スナップショット)
ccstat --watch   # live モード: 自動更新 + 実行中アイテムに spinner

ccmap            # agent / skill / command / plugin / MCP server を一覧
```

どのツールも `--help` (全 flag) と `--version` に対応。ログ / config の場所は
`--projects-dir` / `--claude-dir` / `--project-dir` で上書き可 (該当ツールのみ)。

### `ccfocus` — session の状態を記録する hook

`ccfocus hook <event>` は、session ごとの状態 (`idle` / `working` / `waiting` /
`done` / `failed`) を `~/.claude/focus/sessions/<session_id>.json` に記録し、
event ごとに 1 行を `~/.claude/focus/events/<YYYYMMDD>.tsv` に追記する。prompt・
message・tool の本文は保存しない。場所は `--state-dir` か `CCFOCUS_STATE_DIR` で
変えられる。

Claude Code の settings (例: `~/.claude/settings.json`) に、次の 7 つの event で
登録する。記録する `pid` が shell ではなく Claude のものになるように exec form
(`command` + `args`) で書き、binary は絶対 path で指す:

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

hook は常に exit 0 で終わり、stdout には何も出さないので、失敗しても Claude を
止めない。問題は stderr に 1 行で出る。

## Releasing

`v*` tag を push すると [`.github/workflows/release.yml`](.github/workflows/release.yml)
が発火し、各バイナリをそれぞれの native runner (macOS / Linux / Windows) で
ビルド → target ごとに全ツールを bundle 化 → `SHA256SUMS` を生成し、
archive を添付した GitHub Release を作成する。

```sh
git tag v0.1.0
git push origin v0.1.0
```

## Adding a new tool

1. `crates/<new-name>/Cargo.toml` を作成し、workspace から各種 metadata を継承
   (`edition.workspace = true` 等)
2. `[lints] workspace = true` で lint 設定も継承
3. この README **と** `README.md` の tools 表に行を追加
4. `make ci` が通ることを確認

`/add-rust-crate` skill が雛形を出すので、それを使えば手作業が減る。
