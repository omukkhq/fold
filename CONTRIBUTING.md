# Contributing to fold

Thank you for helping. The GitHub repository is a read-only mirror of our internal repository, so we do not accept pull requests. Use the ways below instead.

## Report a bug or ask for a feature

Open an [issue on GitHub](https://github.com/omukkhq/fold/issues). For a bug, give the `fld --version` output, the full output of `fld doctor`, the steps that cause it, what you expected and what happened.

## Report a security problem

Do not open a public issue. Report it privately through a [GitHub security advisory](https://github.com/omukkhq/fold/security/advisories/new). See [SECURITY.md](SECURITY.md).

## Send a patch

Send code as a patch by email, in the style of the Linux kernel, to **patch-fold@omukk.dev**:

```bash
git clone https://github.com/omukkhq/fold.git
cd fold
# make your change and commit it
git format-patch origin/main --cover-letter -o outgoing/
git send-email --to=patch-fold@omukk.dev outgoing/*.patch
```

- One logical change per commit. A series of commits gets a cover letter that says what the series does and why.
- Follow the rules in [AGENTS.md](AGENTS.md): run `make check` before you send, and write the commit message for the person who reads the history: what changed and why it matters.
- Add no trailers to the commit message, such as `Signed-off-by` or `Co-authored-by`.
- For a new version of a series, use `git format-patch -v2` and say in the cover letter what changed.
- To set up `git send-email`, see `git help send-email`.

We review each patch by email. When we accept it, we apply it to the internal repository, and it reaches the mirror with the next push.

## Work on fold

[AGENTS.md](AGENTS.md) has the rules for code in this repository, for people and for coding agents. Read it before you change code.

`crates/` holds the Rust workspace and `web/` holds the browser application:

| Part | What it holds |
|---|---|
| `crates/fold-core` | The decisions: task graph, status, policy and merge. It does no I/O. |
| `crates/fold-vcs` | The version control objects and the workspace files. |
| `crates/fold-store` | The SQLite database of each project. |
| `crates/fold-daemon` | The services, the HTTP API, the live events and the MCP tools. |
| `crates/fold-cli` | The `fld` binary. |
| `web/` | The web interface: Vite and React, managed by bun. |

```bash
make setup      # install the toolchain components, cargo tools, web dependencies and the git hook (one time for each clone)
make build      # fast development build of the Rust workspace for your system, with the dev profile
make dev        # run the daemon on :7420 with reload, and the Vite dev server on :5173
make seed-dev   # make a demo project through the dev daemon
make dev-sign-in # print a one-time link that signs you in to the dev daemon; change its port to 5173 for the Vite dev server
make check      # run all checks: format, clippy, tests, tsc, eslint, vitest and the builds
```

`make build`, `make dev`, the checks and the tests use the dev profile for your system, so they compile fast. Only `make release` does the slow static musl build.

`make help` shows all targets. Other common targets are `make fmt`, `make check-rust`, `make check-web`, `make acceptance` and `make gen-api`.

To run the tests of one file in `crates/<crate>/tests/`, select its module:

```bash
cargo nextest run -p <crate> --test integration -E 'test(/^<module>::/)'
```


## Benchmarks

`make bench` measures five runtime values against a real `fld serve` process and a test repository of 10 000 files and 200 MB. `make bench-smoke` runs the same tests on a repository 100 times smaller.

- `FOLD_BENCH_DIR=<dir>` selects the disk for the test repository.
- Each run writes `target/bench/results.json` and `target/bench/results.md`, with the host, the date and the fold version.

These are the results of `make bench` on 2026-10-15. The computer had a 13th Gen Intel Core i7-13620H with 16 logical cores, 15.3 GiB of memory and Zorin OS 18 with Linux 7.0. Other fold builds ran on the same computer during the test. Each time is the median of a number of runs.

| Target | What it measures | Value |
|---|---|---|
| `idle_memory` | Memory of the daemon with three sessions and three workspaces that do not change | 33.7 MB |
| `workspace_creation` | Time to make a workspace of the full repository | 16.582 s |
| `full_stat_scan` | Time to examine all files of a workspace for changes | 0.099 s |
| `change_visibility` | Time from a file change to the live event that shows it | 0.504 s |
| `integration` | Time to integrate a proposal that needs a three-way merge | 0.348 s |

## Build from source

Build from source for a different system, such as aarch64 Linux. You must have:

- The Rust toolchain in `rust-toolchain.toml`. rustup installs it the first time you build.
- [bun](https://bun.sh).
- git 2.36 or newer.

Get the source, then build the web application first, because the binary contains it:

```bash
git clone https://github.com/omukkhq/fold.git
cd fold
make web-build
cargo build --release --locked -p fold-cli
mkdir -p ~/.local/bin
cp target/release/fld ~/.local/bin/fld
```

`make release` builds the static release binary for x86_64 Linux into `dist/fld-x86_64-linux`, with a `SHA256SUMS` file. It builds for the `x86_64-unknown-linux-musl` target with link-time optimisation, makes sure that the binary has no shared libraries and no program interpreter, and runs a smoke test. It needs a musl C compiler for the bundled SQLite and zstd: install it with `sudo apt install musl-tools`. rustup installs the musl target from `rust-toolchain.toml`.

`rust-toolchain.toml` lists only the x86_64 musl target. To build another release target, name it in `RELEASE_TRIPLE`: `make release` adds that target with rustup, and names the binary after it, `dist/fld-<arch>-linux` or `dist/fld-<arch>-macos`. For example, on an aarch64 Linux machine this builds `dist/fld-aarch64-linux`:

```bash
make release RELEASE_TRIPLE=aarch64-unknown-linux-musl
```

On an Apple silicon Mac, use `aarch64-apple-darwin`. The release CI does the same for each platform when a version tag is pushed.

To change only the web interface, run `bun run build` in `web/`, then `fld serve --web-dir web/dist`. The daemon then serves that folder, so you do not build a new binary.
