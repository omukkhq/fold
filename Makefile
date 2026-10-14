.DEFAULT_GOAL := help
.PHONY: help setup build fmt dev-web check-rust check-web clean clean-keep-cargo

help: ## List the targets
	@grep -E '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-18s %s\n", $$1, $$2}'

setup: ## Install the toolchain, components, cargo tools and web dependencies (once per clone)
	rustup show
	rustup component add llvm-tools-preview
	cargo nextest --version >/dev/null 2>&1 || cargo install --locked cargo-nextest
	cargo llvm-cov --version >/dev/null 2>&1 || cargo install --locked cargo-llvm-cov
	cargo watch --version >/dev/null 2>&1 || cargo install --locked cargo-watch
	cd web && bun install --frozen-lockfile

build: ## Build the Rust workspace
	cargo build --workspace

fmt: ## Format the Rust code, fix lint findings and format the web sources
	cargo fmt --all
	cd web && bun run lint --fix
	cd web && bun run format

dev-web: ## Run the Vite dev server on :5173
	cd web && bun run dev

check-rust: ## Run rustfmt, clippy and the Rust tests
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo nextest run --workspace

check-web: ## Run the web install, typecheck, lint, format check, coverage and build
	cd web && bun install --frozen-lockfile
	cd web && bun run typecheck
	cd web && bun run lint
	cd web && bun run format:check
	cd web && bun run coverage
	cd web && bun run build

clean: clean-keep-cargo ## Delete build output and dev projects
	cargo clean

clean-keep-cargo: ## Delete build output and dev projects, but keep the cargo target directory
	rm -rf dist .dev web/dist web/coverage
