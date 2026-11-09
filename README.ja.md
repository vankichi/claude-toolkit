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
| **Focus** `[4]` | [`ccfocus`](#ccfocus--focus-board-と-session-の状態を記録する-hook) の board の要約 — 並行の数と上限・今日の数字・次に見る 1 件・先頭の数 session。`Enter` で board を全画面表示、`q` / `Esc` で overview に戻る。読む場所は hook が書く場所と同じで、`$CCFOCUS_STATE_DIR`、無ければ `~/.claude/focus` (`--claude-dir` では動かない)。 |

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
ccfocus tui --tiny               # 細い pane 向けの card 表示
```

state dir は `--state-dir`、無ければ `$CCFOCUS_STATE_DIR`、それも無ければ
`~/.claude/focus`。次は合成のデータによる例 (幅 120):

```text
focus · main:0                                                              parallel 5 / 4 · next round 14:30 (in 6 min)
┌ next up · waiting 3m ────────────────────────────────────────────────────────────────────┐ ┌ today ──────────────────┐
│[1] repo-a / tmux status line · feat/tmux · dev:2.1                                       │ │tasks done             1 │
└──────────────────────────────────────────────────────────────────────────────────────────┘ │switches              12 │
 # status   repo         project                               worktree       start updated  │yesterday             56 │
 1 waiting  repo-a       tmux status line                      feat-tmux      01/02      3m  │parallel avg/max 2.4 / 5 │
 2 done     repo-b       e2e 安定化                            e2e-388        12/31     12m  │week (switches/day)      │
 3 working  repo-c       token auth                            guard-token    01/03     40s  │▄▇█▃▆█▂                  │
 4 working  repo-a       -                                     main           09:12      2m  └─────────────────────────┘
 - stopped  repo-d       renovate batching                     renovate-2     12/29      6h
   2 sessions done or stopped 2+ days ago









to finish (ahead of base · no PR)
   repo-a       guard-egress   ↑1 ↓5 clean
   repo-c       guard-token    ↑3 ↓0 2 uncommitted [3]
1-9 jump · j/k select · d diff (hunk) · p name window · r reload · q quit
```

画面は 4 つの部分と、header・key の案内行でできている:

- **header**: `parallel N / L` は、直近 30 分に event がある session の数 (N) と
  上限 (L)。上限ちょうどで黄色、超えると赤になる。警告するだけで、何も止めない。
  隣の `next round` は次の見回りの時刻で、ローカルの 0 時から `round_minutes`
  ごとの格子で決まる。
- **next up**: 次に見る session。承認待ち (`waiting`、待ちが長い順) → 失敗
  (`failed`) → まだ見ていない完了 (`done`) → 作業中 (`working`) の順で選ぶ。
  完了した session は、board から移動した時点で見たものとして扱う。
- **一覧**: 全 session を `#`・status・repo・project・worktree・start (開始)・
  updated (最後の更新からの経過) で表示する。並びは上の順で、その後に待機
  (`idle`)、止まっている (`stopped`) が続く。`1`-`9` の行は数字 key で移動でき、
  止まっている行に番号は付かない。
- **to finish** (終わらせ候補): `origin/HEAD` より先行していて、その branch に
  open か merge 済みの自分の PR が無い worktree。close して merge していない PR は
  branch を隠さない (まだ終わっていないので候補に残る)。各行に先行 (`↑`) と遅れ
  (`↓`) の commit 数、未 commit の数 (`N uncommitted`、無ければ `clean`)、使って
  いる session の番号を出す。
- **today**: `tasks done` は今日作成または merge した自分の PR の数。`switches` は、
  直前の prompt と別の session に prompt を送った回数。`yesterday` は昨日の
  `switches`。`parallel avg/max` は、今日の 30 分ごとの区間で event があった
  session 数の平均と最大。棒グラフ (`week`) は今日までの 7 日間の 1 日ごとの
  切り替え数。幅が 110 桁未満だと、この枠は一覧の上の 1 行
  (`today  done 1 · switches …`) にまとめる。

`--tiny` は、細い pane (20〜40 桁ほど。window の 10% 幅の tmux pane など) 向けの
表示にする。session ごとに 1 枚の card を出し、番号・status・最後の event からの
時間・`repo · worktree`・project を載せる。選んでいる card は status の色の太枠で、
1 行増やして branch・tmux の window・開始時刻も出す。next up の card には `▸` が
付く。header は `N / L` と次の round (残り分数付き) で、その下に today を縮めて
出す (`done`・`sw` 切り替え・`y` 昨日、次の行に `par` 平均/最大と週の棒グラフ)。
card が入りきらない時は、選んでいる card が見える位置まで送り、上 (`↑N`) と下
(`↓N`) に隠れた card の数と、`·` の後に通常の board と同じくまとめた session の数
(`+N old`) を 1 行で出す。最下部に to finish の先頭 2 件を出す (残りは `+N more`、
PR が取れない時は `· PR?`)。遅れが無ければ `↓` を省き、未 commit の数は `±N`
で出して clean なら省く (分からない時は `?`)。背の低い pane では、選んでいる card
が見えるように、to finish、次に today の順で削る。key は同じ。同じ例を幅 24 で出すと:

```text
5 / 4 · 14:30 (6m)
done 1 · sw 12 (y 56)
par 2.4/5 ▄▇█▃▆█▂
┏▸1 waiting━━━━━━━━━━3m┓
┃repo-a · feat-tmux    ┃
┃tmux status line      ┃
┃feat/tmux · dev… 01/02┃
┗━━━━━━━━━━━━━━━━━━━━━━┛
┌2 done─────────────12m┐
│repo-b · e2e-388      │
│e2e 安定化            │
└──────────────────────┘
┌3 working──────────40s┐
│repo-c · guard-token  │
│token auth            │
└──────────────────────┘
↓2 more · +2 old



to finish 2
 repo-a·guard-egr… ↑1 ↓5
 repo-c·guard… ↑3 ±2 [3]
1-9 j/k d p r q
```

session の status:

- board と同じ側 (host か container かと hostname が同じ) の session は、`pid`
  が無くなると `stopped` になる。
- それ以外は event が来ないことだけが手掛かりなので、30 分 event が無いと
  `stopped` になる。対象は、`pid` を確かめられない反対側の session (host から
  見た container の session、またはその逆) と、同じ側でも生存を判定できない
  session (`pid` が記録されていない・`kill` が無い・Windows など)。
- 完了または止まっていて 2 日以上たった session は、1 行の件数にまとめる
  (`N sessions done or stopped 2+ days ago`)。

key:

| キー | 動作 |
|---|---|
| `1`-`9` | その session の tmux pane に移る (`tmux switch-client`)。container の session は、container の中でもその session の window と pane を選ぶ (「container の session に移る」を参照) |
| `j` / `k` (`↓` / `↑`) | 選ぶ行を動かす |
| `Enter` | 選んだ行に移る |
| `d` | 選んだ行の worktree で `hunk diff <merge-base> --watch` を board の上の tmux の popup で開く (`<merge-base>` は `git merge-base HEAD origin/HEAD`)。hunk を終えると popup が閉じる。hunk が失敗した時は error を出したまま残り、`Esc` で閉じる。window の layout は変わらない |
| `p` | 選んだ行の tmux window 名を project 名にする |
| `r` | 今すぐ読み直す (`config.toml` も) |
| `q` / `Esc` | board を閉じる |

tmux の外では、移動や名前変更の key (`1`-`9`・`Enter`・`d`・`p`) は、できない
理由を最下行に 1 行出すだけ。

board が呼ぶ tool:

- `tmux`: `1`-`9`・`Enter`・`d`・`p` の key。container の session では、
  env `CCFOCUS_CONTAINER_TMUX` に書いた command も (host で、shell を通さずに) 実行する。
- `hunk`: `d` の key。
- `gh`: `tasks done` と、`to finish` の PR の判定。`gh` が無い・失敗した時は、
  `tasks done` が `n/a` になり、`to finish` は PR で絞らない。

board は `git fetch` をしない。local の ref だけを読む。session の file と events は
2 秒ごと、worktree は 30 秒ごとに読み直し、`gh` の結果は 10 分 cache する (`r` でも
`gh` は取り直さない)。

設定は `<state dir>/config.toml` に書く。読むのは平らな `key = value` の行だけ
(table と escape は使えない)。`config.toml` は symlink でもよい (辿って読む) ので、
dotfiles から link で配れる。壊れていたら既定の値で動き、理由を最下行に出す。

```toml
limit = 4                   # 並行の上限 (1-99)
round_minutes = 30          # 次に回る時刻の間隔。0 で出さない (0-1440)
container_target = "dev"    # container の session に移る時の fallback の tmux target
```

container の中の tmux を動かす command は、`config.toml` からは読まない。host の
環境変数 `CCFOCUS_CONTAINER_TMUX` に書く。board を起動する host の shell か tmux の
環境に置く。例: `export CCFOCUS_CONTAINER_TMUX="dev tmux"`。未設定なら、board は
container の tmux を操作しない。

理由: この設定は、board が host で実行する program を決めるため。dev container は
state dir (`~/.claude`) を書き込みのできる形で mount することがあり、`config.toml`
から読むと、container が host で動く program を選べてしまう。`config.toml` に
`container_tmux` の行があると、壊れた行として拒否する (他の壊れた行と同じく、file
全体が既定値になり、`$CCFOCUS_CONTAINER_TMUX` を使うよう最下行に出す)。

値は、space と tab で区切って argv にし、shell を通さずに実行する。NBSP などの他の
空白は区切りにならない。space と tab 以外の空白や制御文字を含む値、256 byte を超える
値、UTF-8 でない値は無視し、理由を最下行に出す (file の警告があれば並べて 1 行に)。

掃除: board (`ccfocus` の TUI と cctop の Focus panel) が、起動時と 1 時間ごとに、
90 日より古い event の log (`events/*.tsv`) と、10 分より古い hook の temp file
(`.<id>.json.<pid>.<nanos>.tmp`) を消す。

外への送信: `gh` が GitHub に問い合わせるのは、自分 (`@me`) の PR の検索と、
各 session の repo での `gh pr list --author @me` だけ。board は prompt や
transcript を読まない。

#### container の session に移る

board を host の tmux で動かし、Claude の session が dev container の中にある時、
その行で `1`-`9` か `Enter` を押すと、次の 2 段階で移る。

1. `CCFOCUS_CONTAINER_TMUX` が設定されていて、session の `tmux_pane` (container の中の
   pane、`%N`) が分かる時は、`<CCFOCUS_CONTAINER_TMUX> select-window -t <pane>` と
   `<CCFOCUS_CONTAINER_TMUX> select-pane -t <pane>` を実行する。container の中の tmux で、
   その session の window と pane を選ぶ。`CCFOCUS_CONTAINER_TMUX="dev tmux"` と、
   dotfiles の `dev tmux <args>` (`docker exec -u <user> <container> tmux <args>`
   のこと) の組み合わせなら、container の中で tmux が動く。
   その後、`<CCFOCUS_CONTAINER_TMUX> show-environment -t <tmux_pane> CCFOCUS_HOST_PANE`
   を実行し、今 container を映している host の pane を container の tmux に聞く
   (理由は下に書く)。
2. host の client を、host の pane へ `tmux switch-client -t` で移す。候補は次の順
   に試す: 1 で今聞いた pane、session の file の `host_tmux_pane` (Claude の起動時
   の値)、`container_target`。値が無い・`%N` の形でない・host にその pane が無い
   (`switch-client` が失敗する) 候補は飛ばす。

1 が失敗しても 2 は行い、1 の error は最下行に出る。select が失敗した時は問い合わせ
を省く。`CCFOCUS_CONTAINER_TMUX` が無ければ、問い合わせも含めて 1 をしない (記録の pane、
次に `container_target`)。候補が失敗して後の候補に移った時は、その理由を最下行に出す (`fell back to <target>: <理由>`)。

聞く理由: `dev in` で入り直すと、container の tmux は `update-environment` で、
session env の `CCFOCUS_HOST_PANE` を新しい host の pane に書き換える。既に動いて
いる Claude と hook は起動時の値のままで、入力待ちの session は hook の event も
来ないので、記録だけでは古い pane に移ってしまう。container の `tmux.conf` に
`set -ga update-environment CCFOCUS_HOST_PANE` が要る (dotfiles は設定済み)。無い
時や問い合わせが失敗した時は、記録の pane を使う。答えも container 由来なので、
使う前に `%N` の形を確かめる。

`host_tmux_pane` を記録するには、container を起動する側が、host の pane を
`CCFOCUS_HOST_PANE` の環境変数で渡す。例: `docker exec -e CCFOCUS_HOST_PANE="$TMUX_PANE" …`。
container の中の hook がこれを読んで `host_tmux_pane` に記録する。記録するのは
`%N` の形の値だけで、host の hook はこの変数を読まない。

session の file は container が書くので、board は信用しない。pane id は `%N` の形を
確かめてから tmux の target にだけ使い、実行する command は host の env `CCFOCUS_CONTAINER_TMUX`
からだけ取る。`%N` の形でさえあれば host の任意の pane を指せるが、起きるのは移動
だけで、command は実行されない。desktop app の session (tmux の外) には、今までどおり移れない。

制約:

- 同じ container に複数の host pane から同時に attach していると、最後に attach
  した pane に移る。
- container から detach した (`dev in` を抜けた) 後は、その host pane は host の
  shell に戻っている。switch は成功するが、container は映っていない。

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
