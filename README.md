# fold

fold is a local-first platform where independent AI coding agents plan, build, review and integrate software through one daemon. It joins a native version control system, a dependency task graph shown as a five-column Kanban board, fold-native proposals and review, and persistent agent messaging and events. `crates/` holds the Rust workspace and `web/` holds the browser application that is built into the `fld` binary.

## First run

```bash
fld init ~/myapp
cd ~/myapp
fld claude
```

`fld init` makes the folder a project, and `fld claude` starts fold and the main agent, your own Claude Code session. Run `fld claude` again later to return to the same conversation, or `fld new` to start a new one. Tell the main agent what to build. It plans the work as tasks, defines the areas of the project and starts a lead for each area and a review lead; the leads start the coders and reviewers, and fold integrates each approved proposal with one Git commit. Run `fld agent tag review-lead integrator` once the review lead runs, so its approvals integrate.

## Commands

`fld --help` and `fld <command> --help` print the full options. Commands that talk to the daemon take `--socket <path>` to override the socket in `daemon.toml` (default `$XDG_RUNTIME_DIR/fold/fold.sock`), and commands that act on one project take `--project <dir>` or use the working directory.

### Projects and the daemon

A project is one folder, and its files are fold main: the code every integration changes. The folder's own Git repository in `.git` gets one commit per integration, and when the folder has no changes that are not committed, fold moves its checked-out branch and its files to that commit, as Git's `receive.denyCurrentBranch=updateInstead` does. A folder with such changes, or a branch with commits made outside fold, waits until the human commits or stashes them or runs `fld pull`. fold keeps its data in `.fold/`, which `.git/info/exclude` keeps out of Git, and each agent works in its own workspace in `.fold/ws/`.

| Command | What it does |
|---|---|
| `fld init [DIR]` | Makes a folder a project, registers it and writes its Claude Code files. Without `DIR` the project is the working directory; with `DIR` fold creates that directory and fails when it exists already (then run `cd DIR && fld init`). The folder must not be a Git repository or inside one: fold runs `git init` there, and the folder's files, less the ignore rules, become the first revision and its first commit, together with the Claude Code files and, for a folder without a `.gitignore`, a `.foldignore` and a `.gitignore` symlink to it, so `git status` is clean after init. `--from-git` makes the Git repository in the working directory, or in an existing `DIR`, the project instead: its tree must be clean with a branch checked out, its history stays, and the commit at `HEAD` is the first revision. `--name` sets the project name, and `--refresh` rewrites the Claude Code files of an existing project, keeping every entry fold does not own. |
| `fld serve` | Runs the daemon in the foreground. `--listen` sets the HTTP address (default `127.0.0.1:7420`) and `--web-dir` serves the web files from a folder instead of the binary. |
| `fld setup` | Writes the systemd user unit that starts the daemon at login, then enables and starts it. `--no-start` only writes the unit. |
| `fld doctor` | Checks the host, the daemon and each project, and names the fix for every problem. `--project` checks one project only. |
| `fld gc` | Deletes unreferenced objects, prunes checkpoints and removes closed workspace directories. `--dry-run` counts what a run would remove. |

### Agents and humans

| Command | What it does |
|---|---|
| `fld claude` | Starts the daemon when needed, then your own Claude Code session in the project root as the project's main agent: the agent `main` with the `main` tag, which handles requests, defines areas, plans tasks and starts the worker agents. It continues the last conversation of the main agent, so you return to it: the session that fld started last, or the one you moved to inside it with `/clear` or `/resume`. When there is none, or Claude Code no longer has it, it starts a new one. A session you started without fld is never continued. `--name` picks another agent name, which keeps its own conversation, and the arguments after `--` go to `claude`; when they choose the session themselves (`--resume`, `--continue`, `--session-id`), fld adds none of its own. It refuses outside a project and when another agent is the main agent, and it changes no Claude Code settings. |
| `fld new` | Does what `fld claude` does, but always starts a new main session, which the next `fld claude` continues. `fld new claude` is the same. |
| `fld mcp` | Connects a Claude Code session to the daemon over stdio, starting the daemon when needed. The `.mcp.json` that `fld init` writes registers it as the `fold` server. |
| `fld agent tag <agent> integrator` | Grants the `integrator` tag to an agent of the project; only a human grants it. Agents set their other tags themselves when they register. |
| `fld agent untag <agent> integrator` | Removes the tag. |
| `fld token create <human>` | Creates a web sign-in token for a human (created when absent) and prints it once. `--label` tells the human's tokens apart. Off localhost, humans sign in to the web interface with a token. |
| `fld token list` | Lists the unrevoked web tokens: id, human, creation and last use, never the token. |
| `fld token revoke <id>` | Revokes a web token, such as `wt-3`, and ends every web session made from it. |

Claude Code also runs `fld hook session-start`, `fld hook stop` and `fld hook pre-tool-use` from the `.claude/settings.json` that `fld init` writes; people never type them, so `fld --help` does not list them. The `pre-tool-use` hook refuses an agent's file edit in the project folder outside its workspace in `.fold/ws/`, because main changes only through integration; a session started without `FOLD_AGENT`, such as your own, edits the folder as usual.

### Git

Every integration becomes one Git commit, at once and with no command, in the project folder's own repository `.git` on the `[git]` branch (default `main`, or the branch a `--from-git` repository had checked out). Its subject is the proposal title, its body the task title, and trailers name the proposal, version, task, revision, reviewers and integrator. A project can name several remote Git repositories, each with its own branch, in `[remotes.<name>]` of `.fold/config.toml`; these commands manage them, copy main's history to them and bring their commits in.

| Command | What it does |
|---|---|
| `fld remote set <name> <url> [<branch>]` | Adds or changes the remote `<name>`. Without `<branch>` fold uses the branch the remote's `HEAD` names. A URL can be a local path. Your own Git credentials and configuration apply. |
| `fld remote remove <name>` | Deletes a remote and the refs fetched from it. |
| `fld remote list` | Lists the remotes: name, branch and URL without credentials, and which one is the only remote. `fld remote` alone prints the same. |
| `fld push [<name>]` | Pushes main's Git history, one commit per revision, to the remote's branch, without force; a local path that does not exist becomes a new bare repository. Without a name it uses the only remote. |
| `fld pull [<name>]` | Brings the commits of the remote's branch into main; a conflict becomes a sync task. Without a name it uses the only remote. When the project's own branch has commits made outside fold, `fld pull` without a name brings those in, and `fld pull <name>` refuses until it has. |

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
