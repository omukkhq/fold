.DEFAULT_GOAL := help
.PHONY: help setup build fmt dev dev-daemon dev-web fmt-check-rust fmt-check-web fmt-check check-inner-allow clippy test test-fast test-web coverage check-rust check-web check acceptance clean clean-keep-cargo

help: ## List the targets
	@grep -E '^[a-zA-Z0-9_-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-18s %s\n", $$1, $$2}'

setup: ## Install the toolchain, components, cargo tools, web dependencies, prek and its git hook (once per clone)
	rustup show
	rustup component add llvm-tools-preview
	cargo nextest --version >/dev/null 2>&1 || cargo install --locked cargo-nextest
	cargo llvm-cov --version >/dev/null 2>&1 || cargo install --locked cargo-llvm-cov
	cargo sqlx --version >/dev/null 2>&1 || cargo install --locked sqlx-cli --no-default-features --features sqlite,rustls
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

DEV_DIR := $(CURDIR)/target/dev

dev: ## Run the daemon on :7420 and the Vite dev server on :5173; Ctrl-C stops both
	$(MAKE) -j2 dev-daemon dev-web

dev-daemon: ## Run the daemon on 127.0.0.1:7420, rebuilt on every change; its data lives under target/dev/
	mkdir -p $(DEV_DIR)/data $(DEV_DIR)/config $(DEV_DIR)/runtime
	chmod 700 $(DEV_DIR)/runtime
	XDG_DATA_HOME=$(DEV_DIR)/data XDG_CONFIG_HOME=$(DEV_DIR)/config XDG_RUNTIME_DIR=$(DEV_DIR)/runtime \
		cargo watch -w crates -w Cargo.toml -w Cargo.lock -x 'run -p fold-cli -- serve --listen 127.0.0.1:7420'

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

check-rust: fmt-check-rust sqlx-check clippy coverage ## Run rustfmt, clippy and the Rust tests with coverage

acceptance: ## Run only the acceptance tests
	cargo nextest run --workspace --all-features -E 'test(/^acceptance_/)' --no-tests=pass

SQLX_DIR := target/sqlx
SQLX_DB_URL := sqlite:$(CURDIR)/$(SQLX_DIR)/prepare.db
MIGRATION_SETS := project registry

.PHONY: migration sqlx-scratch sqlx-prepare sqlx-check

migration: ## Add a forward-only migration: make migration NAME=<name> [DB=registry]
	@test -n "$(NAME)" || { echo "NAME is required: make migration NAME=<name> [DB=registry]"; exit 1; }
	@case "$(or $(DB),project)" in \
		project|registry) sqlx migrate add --sequential --source crates/fold-store/migrations/$(or $(DB),project) $(NAME) ;; \
		*) echo "DB must be one of: $(MIGRATION_SETS)"; exit 1 ;; \
	esac

# The scratch database holds the tables of both sets so every query can be checked. Both sets number from 0001 and would collide in one _sqlx_migrations table, so they run as one joined file.
sqlx-scratch:
	rm -rf $(SQLX_DIR)
	mkdir -p $(SQLX_DIR)/migrations
	cat $(foreach set,$(MIGRATION_SETS),crates/fold-store/migrations/$(set)/*.sql) > $(SQLX_DIR)/migrations/0001_scratch.sql
	sqlx database create --database-url $(SQLX_DB_URL)
	sqlx migrate run --source $(SQLX_DIR)/migrations --database-url $(SQLX_DB_URL)

sqlx-prepare: sqlx-scratch ## Refresh the .sqlx query metadata after changing a query
	SQLX_OFFLINE=false DATABASE_URL=$(SQLX_DB_URL) cargo sqlx prepare --workspace -- --all-targets

sqlx-check: sqlx-scratch ## Fail when the .sqlx query metadata is stale
	SQLX_OFFLINE=false DATABASE_URL=$(SQLX_DB_URL) cargo sqlx prepare --workspace --check -- --all-targets

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
	rm -rf dist .dev target/dev web/dist web/coverage
