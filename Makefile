.DEFAULT_GOAL := help
.PHONY: help setup build fmt dev-web fmt-check-rust fmt-check-web fmt-check check-inner-allow clippy test test-fast test-web coverage check-rust check-web check acceptance clean clean-keep-cargo

help: ## List the targets
	@grep -E '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-18s %s\n", $$1, $$2}'

setup: ## Install the toolchain, components, cargo tools, web dependencies, prek and its git hook (once per clone)
	rustup show
	rustup component add llvm-tools-preview
	cargo nextest --version >/dev/null 2>&1 || cargo install --locked cargo-nextest
	cargo llvm-cov --version >/dev/null 2>&1 || cargo install --locked cargo-llvm-cov
	cargo watch --version >/dev/null 2>&1 || cargo install --locked cargo-watch
	cd web && bun install --frozen-lockfile
	prek --version >/dev/null 2>&1 || cargo install --locked prek
	prek install --allow-missing-config

build: ## Build the Rust workspace
	cargo build --workspace

fmt: ## Format the Rust code, fix lint findings and format the web sources
	cargo fmt --all
	cd web && bun run lint --fix
	cd web && bun run format

dev-web: ## Run the Vite dev server on :5173
	cd web && bun run dev

fmt-check-rust: ## Check the Rust formatting
	cargo fmt --all --check

fmt-check-web: ## Check the web formatting
	cd web && bun run format:check

fmt-check: fmt-check-rust fmt-check-web ## Check the formatting of both halves

check-inner-allow: ## Fail when a Rust file under crates/ has an inner allow attribute, also inside cfg_attr
	@find crates -name '*.rs' -exec awk ' \
		FNR == 1 { open = 0 } \
		/^[[:space:]]*#![[:space:]]*\[/ { open = 1; start = FNR; first = $$0; text = "" } \
		open { text = text " " $$0 } \
		open && /\][[:space:]]*$$/ { \
			open = 0; \
			gsub(/"([^"\\]|\\.)*"/, "", text); \
			if (text ~ /(^|[^[:alnum:]_])allow[[:space:]]*\(/) { print FILENAME ":" start ":" first; bad = 1 } \
		} \
		END { exit bad }' {} + || { echo 'inner #![allow] attributes are not permitted; use #[expect(lint, reason = "...")]' >&2; exit 1; }

clippy: check-inner-allow ## Run clippy with warnings denied, after the inner allow check
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test: ## Run every Rust test
	cargo nextest run --workspace --all-features

test-fast: ## Run the Rust tests without the acceptance tests and failpoint tests
	cargo nextest run --workspace --profile fast

test-web: ## Run the web tests
	cd web && bun run test

coverage: ## Run every Rust test and fail under 85% line coverage
	cargo llvm-cov nextest --workspace --all-features --cargo-profile ci --ignore-filename-regex '/tests/' --fail-under-lines 85

check-rust: fmt-check-rust clippy coverage ## Run rustfmt, clippy and the Rust tests with coverage

acceptance: ## Run only the acceptance tests
	cargo nextest run --workspace --all-features -E 'test(/^acceptance_/)' --no-tests=pass

check-web: ## Run the web install, typecheck, lint, format check, coverage and build
	cd web && bun install --frozen-lockfile
	cd web && bun run typecheck
	cd web && bun run lint
	cd web && bun run format:check
	cd web && bun run coverage
	cd web && bun run build

check: check-rust check-web ## Run every check

clean: clean-keep-cargo ## Delete build output and dev projects
	cargo clean

clean-keep-cargo: ## Delete build output and dev projects, but keep the cargo target directory
	rm -rf dist .dev web/dist web/coverage
