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
| [`cctop`](crates/cctop) | live 使用状況 (Now)・横断ランキング・config map・Focus panel (`ccfocus` の board) を 1 画面に束ねる bottom 風統合ダッシュボード。braille ドットバー / fuzzy filter / 各ツールのフルビューへの drill-down を備える。 |
| [`ccfocus`](crates/ccfocus) | Claude Code の hook (`ccfocus hook <event>`)。全 session の状態 (idle / working / waiting / done / failed)・project 名・repo・worktree・tmux の pane を、session ごとに 1 つの JSON file として `~/.claude/focus/` に記録する。読む側は `cctk::focus` を使う。prompt や message の本文は保存しない。TUI でもある: `ccfocus` (引数なし) か `ccfocus tui` で focus board が開き、次に見る session・全 session の一覧・終わらせる worktree・今日の数字を表示する ([下記](#ccfocus--focus-board-と-session-の状態を記録する-hook))。 |

## Install

### ビルド済みバイナリ (推奨)

macOS / Linux はコマンド 1 発で、最新 release のダウンロード → SHA-256 checksum
検証 → その release に含まれるツールを `~/.local/bin` へ配置 (既存の古いバイナリは
置換) まで実行する。`VERSION` で古い release を指定すると、その時点に無いツールは
入らない:

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
`q` で終了。`ccfocus` は hook でもあり TUI でもある。Claude Code が hook として
呼ぶので最初に 1 回登録し、開く focus board はその hook が記録した内容から作る
([下記](#ccfocus--focus-board-と-session-の状態を記録する-hook))。

### `cctop` — 全部入りダッシュボード

まずはこれ。1 画面に 4 パネルを live 表示:

```sh
cctop
```

| パネル | 表示内容 |
|---|---|
| **Now** `[1]` | active な全セッションを合算 — 合計 token / cost / context % + tokens/分のリアルタイムチャート。`Enter` でセッション別に分解。 |
| **Top usage** `[2]` | 直近 30 分の横断ランキング (model / agent / skill / command / MCP 別)。 |
| **Config map** `[3]` | agent / skill / command / plugin / MCP の数と、今まさに動いているもの。 |
| **Focus** `[4]` | [`ccfocus`](#ccfocus--focus-board-と-session-の状態を記録する-hook) の board の要約 — 並行の数と上限・今日の数字・次に見る 1 件・先頭の数 session。`Enter` で board を全画面表示、`q` / `Esc` で overview に戻る。読む場所は `$CCFOCUS_STATE_DIR`、無ければ Claude の config dir 配下の `focus/` (`~/.claude/focus`)。 |

キー操作:

| キー | 動作 |
|---|---|
| `1` `2` `3` `4` | パネル選択 (`j` / `k` / `Tab` でも移動) |
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

### `ccfocus` — focus board と session の状態を記録する hook

`ccfocus` は、1 つの state dir を共有する 2 つの部品でできている。hook
(`ccfocus hook <event>`) は Claude Code の全 session の状態を動作中に記録し、
focus board (`ccfocus` か `ccfocus tui`) はその記録を読んで、次に見るべき
session を表示する。先に hook を登録する。hook が無いと board に出る内容が無い。

#### hook の登録

`ccfocus hook <event>` は、session ごとの状態 (`idle` / `working` / `waiting` /
`done` / `failed`) を `~/.claude/focus/sessions/<session_id>.json` に記録し、
記録した event ごとに 1 行を `~/.claude/focus/events/<YYYYMMDD>.tsv` に追記する
(重なった hook のうち、後の hook に状態を譲ったものも行は書く)。`SessionEnd`
(session の file を消す) と、無視した event (対象外の `Notification`、wait を
解かない `PostToolBatch`) では行を書かない。prompt・message・tool の本文は保存
しない。場所は `--state-dir` か `CCFOCUS_STATE_DIR` で
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

#### focus board

```sh
ccfocus                          # `ccfocus tui` と同じ
ccfocus tui --state-dir DIR      # 既定の代わりに DIR を読む
```

state dir は `--state-dir`、無ければ `$CCFOCUS_STATE_DIR`、それも無ければ
`~/.claude/focus`。画面の表記は日本語。次は合成のデータによる例 (幅 120):

```text
focus · main:0                                                                   並行 5 / 4 · 次に回る 14:30 (残り 6 分)
┌ 次の 1 件 · 承認待ち 3m ─────────────────────────────────────────────────────────────────┐ ┌ 今日 ───────────────────┐
│[1] repo-a / tmux 表示の改善 · feat/tmux · dev:2.1                                        │ │終わった task         1  │
└──────────────────────────────────────────────────────────────────────────────────────────┘ │切り替え             12  │
 # status       repo         project                           worktree       start updated  │昨日の切り替え       56  │
 1 承認待ち     repo-a       tmux 表示の改善                   feat-tmux      01/02      3m  │並行 平均/最大  2.4 / 5  │
 2 完了         repo-b       e2e の安定化                      e2e-388        12/31     12m  │今週 (切り替え/日)       │
 3 作業中       repo-c       token 認証                        guard-token    01/03     40s  │▄▇█▃▆█▂                  │
 4 作業中       repo-a       -                                 main           09:12      2m  └─────────────────────────┘
 - 止まっている repo-d       renovate 集約                     renovate-2     12/29      6h
   2 日以上前に終わった・止まった session 2 件









終わらせ候補 (base より先行 · PR 無し)
   repo-a       guard-egress   ↑1 ↓5 clean
   repo-c       guard-token    ↑3 ↓0 未 commit 2 [3]
1-9 移る · j/k 選ぶ · d 差分 (hunk) · p window 名 · r 読み直す · q 閉じる
```

画面は 4 つの部分と、header・key の案内行でできている:

- **header**: `並行 N / L` は、直近 30 分に event がある session の数 (N) と
  上限 (L)。上限ちょうどで黄色、超えると赤になる。警告するだけで、何も止めない。
  隣の `次に回る` は次の見回りの時刻で、ローカルの 0 時から `round_minutes` ごとの
  格子で決まる。
- **次の 1 件** (`次の 1 件`): 次に見る session。承認待ち (`承認待ち`、待ちが長い
  順) → 失敗 (`失敗`) → まだ見ていない完了 (`完了`) → 作業中 (`作業中`) の順で選ぶ。
  完了した session は、board から移動した時点で見たものとして扱う。
- **一覧**: 全 session を `#`・status・repo・project・worktree・start (開始)・
  updated (最後の更新からの経過) で表示する。並びは上の順で、その後に待機
  (`待機`)、止まっている (`止まっている`) が続く。`1`-`9` の行は数字 key で移動でき、
  止まっている行に番号は付かない。
- **終わらせ候補** (`終わらせ候補`): `origin/HEAD` より先行していて、
  その branch に open か merge 済みの自分の PR が無い worktree。close して merge
  していない PR は branch を隠さない (まだ終わっていないので候補に残る)。各行に先行 (`↑`) と遅れ (`↓`) の commit 数、
  未 commit の数 (`未 commit`、無ければ `clean`)、使っている session の番号を出す。
- **今日** (`今日`): `終わった task` は今日作成または merge した自分の PR の数。
  `切り替え` は、直前の prompt と別の session に prompt を送った回数。
  `昨日の切り替え` は昨日の回数。`並行 平均/最大` は、今日の 30 分ごとの区間で
  event があった session 数の平均と最大。棒グラフは今日までの 7 日間の 1 日ごとの
  切り替え数。幅が 110 桁未満だと、この枠は一覧の上の 1 行にまとめる。

session の status:

- board と同じ側 (host か container かと hostname が同じ) の session は、`pid`
  が無くなると `止まっている` になる。
- それ以外は event が来ないことだけが手掛かりなので、30 分 event が無いと
  `止まっている` になる。対象は、`pid` を確かめられない反対側の session (host から
  見た container の session、またはその逆) と、同じ側でも生存を判定できない
  session (`pid` が記録されていない・`kill` が無い・Windows など)。
- 完了または止まっていて 2 日以上たった session は、1 行の件数にまとめる
  (`2 日以上前に終わった・止まった session N 件`)。

key:

| キー | 動作 |
|---|---|
| `1`-`9` | その session の tmux pane に移る (`tmux switch-client`)。container の session は `container_target` の window に移る |
| `j` / `k` (`↓` / `↑`) | 選ぶ行を動かす |
| `Enter` | 選んだ行に移る |
| `d` | 選んだ行の worktree で `hunk diff <merge-base> --watch` を board の横に開く (`<merge-base>` は `git merge-base HEAD origin/HEAD`) |
| `p` | 選んだ行の tmux window 名を project 名にする |
| `r` | 今すぐ読み直す (`config.toml` も) |
| `q` / `Esc` | board を閉じる |

tmux の外では、移動や名前変更の key (`1`-`9`・`Enter`・`d`・`p`) は、できない
理由を最下行に 1 行出すだけ。

board が呼ぶ tool:

- `tmux`: `1`-`9`・`Enter`・`d`・`p` の key。
- `hunk`: `d` の key。
- `gh`: `終わった task` と、終わらせ候補の PR の判定。`gh` が無い・失敗した時は、
  `終わった task` が `未取得` になり、終わらせ候補は PR で絞らない。

board は `git fetch` をしない。local の ref だけを読む。session の file と events は
2 秒ごと、worktree は 30 秒ごとに読み直し、`gh` の結果は 10 分 cache する (`r` でも
`gh` は取り直さない)。

設定は `<state dir>/config.toml` に書く。読むのは平らな `key = value` の行だけ
(table と escape は使えない)。壊れていたら既定の値で動き、理由を最下行に出す。

```toml
limit = 4                # 並行の上限 (1-99)
round_minutes = 30       # 次に回る時刻の間隔。0 で出さない (0-1440)
container_target = "dev" # container の session に移る時の tmux target
```

掃除: board (`ccfocus` の TUI と cctop の Focus panel) が、起動時と 1 時間ごとに、
90 日より古い event の log (`events/*.tsv`) と、10 分より古い hook の temp file
(`.<id>.json.<pid>.<nanos>.tmp`) を消す。

外への送信: `gh` が GitHub に問い合わせるのは、自分 (`@me`) の PR の検索と、
各 session の repo での `gh pr list --author @me` だけ。board は prompt や
transcript を読まない。

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
