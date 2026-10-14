.DEFAULT_GOAL := help
.PHONY: help setup build fmt check-rust clean

help: ## List the targets
	@grep -E '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-14s %s\n", $$1, $$2}'

setup: ## Install the toolchain, components and cargo tools (once per clone)
	rustup show
	rustup component add llvm-tools-preview
	cargo nextest --version >/dev/null 2>&1 || cargo install --locked cargo-nextest
	cargo llvm-cov --version >/dev/null 2>&1 || cargo install --locked cargo-llvm-cov
	cargo watch --version >/dev/null 2>&1 || cargo install --locked cargo-watch

build: ## Build the Rust workspace
	cargo build --workspace

fmt: ## Format the Rust code
	cargo fmt --all

check-rust: ## Run rustfmt, clippy and the Rust tests
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo nextest run --workspace

clean: ## Delete build output and dev projects
	cargo clean
	rm -rf dist .dev
