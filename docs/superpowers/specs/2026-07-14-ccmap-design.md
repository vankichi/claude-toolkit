# ccmap design

## 1. 目的 / 背景

現在の環境で「実際に使える」agent / skill / command / plugin / MCP server を横断的に見渡す手段がない。それぞれの定義は `~/.claude/agents`、`~/.claude/skills`(project-local を含む)、各 plugin ディレクトリ、`~/.claude/plugins/*.json`、`~/.claude.json` に分散しており、frontmatter を手で `grep`/`find` して回るしかない。`claude-toolkit` workspace に `ccwatch`(session usage の TUI)がある通り、同じ workspace に「自分が使える資産の参照 TUI」を追加し、name / description / 定義ファイルの場所 / plugin の有効化状態を一望できるようにする。

## 2. スコープ / non-goals

**スコープ**
- 対象種別: agent / skill / command(slash command)/ plugin / MCP server(名前のみ)
- 取得元: user-level(`~/.claude/*`)+ 実行時 cwd の project-level(`<cwd>/.claude/*`)+ 有効な plugin が提供する agent/skill/command
- 一覧・fuzzy filter・詳細表示・`$EDITOR` で開く・呼び出し文字列のクリップボードコピー・手動 rescan

**non-goals**
- plugin の install / enable / disable などの書き込み操作(read-only tool)
- claude.ai connector 系(Slack / Notion / Gmail 等)MCP の列挙 — local file から辿れないため対象外
- 自動 polling による live-refresh(`ccwatch` のような watch mode は持たない。手動 `R` rescan のみ)
- 同名 item の名前解決 / 優先順位判定(user vs project vs plugin で同名があっても両方そのまま表示し、どちらが実際に優先されるかは判定しない)

## 3. 設計本体

### 3.1 配置

新規 crate `crates/ccmap`(binary名 `ccmap`)。`crates/ccwatch` と同じ workspace member。`Cargo.toml` は `workspace.dependencies` から `anyhow` / `clap` / `crossterm` / `ratatui` / `serde` / `serde_json` / `tempfile`(dev-dependency)を継承。**新規 dependency は追加しない**(理由は 3.4 / 3.5 を参照)。

### 3.2 データモデル

```rust
enum Kind { Agent, Skill, Command, Plugin, Mcp }

enum Source {
    User,                                       // ~/.claude/{agents,skills,commands}
    Project,                                    // <cwd>/.claude/{agents,skills,commands}
    Plugin { plugin: String, marketplace: String },
}

enum PluginState { Available, Installed, Enabled }

struct Item {
    kind: Kind,
    name: String,
    description: String,                        // frontmatter 由来。無ければ "(no description)"
    source: Source,
    path: Option<PathBuf>,                       // MCP server は None(実体ファイルが無いため)
    extra: Vec<(String, String)>,                // agent の tools、command の allowed-tools 等
    plugin_state: Option<PluginState>,           // Kind::Plugin のみ Some
}
```

name の衝突は解決しない: 同名の user/project skill が両方存在すれば両方をそのまま表示し、それぞれに `Source` タグを付ける。

### 3.3 Discovery

- **Agent / Skill / Command**
  - user-level: `~/.claude/agents/*.md`、`~/.claude/skills/*/SKILL.md`、`~/.claude/commands/*.md`
  - project-level: `<project-dir>/.claude/agents/*.md` 等、同じ構造
  - **symlink 追従**: `~/.claude/agents` `~/.claude/skills` 等は**ディレクトリ自体が dotfiles 等への symlink** であるケースがある(この環境が該当)。`std::fs::read_dir` + `metadata()`(symlink を follow する API)で走査し、`walkdir` 等を使う場合も symlink 追従を有効にする。存在しないディレクトリ(この環境の `~/.claude/commands` 等)はエラーにせず skip する
  - plugin 提供分: 各 enabled plugin の `installPath`(下記 Plugin step3 で解決)配下を走査する。既定レイアウトは `agents/*.md`、`skills/*/SKILL.md`、`commands/*.md`。plugin の `.claude-plugin/plugin.json` が `agents` / `skills` / `commands` のカスタムパスを宣言している場合はそれを優先し、未宣言なら既定レイアウトを使う。Installed だが未 enable の plugin の agent/skill/command は「実際に使えない」ため Agent/Skill/Command tab には出さない(Plugin tab 側は 3 状態すべてを表示する非対称な扱いを意図的に採る)
  - skill は `<name>/SKILL.md`、agent/command は `<name>.md`
- **Plugin**
  1. marketplace 列挙は 2 ソースをマージする: `~/.claude/plugins/known_marketplaces.json`(key = marketplace 名、値に `installLocation`)+ `~/.claude/settings.json` の `extraKnownMarketplaces`(片方のみ登録の marketplace を取りこぼさないため)
  2. 各 marketplace の `<installLocation>/.claude-plugin/marketplace.json` の `plugins[]` から `Available` 集合を作る(entry フィールドは `name` / `description` / `source` / `author` / `category` / `homepage`。`version` はここには無く step3 の installed 情報から取る)。1 marketplace で数百 entry(official は 255 件)になり得るため Plugins tab は filter 前提
  3. `~/.claude/plugins/installed_plugins.json` は `{ "version": <n>, "plugins": { "<plugin>@<marketplace>": [ { "scope", "installPath", "version", ... }, ... ] } }` 構造。`.plugins` 配下に該当 key があれば `Installed` とする。値は install record の**配列**なので `scope == "user"` を優先し、無ければ先頭 record の `installPath` / `version` を採る。トップの `version` フィールド(現状 `2`)が未知の値でも panic せず best-effort で読む(スキーマ drift 耐性)
  4. enable 判定は `enabledPlugins["<plugin>@<marketplace>"] == true`。`~/.claude/settings.json` を主とし、`~/.claude/settings.local.json`(+ managed settings があれば)をマージして評価する。true なら `Enabled` に格上げ。同名 plugin が複数 marketplace に存在するケース(例: `superpowers@claude-plugins-official` と `superpowers@superpowers-marketplace`)は marketplace 込みの key で区別する
- **MCP server**(名前のみを安全に抽出)。local file から辿れる 3 スコープを対象にし、母集団の異なる 2 系統を混同しない:
  - **user スコープ**: `~/.claude.json` トップレベルの `mcpServers`(存在すれば)の key
  - **local スコープ**: `~/.claude.json` の `projects` map で、正規化した `--project-dir`(default cwd)の絶対パスに一致する key の entry の `mcpServers` の key。該当 key が存在しない場合は空(panic しない)。他の project entry には触れない
  - **project スコープ**: `<project-dir>/.mcp.json`(存在すれば)の `mcpServers` の key
  - いずれも **key(サーバ名)のみ** を取り出し、値(command / args / env / token 等)は取得しない(§5 の typed 部分 deserialize で materialize しない)
  - **enable/disable の適用範囲**: 対象 project entry の `enabledMcpjsonServers` / `disabledMcpjsonServers` は **`.mcp.json` 由来サーバの承認状態** であり、`mcpServers`(user/local で明示追加した定義)とは母集団が別。承認状態は project スコープ(`.mcp.json`)のサーバにのみ適用する。user/local スコープの `mcpServers` は明示追加済みとして常に有効扱い
  - claude.ai connector 系(Slack / Notion / Gmail 等)は local file から辿れないため対象外(§2 non-goals)
  - `oauthAccount` など `~/.claude.json` 内の他フィールドは一切参照しない

### 3.4 Frontmatter parser

対象ファイルの frontmatter(agent / skill / command の `.md` 先頭 `---` ブロック)を user + plugin 双方の実データで確認した結果、以下が成立する:
- すべて flat な `key: value` 行(block scalar・nested map は無し)
- value 内に `:` が含まれるケースがある(例: `allowed-tools: Bash(gh issue view:*), ...`)が、`key` と `value` の境界は行内**最初の** `:` なので `splitn(2, ':')` で安全に分離できる
- quote(`"..."`)で囲われた description もあるが、前後の quote を trim すればよい

この形であれば自前の最小 parser で十分なため、YAML crate(`serde_yaml` 等)は追加しない。壊れた/読めない frontmatter はエラーにせず `name = ファイル名(拡張子抜き)`、`description = "(no description)"` にフォールバックする。

**flat 前提の限界(既知・許容)**: 上記は現環境の user + plugin 資産すべてで確認した経験則であり、Claude Code の frontmatter 仕様上は block scalar(`>` / `|`)の description や list 形式の `allowed-tools` も許容される。これらは parse 全体失敗にはならず fallback 経路にも乗らないため、description 切れ等の silent 誤表示になり得る。個人 tool として許容し、誤表示に気付いたら都度対応する方針(YAML crate 追加はしない)。

### 3.5 UI(ratatui / crossterm、`crates/ccwatch/src/ui.rs` の作法を踏襲)

「踏襲」は描画・レイアウト作法を指す。event loop は §5 の同期 scan 方針に合わせ、`ccwatch` の async `event-stream` ではなく crossterm の blocking `event::read()` による同期ループとする(tokio 非依存)。

- 上部に category tab(Agents / Skills / Commands / Plugins / MCP)。`Tab` / `BackTab` で切替
- 左ペイン: 一覧リスト。`j`/`k`/矢印キーで選択移動。選択変更に応じて右ペインが即時更新(ccwatch の Summary/Single のような明示的な「開く」操作は無い)
- 右ペイン: 詳細表示。description 全文、`Source`、`path`(あれば)、Plugin なら `PluginState` バッジ、Agent/Command なら `extra`(tools 等)
- `/`: 現在の tab 内で name + description を対象にした大文字小文字を無視する部分一致(substring)のインクリメンタル filter。`Esc` で解除(fzf 的な subsequence/fuzzy match は対象外)
- `e`: 選択中item の定義ファイルを `$EDITOR`(未設定なら `vi`)で開く。leave alternate screen → editor 起動 → 復帰。`path` が無い kind(MCP)では無効
- `y`: 選択中 item の「呼び出し文字列」(CLI プロンプトに貼れば即起動できる形)をクリップボードにコピー。kind ごとの整形ルール:
  - Agent: そのまま name(例: `code-refactor-advisor`)
  - Skill: `/<name>`、plugin 提供なら `/<plugin>:<name>`(例: `/api-design-review`、`/superpowers:brainstorming`)
  - Command: `/<name>`、plugin 提供なら `/<plugin>:<name>`
  - Plugin: `<name>@<marketplace>`
  - MCP: サーバ名そのまま
  - コピー実装は新規 crate を追加せず OS コマンドへ shell out(macOS: `pbcopy`、Linux: `xclip -selection clipboard` または `wl-copy`、それ以外: no-op)。ただし `pbcopy` / `xclip` / `wl-copy` は **payload を STDIN から読む** ため、arg で URL を渡す `ccwatch::ui::open_in_browser` とは呼び出し方が異なる。`Stdio::piped()` で子プロセスを spawn し、その stdin へ文字列を書き込む
- `R`: 手動 rescan(discovery をやり直す。自動 polling はしない)
- `q` / `Esc`(filter 非活性時): 終了

### 3.6 CLI(clap、`ccwatch` の `Cli` 構造を踏襲)

```
ccmap [--claude-dir <PATH>] [--project-dir <PATH>]
```
- `--claude-dir`: `~/.claude` の override(default: `$HOME/.claude`)
- `--project-dir`: project-level scan の起点(default: 現在の cwd)

### 3.7 モジュール構成(想定)

```
crates/ccmap/src/
  main.rs        // clap CLI、entry point
  model.rs       // Kind / Source / PluginState / Item
  frontmatter.rs // 自前 flat key:value parser
  discover/
    agents.rs
    skills.rs
    commands.rs
    plugins.rs   // (known_marketplaces + extraKnownMarketplaces) × marketplace.json × installed_plugins × enabledPlugins のマージ
    mcp.rs       // ~/.claude.json(user/local)+ .mcp.json(project)の mcpServers key のみ抽出
  ui.rs          // ratatui 描画 + イベントループ
```

## 4. DoD(受け入れ条件)

- [ ] `cargo test --workspace` で `ccmap` の discovery / frontmatter parser の unit test が pass する(`tempfile` で fixture ディレクトリを構築し、以下を検証):
  - user / project / plugin 各 source からの検出、および **symlink されたディレクトリ** 越しの検出
  - frontmatter 欠落・破損時のフォールバック
  - marketplace 列挙が `known_marketplaces.json` と `extraKnownMarketplaces` の **マージ** であること
  - `installed_plugins.json` の `{version, plugins:{...: [records]}}` 構造(配列 record・`scope` 優先・未知 version)を正しく読むこと
  - `enabledPlugins` の 3 状態(Available / Installed / Enabled)判定、および同名 plugin の marketplace 別区別
  - MCP: user(トップレベル `mcpServers`)/ local(cwd project entry)/ project(`.mcp.json`)の 3 スコープ検出、cwd 以外の project entry を無視、`enabledMcpjsonServers` / `disabledMcpjsonServers` を `.mcp.json` 由来サーバにのみ適用すること
- [ ] `make ci`(fmt-check + clippy + test)が通る(clippy は `pedantic` + `-D warnings`)
- [ ] `README.md` の `## Tools` 表に `ccmap` 行を追加(`/add-rust-crate` が雛形を更新)
- [ ] `ccmap` を実行し、実 `~/.claude` 環境で agent / skill / plugin が各 tab に表示されることを手動確認。**Commands / MCP tab はこの環境では空になり得る**(commands ディレクトリ無し・全 project の `mcpServers` 空)ため、両 tab は合成 fixture(`--claude-dir` / `--project-dir` で指す)で表示を確認する
- [ ] `/` filter、`Tab` tab 切替、`e`(`$EDITOR` 起動)、`y`(コピー内容を確認)、`R` rescan、`q` 終了が動作することを手動確認
- [ ] MCP 抽出処理が `~/.claude.json` 内の `mcpServers` 以外のキー(`oauthAccount` 等)を読み取っていないことをコードレビューで確認(§5 の typed 部分 deserialize)

## 5. 制約

- **新規 dependency 追加なし**: frontmatter parser は自前実装、クリップボードは OS コマンド shell out(stdin 経由。§3.5 参照)
- **security**: `~/.claude.json` は oauth token 等の機微情報を含む。**unknown field を無視する serde の部分 typed struct** で deserialize する: トップレベルは `projects` と `mcpServers` のみ、project entry は `mcpServers` / `enabledMcpjsonServers` / `disabledMcpjsonServers` のみを持つ型とし、`mcpServers` の値は `serde::de::IgnoredAny`(key = サーバ名だけ残し、command / args / env / token 等の値は materialize しない)。`oauthAccount` 等は型に無いので読み込まれない。ログ・画面表示・エラーメッセージに token / URL / command 等の値を一切出力しない
- **read-only**: plugin の install/enable/disable、skill/agent/command の編集・削除は行わない(`e` は `$EDITOR` を起動するのみで、ccmap 自身は書き込まない)
- **frontmatter の flat 前提**: 経験則(現環境で確認済み)であり、valid だが非 flat な YAML は silent 誤表示になり得る(§3.4)。YAML crate は追加しない
- **フォーマット drift 耐性**: `installed_plugins.json` の `version` 等、Claude Code 側スキーマは version 付きで変化し得る。未知の値でも panic せず best-effort で読み、読めない部分は skip する
- **performance**: 想定件数(数百 item 程度。ただし Plugins tab の Available は marketplace catalog 由来で単独数百件)であり、非同期化や index 構築は不要。起動時に同期的に全 source を走査する

## 6. メタ

- 対象 repo: `github.com/vankichi/claude-toolkit`(このリポジトリ)
- 優先度: 中(次の `ccwatch` 拡張と並ぶ個人 tool 追加)
- ready: 未設定(この spec は brainstorming の成果物であり、実装計画作成前に人間のレビュー待ち)
