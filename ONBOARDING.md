# claude-toolkit — Onboarding

Claude Code 補助ツール群の Cargo workspace。TUI の `ccwatch` / `ccmap` / `ccstat` / `cctop`、
hook の `ccfocus`、それらが共有する core の `cctk`（library のみ）の 6 crate。
概要・install は [README.md](README.md)、開発 target は `make help`。

## Release フロー（`vX.Y.Z`）

**方式**: workspace 一括 release。1 つの `vX.Y.Z` tag で全 6 crate を**同一 version**で束ね、
3 ターゲットのバイナリ（`cctk` 以外の 5 つ）を public GitHub Release として発行する
（`.github/workflows/release.yml`）。

### 手順

1. **version bump（全 crate 統一）**
   - 全 crate の `Cargo.toml`（`crates/{ccfocus,ccmap,ccstat,cctk,cctop,ccwatch}/Cargo.toml`）の
     `version` を新 version に更新
   - crate 間の path 依存の version（`cctk = { path = "../cctk", version = "..." }` など。
     `cctk` は他の 5 crate、`ccmap` は ccstat と cctop、`ccstat` は cctop が参照）も同じ version に揃える
   - `git grep -n '"<旧 version>"' -- 'crates/*/Cargo.toml'` が 0 件になれば漏れは無い
   - `cargo check --workspace` で `Cargo.lock` を再生成（変わるのは workspace の 6 package の version だけ）
   - ※ unchanged な crate も上げる（v0.1.0 → v0.1.1 以降の前例。tag と各バイナリ `--version` を一致させる）

2. **`make ci`（fmt-check + clippy `-D warnings` + test）を green に**

3. **bump commit を main へ**
   ```sh
   git add crates/ccfocus/Cargo.toml crates/ccmap/Cargo.toml crates/ccstat/Cargo.toml \
     crates/cctk/Cargo.toml crates/cctop/Cargo.toml crates/ccwatch/Cargo.toml Cargo.lock
   git commit -m "chore: bump workspace crates to X.Y.Z"
   git push origin main
   ```
   - `git add` は **specific path** で（`git add -A` / `.` / `*` は guard hook が拒否）
   - release の bump commit のみ main 直 push（owner）。通常の feature は PR → squash merge
   - main は ruleset で PR 必須。owner は bypass で push 可（`Bypassed rule violations` は正常）

   **新しいバイナリを足した release の場合**: `.github/workflows/release.yml` の bundle の bin 列挙
   （unix と windows の 2 箇所）と、`install.sh` の bin 列挙に足してあることを確かめる。
   `install.sh` では、過去の release に無い bin を `OPTIONAL_BINS` に入れる
   （`VERSION` で古い release を指定した時に skip させるため）。

4. **tag 作成 + push（= release トリガー）**
   ```sh
   git tag -a vX.Y.Z -m "vX.Y.Z — <要約>"
   git push origin vX.Y.Z
   ```
   - tag creation は ruleset で制限。**owner が bypass で push 可**（`Bypassed rule violations` は正常）

5. **`release.yml` の自動実行を見届け**
   ```sh
   gh run list --workflow=release.yml --limit 1   # run-id 取得
   gh run watch <run-id> --exit-status
   ```
   - build target: `aarch64-apple-darwin` / `x86_64-unknown-linux-gnu` / `x86_64-pc-windows-msvc`
   - 各 archive（unix=`.tar.gz` / windows=`.zip`、LICENSE 同梱）+ per-target `.sha256` →
     `SHA256SUMS` に集約 → public Release に添付

6. **確認**
   ```sh
   gh release view vX.Y.Z --json name,isDraft,assets
   ```
   - assets 4 点（3 archive + `SHA256SUMS`）、archive 名の version 一致、`draft=false`
   - `install.sh` は latest release を参照し checksum 検証込みで導入

### rollback（誤リリース時）

```sh
git push --delete origin vX.Y.Z && git tag -d vX.Y.Z   # tag 削除
gh release delete vX.Y.Z                                # Release 削除
```
- bump commit は **revert commit** で戻す（履歴書き換え・force push はしない）
