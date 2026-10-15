# fold

fold is a local-first platform where independent AI coding agents plan, build, review and integrate software through one daemon. It joins a native version control system, a dependency task graph shown as a five-column Kanban board, fold-native proposals and review, and persistent agent messaging and events. `crates/` holds the Rust workspace and `web/` holds the browser application that is built into the `fold` binary.

## Developer setup

Prerequisites: the Rust toolchain pinned in `rust-toolchain.toml` (rustup installs it on first use), [bun](https://bun.sh) and git 2.36 or newer.

```bash
make setup   # toolchain components, cargo tools, web dependencies, prek and its git hook (once per clone)
make dev     # daemon with reload on :7420 plus the Vite dev server on :5173
make check   # everything a change must pass: format, clippy, tests, coverage floors, tsc, eslint, vitest, builds
```

`make help` lists every target. The common ones are `make fmt`, `make check-rust`, `make check-web`, `make acceptance`, `make gen-api` and `make seed-dev`.

Each crate's integration tests build as one program, `tests/main.rs`, with one module per file. To run the tests of one file, select its module: `cargo nextest run -p <crate> --test integration -E 'test(/^<module>::/)'`.

## Build cache

By default cargo builds into the repository's own `target/`, and `make clean` deletes it with `cargo clean`.

You can instead set one shared `target-dir` for all your projects in `~/.cargo/config.toml`, or point `CARGO_TARGET_DIR` at a directory. The Makefile asks cargo for the real target directory, so every target works with both setups. With a shared directory, `make clean` runs `cargo clean -p` for each fold crate in the dev and release profiles, so the builds of your other projects stay. cargo matches those artifacts by crate name, so the clean also removes the fold builds of your other worktrees, and the dependencies that fold built stay. Disk use grows with every worktree and branch, because each worktree path builds its own copy of every fold crate and of each crate's integration test program. Prune old fold artifacts from time to time, for example with `cargo sweep`.

The dev profile sets `incremental = false`. In a shared target directory each worktree keeps its own incremental data for every fold crate and test binary, which fills tens of gigabytes.

Coverage builds into `target/llvm-cov-target` inside each worktree, not into the shared directory. Parallel worktrees then do not rebuild each other's instrumented test binaries.

`CARGO_TARGET_DIR` overrides the target directory for one command, for example `CARGO_TARGET_DIR=target make check` to build into the repository. On a machine with little memory, lower the number of parallel compile jobs with `CARGO_BUILD_JOBS`, for example `CARGO_BUILD_JOBS=3 make check`.

sccache gives few hits across worktrees. Between two target folders sccache 0.18 had 6 hits in 280 compilations, because its cache key includes the absolute paths of the dependencies.
