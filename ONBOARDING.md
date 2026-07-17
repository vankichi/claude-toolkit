# claude-toolkit — Onboarding

Claude Code 補助 TUI 群の Cargo workspace(`ccwatch` / `ccmap` / `ccstat`)。
概要・install は [README.md](README.md)、開発 target は `make help`。

## Release フロー（`vX.Y.Z`）

**方式**: workspace 一括 release。1 つの `vX.Y.Z` tag で 3 crate を**同一 version**で束ね、
3 ターゲットのバイナリを public GitHub Release として発行する（`.github/workflows/release.yml`）。

### 手順

1. **version bump（全 crate 統一）**
   - `crates/ccstat/Cargo.toml` / `crates/ccmap/Cargo.toml` / `crates/ccwatch/Cargo.toml` の
     `version` を新 version に更新
   - `crates/ccstat/Cargo.toml` の ccmap dep 参照
     （`ccmap = { path = "../ccmap", version = "..." }`）も同じ version に揃える
   - `cargo check --workspace` で `Cargo.lock` を再生成
   - ※ unchanged な crate も上げる（v0.1.0 → v0.1.1 の前例。tag と各バイナリ `--version` を一致させる）

2. **`make ci`（fmt-check + clippy `-D warnings` + test）を green に**

3. **bump commit を main へ**
   ```sh
   git add crates/ccstat/Cargo.toml crates/ccmap/Cargo.toml crates/ccwatch/Cargo.toml Cargo.lock
   git commit -m "chore: bump workspace crates to X.Y.Z"
   git push origin main
   ```
   - `git add` は **specific path** で（`git add -A` / `.` / `*` は guard hook が拒否）
   - release の bump commit のみ main 直 push（owner）。通常の feature は PR → squash merge

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
