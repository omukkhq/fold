.DEFAULT_GOAL := help
.PHONY: help setup build fmt dev dev-daemon dev-web seed-dev dev-sign-in fmt-check-rust fmt-check-web fmt-check web-install web-build check-web-sources check-inner-allow clippy test test-fast test-default-features test-web coverage check-rust check-web check acceptance clean clean-keep-cargo gen-api check-api gen-languages release release-tools bench bench-smoke

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

build: ## Build the Rust workspace for the host with the dev profile, the fast build for development
	cargo build --workspace

fmt: ## Format the Rust code, fix lint findings and format the web sources
	cargo fmt --all
	cd web && bun run lint --fix
	cd web && bun run format

RELEASE_TRIPLE := x86_64-unknown-linux-musl
RELEASE_ASSET = fld-$(RELEASE_ARCH)-$(if $(findstring -apple-darwin,$(RELEASE_TRIPLE)),macos,linux)

DEV_DIR := $(CURDIR)/target/dev

dev: ## Run the daemon on :7420 and the Vite dev server on :5173; Ctrl-C stops both
	$(MAKE) -j2 dev-daemon dev-web

dev-daemon: ## Run the daemon on 127.0.0.1:7420, rebuilt on every change; its data lives under target/dev/, and the workers of a seeded project run the scripted stub
	mkdir -p $(DEV_DIR)/data $(DEV_DIR)/config $(DEV_DIR)/runtime
	chmod 700 $(DEV_DIR)/runtime
	XDG_DATA_HOME=$(DEV_DIR)/data XDG_CONFIG_HOME=$(DEV_DIR)/config XDG_RUNTIME_DIR=$(DEV_DIR)/runtime \
		cargo watch -w crates -w Cargo.toml -w Cargo.lock -x 'run -p fold-cli --features dev-seed -- serve --listen 127.0.0.1:7420'

# The seed registers its project in the dev daemon's registry and acts on the dev daemon's socket, so it takes the same directories as dev-daemon.
seed-dev: ## Development only: build a throwaway demo project in .dev/projects/demo through the running dev daemon
	XDG_DATA_HOME=$(DEV_DIR)/data XDG_CONFIG_HOME=$(DEV_DIR)/config XDG_RUNTIME_DIR=$(DEV_DIR)/runtime FOLD_HUMAN=dev \
		cargo run -p fold-cli --features dev-seed -- dev seed --socket $(DEV_DIR)/runtime/fold/fold.sock

dev-sign-in: ## Development only: print a one-time link that signs the human dev in to the running dev daemon; change its port to 5173 to sign in on the Vite dev server
	XDG_DATA_HOME=$(DEV_DIR)/data XDG_CONFIG_HOME=$(DEV_DIR)/config XDG_RUNTIME_DIR=$(DEV_DIR)/runtime FOLD_HUMAN=dev \
		cargo run -p fold-cli --features dev-seed -- web --socket $(DEV_DIR)/runtime/fold/fold.sock

# `make check` installs once and tells the second make so through WEB_INSTALLED.
web-install:
	$(if $(WEB_INSTALLED),@:,cd web && bun install --frozen-lockfile)

web-build: web-install ## Build the web application into web/dist, which the binary embeds
	cd web && bun run build

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

test: test-default-features ## Run every Rust test
	cargo nextest run --workspace --all-features

test-default-features: ## Run the tests that only build without features (the hidden dev command stays out of a normal build)
	cargo nextest run -p fold-cli --test integration -E 'test(/^dev_command::/)'

test-fast: ## Run the Rust tests without the acceptance tests and failpoint tests
	cargo nextest run --workspace --profile fast

test-web: ## Run the web tests
	cd web && bun run test

# The instrumented build lives in the worktree so parallel coverage runs in several worktrees do not rebuild each other's test binaries; ordinary builds keep the shared target directory.
coverage: export CARGO_LLVM_COV_TARGET_DIR := $(CURDIR)/target/llvm-cov-target
coverage: ## Run every Rust test and fail under 85% line coverage
	cargo llvm-cov nextest --workspace --all-features --ignore-filename-regex '/tests/' --fail-under-lines 85

check-rust: fmt-check-rust sqlx-check clippy coverage test-default-features ## Run rustfmt, clippy and the Rust tests with coverage

acceptance: ## Run only the acceptance tests
	cargo nextest run --workspace --all-features -E 'test(/^acceptance_/)'

bench: ## Measure the five runtime targets against a real daemon; FOLD_BENCH_DIR=<dir> picks the disk
	cargo bench -p fold-cli --bench budgets

bench-smoke: ## Run the whole bench harness on a fixture 100 times smaller
	FOLD_BENCH_SIZE=smoke cargo bench -p fold-cli --bench budgets

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

# macOS file systems ignore case, so an import of ./Card would find card.ts when it sits next to Card.tsx.
check-web-sources: web-install
	@dupes=$$(git ls-files web/src | sed -E 's/\.[^./]+$$//' | sort -u | sort -f | uniq -di); \
		[ -z "$$dupes" ] || { echo "web/src has module names that differ only in case, which clash on macOS: $$dupes" >&2; exit 1; }
	cd web && bun run typecheck
	cd web && bun run lint
	cd web && bun run format:check
	cd web && bun run coverage

check-web: check-web-sources web-build ## Run the web install, typecheck, lint, format check, coverage and build

GEN_DIR := target/gen

# openapi-typescript runs from web/ so bunx uses the version pinned in bun.lock.
gen-api: ## Regenerate web/src/api/schema.d.ts from the daemon's OpenAPI document (no daemon needed)
	mkdir -p $(GEN_DIR)
	cargo run -q -p fold-daemon --example openapi > $(GEN_DIR)/openapi.json
	cd web && bunx openapi-typescript ../$(GEN_DIR)/openapi.json -o src/api/schema.d.ts

gen-languages: web-install ## Regenerate web/src/syntax/catalog.gen.ts, the language names and file types, from the installed Shiki
	cd web && bun scripts/gen-languages.ts && bunx biome format --write src/syntax/catalog.gen.ts

check-api: ## Fail when web/src/api/schema.d.ts differs from the daemon's OpenAPI document
	mkdir -p $(GEN_DIR)
	cargo run -q -p fold-daemon --example openapi > $(GEN_DIR)/openapi.json
	cd web && bunx openapi-typescript ../$(GEN_DIR)/openapi.json -o ../$(GEN_DIR)/schema.d.ts
	@cmp -s $(GEN_DIR)/schema.d.ts web/src/api/schema.d.ts || { echo "web/src/api/schema.d.ts is stale: run make gen-api" >&2; exit 1; }

RELEASE_ARCH = $(firstword $(subst -, ,$(RELEASE_TRIPLE)))
RELEASE_DIST = dist/$(RELEASE_ASSET)
# The oldest macOS that the darwin build runs on; rustc and the C compiler of the bundled SQLite and zstd both read it.
RELEASE_MACOS_MIN := 13.0
# cc-rs looks for <arch>-linux-musl-gcc and falls back to musl-gcc only for x86_64, and Ubuntu's musl-tools ships only musl-gcc (for the host architecture), so the release names the C compiler of the bundled SQLite and zstd itself. rustc links the musl targets statically with their own C runtime, so no linker setting is needed.
RELEASE_MUSL_CC = $(or $(shell command -v $(RELEASE_ARCH)-linux-musl-gcc 2>/dev/null),$(if $(filter $(RELEASE_ARCH),$(shell uname -m)),$(shell command -v musl-gcc 2>/dev/null)))

ifneq ($(findstring -linux-musl,$(RELEASE_TRIPLE)),)
RELEASE_ENV = CC_$(subst -,_,$(RELEASE_TRIPLE))=$(RELEASE_MUSL_CC)
else ifneq ($(findstring -apple-darwin,$(RELEASE_TRIPLE)),)
RELEASE_ENV = MACOSX_DEPLOYMENT_TARGET=$(RELEASE_MACOS_MIN)
endif

release-tools:
	rustup target add $(RELEASE_TRIPLE)
ifneq ($(findstring -linux-musl,$(RELEASE_TRIPLE)),)
	@test -n "$(RELEASE_MUSL_CC)" || { echo "release: no musl C compiler for $(RELEASE_ARCH): put $(RELEASE_ARCH)-linux-musl-gcc on PATH, or on a $(RELEASE_ARCH) host install musl-gcc with: sudo apt install musl-tools" >&2; exit 1; }
endif

# A musl build is static: no shared libraries and no program interpreter; on x86_64 it is also a static PIE (rustc builds aarch64 musl as a plain static executable). macOS has no static executables, so the darwin build may link only the libraries and frameworks that macOS itself ships in /usr/lib and /System/Library.
release: release-tools $(if $(WEB_BUILT),,web-build) ## Build the standalone fld for RELEASE_TRIPLE (static musl x86_64 Linux by default) into dist/RELEASE_ASSET, check it, run the smoke test and write SHA256SUMS; WEB_BUILT=1 embeds the existing web/dist
	@test -f web/dist/index.html || { echo "release: web/dist/index.html is missing; the binary embeds web/dist" >&2; exit 1; }
	$(RELEASE_ENV) cargo build --release --locked -p fold-cli --target $(RELEASE_TRIPLE)
	rm -rf dist
	mkdir -p dist
	install -m 0755 "$$(cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/$(RELEASE_TRIPLE)/release/fld" $(RELEASE_DIST)
	file $(RELEASE_DIST) | grep -Eq '$(if $(filter x86_64,$(RELEASE_ARCH)),x86-64,aarch64|arm64)' || { echo "release: $(RELEASE_DIST) is not a $(RELEASE_ARCH) binary" >&2; exit 1; }
ifneq ($(findstring -linux-musl,$(RELEASE_TRIPLE)),)
	@! readelf -d $(RELEASE_DIST) | grep -q '(NEEDED)' && \
	 ! readelf -lW $(RELEASE_DIST) | grep -q 'Requesting program interpreter' || \
		{ echo "release: $(RELEASE_DIST) must be static: no shared libraries (DT_NEEDED) and no program interpreter" >&2; exit 1; }
ifeq ($(RELEASE_ARCH),x86_64)
	@readelf -h $(RELEASE_DIST) | grep -q 'Type:.*DYN' && readelf -d $(RELEASE_DIST) | grep -q 'FLAGS_1.*PIE' || \
		{ echo "release: $(RELEASE_DIST) must be a static PIE" >&2; exit 1; }
endif
endif
ifneq ($(findstring -apple-darwin,$(RELEASE_TRIPLE)),)
	@libs=$$(otool -L $(RELEASE_DIST) | tail -n +2 | awk '{print $$1}' | grep -vE '^/(usr/lib|System/Library)/'); \
	 [ -z "$$libs" ] || { echo "release: $(RELEASE_DIST) links libraries that macOS does not ship: $$libs" >&2; exit 1; }
	@otool -l $(RELEASE_DIST) | grep -A4 LC_BUILD_VERSION | grep -q 'minos $(RELEASE_MACOS_MIN)$$' || \
		{ echo "release: $(RELEASE_DIST) does not name macOS $(RELEASE_MACOS_MIN) as its minimum" >&2; exit 1; }
endif
	scripts/release-smoke.sh $(RELEASE_DIST)
	cd dist && $(if $(findstring -apple-darwin,$(RELEASE_TRIPLE)),shasum -a 256,sha256sum) $(RELEASE_ASSET) > SHA256SUMS

# The web build comes first because the binary embeds web/dist; the second make runs the rest: the checks of the web sources, without a second install or build.
check: web-build ## Run every check, after the web build that the binary embeds
	$(MAKE) WEB_INSTALLED=1 check-api check-rust check-web-sources

# cargo reports the directories it really builds into (the target directory and the build directory for intermediate files), whether they come from CARGO_TARGET_DIR, a config file or the default. Only when both are the repository's own target/, and target/ is not a symlink, does `cargo clean` delete it all; anywhere else the directory can be shared with other projects, so only fold's crates are cleaned there, for the dev and test profiles (debug/) and for release. When cargo cannot say, the shared branch is the safe one. The choice is made when make reads the file and only for the clean goal, so `make -n clean` shows just the branch that runs.
ifneq ($(filter clean,$(MAKECMDGOALS)),)
CARGO_BUILD_DIRS := $(shell cargo metadata --format-version 1 --no-deps 2>/dev/null | grep -oE '"(target|build)_directory":"[^"]*"' | sed 's/^"[a-z_]*":"//; s/"$$//' | sort -u)
TARGET_IS_SYMLINK := $(shell test -L target && echo yes)
FOLD_CRATES := $(notdir $(patsubst %/Cargo.toml,%,$(wildcard crates/*/Cargo.toml)))
endif

ifeq ($(CARGO_BUILD_DIRS)$(TARGET_IS_SYMLINK),$(CURDIR)/target)
clean: clean-keep-cargo ## Delete build output and dev projects; in a shared cargo target directory, only fold's own artifacts
	cargo clean
else
clean: clean-keep-cargo
	@echo "Shared cargo target directory $(or $(CARGO_BUILD_DIRS),(cargo did not report it)): cleaning only fold's crates; other projects' builds stay."
	rm -rf target/sqlx target/gen
	for crate in $(FOLD_CRATES); do cargo clean -q -p $$crate && cargo clean -q --release -p $$crate && cargo clean -q --release --target $(RELEASE_TRIPLE) -p $$crate || exit 1; done
endif

clean-keep-cargo: ## Delete build output and dev projects, but keep the cargo target directory
	rm -rf dist .dev target/dev web/dist web/coverage target/llvm-cov-target
