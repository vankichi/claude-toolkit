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
  - user-level: `~/.claude/agents/*.md`、`~/.claude/skills/*/SKILL.md`、`~/.claude/commands/*.md`(symlink 先も辿る)
  - project-level: `<cwd>/.claude/agents/*.md` 等、同じ構造
  - plugin 提供分: `settings.json` の `enabledPlugins` が true な plugin についてのみ、`installed_plugins.json` の `installPath` 配下の `agents/`、`skills/*/SKILL.md`、`commands/*.md` を走査する(Installed だが未 enable の plugin の agent/skill/command は「実際に使えない」ため Agent/Skill/Command tab には出さない — Plugin tab 側は 3 状態すべてを表示する非対称な扱いを意図的に採る)
  - skill は `<name>/SKILL.md`、agent/command は `<name>.md`
- **Plugin**
  1. `~/.claude/plugins/known_marketplaces.json` で登録済み marketplace を列挙
  2. 各 marketplace の `marketplace.json` の `plugins[]` から `Available` 集合を作る(name / description / version / marketplace)
  3. `~/.claude/plugins/installed_plugins.json` に `<plugin>@<marketplace>` key があれば `Installed`(installPath / version を付与)
  4. `~/.claude/settings.json` の `enabledPlugins["<plugin>@<marketplace>"]` が true なら `Enabled` に格上げ
- **MCP server**(名前のみを安全に抽出)
  - `~/.claude.json` の `projects` map から、現在の cwd(正規化した絶対パス)に一致する key の entry のみを対象にする。他の project entry には触れない
  - その entry の `mcpServers` オブジェクトの **key(サーバ名)のみ** を取り出し、値(command / args / env / token 等)は読んだ直後に破棄する
  - 同 entry の `enabledMcpjsonServers` / `disabledMcpjsonServers` で有効・無効の別を付与
  - `oauthAccount` など `~/.claude.json` 内の他フィールドは一切参照しない

### 3.4 Frontmatter parser

対象ファイルの frontmatter(agent / skill / command の `.md` 先頭 `---` ブロック)を実データで確認した結果、以下が成立する:
- すべて flat な `key: value` 行(block scalar・nested map は無し)
- value 内に `:` が含まれるケースがある(例: `allowed-tools: Bash(gh issue view:*), ...`)が、`key` と `value` の境界は行内**最初の** `:` なので `splitn(2, ':')` で安全に分離できる
- quote(`"..."`)で囲われた description もあるが、前後の quote を trim すればよい

この形であれば自前の最小 parser で十分なため、YAML crate(`serde_yaml` 等)は追加しない。壊れた/読めない frontmatter はエラーにせず `name = ファイル名(拡張子抜き)`、`description = "(no description)"` にフォールバックする。

### 3.5 UI(ratatui / crossterm、`crates/ccwatch/src/ui.rs` の作法を踏襲)

- 上部に category tab(Agents / Skills / Commands / Plugins / MCP)。`Tab` / `BackTab` で切替
- 左ペイン: 一覧リスト。`j`/`k`/矢印キーで選択移動。選択変更に応じて右ペインが即時更新(ccwatch の Summary/Single のような明示的な「開く」操作は無い)
- 右ペイン: 詳細表示。description 全文、`Source`、`path`(あれば)、Plugin なら `PluginState` バッジ、Agent/Command なら `extra`(tools 等)
- `/`: 現在の tab 内で name + description を対象にした大文字小文字を無視する部分一致(substring)のインクリメンタル filter。`Esc` で解除(fzf 的な subsequence/fuzzy match は対象外)
- `e`: 選択中item の定義ファイルを `$EDITOR`(未設定なら `vi`)で開く。leave alternate screen → editor 起動 → 復帰。`path` が無い kind(MCP)では無効
- `y`: 選択中 item の「呼び出し文字列」をクリップボードにコピー。kind ごとの整形ルール:
  - Agent: そのまま name
  - Skill: plugin 提供なら `<plugin>:<skill>`、user/project 由来ならそのまま name
  - Command: `/<name>`
  - Plugin: `<name>@<marketplace>`
  - MCP: サーバ名そのまま
  - コピー実装は新規 crate を追加せず、`ccwatch::ui::open_in_browser` と同様に OS コマンドへ shell out(macOS: `pbcopy`、Linux: `xclip -selection clipboard` または `wl-copy`、それ以外: no-op)
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
    plugins.rs   // marketplace.json × installed_plugins.json × enabledPlugins のマージ
    mcp.rs       // ~/.claude.json から対象 cwd entry の mcpServers key のみ抽出
  ui.rs          // ratatui 描画 + イベントループ
```

## 4. DoD(受け入れ条件)

- [ ] `cargo test --workspace` で `ccmap` の discovery / frontmatter parser の unit test が pass する(`tempfile` で fixture ディレクトリを作り、user/project/plugin 各 source からの検出、frontmatter 欠落時のフォールバック、`known_marketplaces.json`×`installed_plugins.json`×`enabledPlugins` の 3 状態マージ、MCP の対象 cwd 以外の project entry を無視することを検証)
- [ ] `make ci`(fmt-check + clippy + test)が通る
- [ ] `ccmap` を実行し、実際の `~/.claude` 環境から agent / skill / command / plugin / MCP が各 tab に表示されることを手動確認
- [ ] `/` filter、`Tab` tab 切替、`e`(`$EDITOR` 起動)、`y`(クリップボードコピーの中身を `pbcopy` 経由で確認)、`R` rescan、`q` 終了が動作することを手動確認
- [ ] MCP 抽出処理が `~/.claude.json` 内の `mcpServers` 以外のキー(oauth 等)を読み取っていないことをコードレビューで確認

## 5. 制約

- **新規 dependency 追加なし**: frontmatter parser は自前実装、クリップボードは OS コマンド shell out(`ccwatch::open_in_browser` と同じパターン)
- **security**: `~/.claude.json` は oauth token 等の機微情報を含む。読み取りは対象 cwd の `mcpServers` キーのみに限定し、他フィールドは parse 後に即座に破棄。ログ・画面表示・エラーメッセージに token / URL 等の値を一切出力しない
- **read-only**: plugin の install/enable/disable、skill/agent/command の編集・削除は行わない(`e` は `$EDITOR` を起動するのみで、ccmap 自身は書き込まない)
- **performance**: 想定件数(数百 item 程度)であり、非同期化や index 構築は不要。起動時に同期的に全 source を走査する

## 6. メタ

- 対象 repo: `github.com/vankichi/claude-toolkit`(このリポジトリ)
- 優先度: 中(次の `ccwatch` 拡張と並ぶ個人 tool 追加)
- ready: 未設定(この spec は brainstorming の成果物であり、実装計画作成前に人間のレビュー待ち)
