.DEFAULT_GOAL := help

# Override at invocation: `make install PREFIX=/opt/homebrew/bin`
PREFIX ?= $(HOME)/.local/bin

.PHONY: help build release test clippy fmt-check fmt check deny ci install clean update

help: ## このヘルプを表示
	@grep -hE '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

build: ## デバッグビルド (workspace 全 crate)
	cargo build --workspace --all-targets

release: ## リリースビルド (lto=fat / panic=abort)
	cargo build --workspace --release --locked

test: ## テスト実行 (workspace 全 crate)
	cargo test --workspace --all-targets --locked

clippy: ## clippy (-D warnings)
	cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt-check: ## フォーマット確認 (差分があれば失敗)
	cargo fmt --all -- --check

fmt: ## フォーマット適用
	cargo fmt --all

check: ## コンパイル確認のみ
	cargo check --workspace --all-targets --locked

deny: ## supply-chain 監査 (要 cargo-deny: cargo install cargo-deny --locked)
	cargo deny check

ci: fmt-check clippy test ## fmt-check + clippy + test を一括実行

install: release ## release build → $(PREFIX) にコピー (default: ~/.local/bin)
	@mkdir -p "$(PREFIX)"
	@for crate in crates/*/; do \
		name=$$(basename $$crate); \
		cp target/release/$$name "$(PREFIX)/$$name"; \
		if [ "$$(uname)" = "Darwin" ]; then \
			codesign --force --sign - "$(PREFIX)/$$name" >/dev/null 2>&1; \
		fi; \
		echo "installed $$name -> "$(PREFIX)/$$name""; \
	done

uninstall: ## $(PREFIX) から各 crate のバイナリを削除
	@for crate in crates/*/; do \
		name=$$(basename $$crate); \
		rm -f "$(PREFIX)/$$name"; \
		echo "removed "$(PREFIX)/$$name""; \
	done

clean: ## ビルド成果物削除
	cargo clean

update: ## Cargo.lock を更新
	cargo update
