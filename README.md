# fold

fold gives you a team of AI coding agents on your own machine. You tell one Claude Code session what to build. fold plans the work, starts more agents to write and review the code, and adds each approved change to your project as one Git commit. A live board in your browser shows all of the work.

fold is one binary, `fld`. Your code and your data stay on your machine.

```bash
curl -fsSL https://omukk.dev/fold/install.sh | sh
fld init ~/myapp
cd ~/myapp
fld claude
```

## Contents

- [How it works](#how-it-works)
- [Install](#install)
- [Quick start](#quick-start)
- [Commands](#commands)
- [The web interface](#the-web-interface)
- [Configuration](#configuration)
- [Your own agent sessions](#your-own-agent-sessions)
- [Build from source](#build-from-source)
- [Develop fold](#develop-fold)
- [License](#license)

## How it works

1. You run `fld claude` in your project. This starts the **main agent**, your own Claude Code session.
2. You tell the main agent what you want. It asks about each unclear point and shows you a plan.
3. You say OK. The main agent then divides the project into **areas**, such as `cli` or `storage`, and divides the work into **tasks**.
4. The main agent starts one **area lead** for each area and one **review lead**.
5. Each area lead starts **task coders**. A coder works in its own copy of the project, a **workspace**, and sends its result as a **proposal**.
6. The review lead, or a **batch reviewer** that it starts, examines each proposal. It approves the proposal or asks for changes. fold sends the changes back to the same coder.
7. fold **integrates** each approved proposal: it adds the change to the project folder and makes one Git commit.

fold starts, supervises and stops each worker agent. Agents change files only in their workspaces. The project folder changes only through integration, so one agent cannot break the work of a different agent.

### The board

Each task is a card on a board with five columns. fold calculates the column of each card, so you do not move a card by hand.

| Column | The task |
|---|---|
| Backlog | Waits for a different task to finish. |
| Todo | Is ready, and no agent works on it. |
| In Progress | Has an agent that works on it. |
| Review | Has a proposal or a report that waits for a review. |
| Done | Is complete. |

A task can depend on other tasks. fold keeps the task in Backlog until each of those tasks is Done. A large task can have child tasks. It goes to Done when all of its children are Done.

A **human task** is work that only a person can do outside every agent, such as a check with a provider. No agent starts it. It stays in Todo until a human marks it Done, or the main agent when you tell it. Then the tasks that need it continue. A **blocker** holds a task until a person says that it can start. You set it, with a reason, before an agent starts the task. While the blocker is set, no agent starts the task, and the task stays in its column. A human clears the blocker, or the main agent when you tell it.

### When fold asks for you

The web interface shows each item that waits for a human:

- **A question** from an agent, with choices.
- **A permission question.** A worker wants to run a tool call, and Claude Code's auto mode does not decide it alone. Only a human can answer.
- **A human review.** You can mark a task for human review. You approve its proposal after a reviewer agent approves it.
- **A request draft** for a request that you made.
- **A human task** that is ready, and **a blocker** with its reason. Open the task to mark the work Done or to clear the blocker.

### Requests

Make a request when you find a bug or want a new feature. A request has a text and can have attachments, such as screenshots. The main agent or an area lead takes the request and writes a draft of cards. You approve the draft, ask for changes or close the request. When you approve the draft, its cards become tasks. Agents also make requests when they find work outside their own tasks.

## Install

You must have:

- x86_64 Linux. The `fld` binary is static, so it does not need a specific glibc version. For a different system, [build from source](#build-from-source).
- [Claude Code](https://code.claude.com), installed and signed in.
- `git`.

Install `fld`:

```bash
curl -fsSL https://omukk.dev/fold/install.sh | sh
fld --version
```

`fld claude` starts the fold daemon when it is necessary. To start the daemon each time you log in, use systemd:

```bash
fld setup                         # write, enable and start the systemd user unit
loginctl enable-linger "$USER"    # keep the daemon on after you log out
fld doctor                        # find problems and show the fix for each one
```

To upgrade, replace the binary at `~/.local/bin/fld`, then run `systemctl --user restart fold.service`.

## Quick start

Make a new project and start the main agent:

```bash
fld init ~/myapp    # make a new folder that is a fold project with its own Git repository
cd ~/myapp
fld claude          # start the main agent
```

Tell the main agent what to build, for example "Build me a simple CLI note taker." Answer its questions, read its plan, then say OK. Open <http://127.0.0.1:7420> to see the board.

Use a Git repository that you have already:

```bash
cd ~/code/myapp
fld init --from-git    # the repository must be clean, with a branch checked out
fld claude
```

The history of the repository stays. The commit at `HEAD` becomes the first revision of the project.

Come back later:

```bash
fld claude    # continue the last conversation with the main agent
fld new       # start a new conversation
```

## Commands

`fld --help` and `fld <command> --help` show all options. Use `--project <dir>` to select a project; the default is the current folder. Use `--socket <path>` to select the daemon socket; the default is `$XDG_RUNTIME_DIR/fold/fold.sock`.

### The main agent

| Command | What it does |
|---|---|
| `fld claude` | Starts the daemon when it is necessary, then the main agent. Continues the last conversation of the main agent, or starts a new one. |
| `fld new` | Starts a new conversation with the main agent. `fld new claude` does the same. |

- `fld claude --name <name>` gives the main agent a different name. Each name keeps its own conversation.
- Arguments after `--` go to `claude`, for example `fld claude -- --model opus`.
- fld adds the main agent prompt to the system prompt of the session at each start and resume. To use your own prompt, write it in `.fold/roles/main.md`. fld always adds its shared authority and recovery rules after your text.
- fld does not add its prompt when your arguments set a system prompt (`--system-prompt`, `--system-prompt-file`, `--append-system-prompt`, `--append-system-prompt-file`).
- fld does not select a session when your arguments select one (`--resume`, `--continue`, `--session-id`).
- `fld claude` does not continue a session that you started without fld. It does not change your Claude Code settings.
- `fld claude` stops with an error outside a project, and when a different agent is the main agent.

### Projects

| Command | What it does |
|---|---|
| `fld init [DIR]` | Makes a folder into a project and registers it with the daemon. |
| `fld init --from-git [DIR]` | Makes an existing Git repository into a project, and keeps its history. |
| `fld init --refresh` | Writes the Claude Code files of a project again. It keeps all entries that fold does not own. |
| `fld doctor` | Examines the host, the daemon and each project, and shows the fix for each problem. |
| `fld gc` | Deletes data that fold does not use, old checkpoints and closed workspaces. `--dry-run` counts what it can delete. |

`fld init` without `DIR` uses the current folder. With `DIR`, fold makes that folder; it stops if the folder exists. The folder must not be in a Git repository, because fold runs `git init` there. Your files become the first commit, with the Claude Code files that fold adds: `.mcp.json`, `.claude/settings.json`, the fold skill and a fold section in `AGENTS.md`. When the folder has no `.gitignore`, fold adds a `.foldignore` and a `.gitignore` link to it. `--name` sets the project name.

fold keeps its data in `.fold/` in the project folder, and Git ignores that folder. Each agent works in its own workspace in `.fold/ws/`.

### Pause and resume

| Command | What it does |
|---|---|
| `fld pause` | Pauses every agent of the project. fold stops each worker and keeps its session. |
| `fld resume` | Resumes the agents. Each stopped worker continues in its own session. |

- Pause the agents before you sleep, or before a power or internet outage. The **Pause agents** control in the top bar of the web interface does the same, and the main agent pauses or resumes when you ask it.
- While the project is paused, fold starts no worker, and no claim becomes stale. Approved proposals still integrate and push, and humans can read, comment and approve.
- The main agent's own session continues, but it starts no work. A pause stays when the daemon restarts.

### The daemon

| Command | What it does |
|---|---|
| `fld serve` | Runs the daemon in the foreground. `--listen <addr>` sets the web address (default `127.0.0.1:7420`). `--web-dir <dir>` serves the web files from a folder. |
| `fld setup` | Writes the systemd user unit that starts the daemon when you log in, then enables and starts it. `--no-start` only writes the unit. |
| `fld mcp` | Connects a Claude Code session to the daemon. You do not type this command: the `.mcp.json` file runs it. |

The daemon reads its settings from `$XDG_CONFIG_HOME/fold/daemon.toml`. `fld claude`, `fld new` and `fld mcp` also read the socket path from `FOLD_SOCKET`.

### Git

Each integration becomes one Git commit in the project folder, on the branch in `[git]` of the project configuration. The commit subject is the proposal title, and the body is the task title. Trailers name the proposal, the task, the reviewers and the agent or human that integrated it.

When the folder has no uncommitted changes, fold moves the checked-out branch and the files to the new commit. When you have uncommitted changes, or commits made outside fold, fold waits. Commit or stash your changes, or run `fld pull`.

| Command | What it does |
|---|---|
| `fld remote set <name> <url> [<branch>]` | Adds or changes a remote. Without `<branch>`, fold uses the branch of the remote `HEAD`. The URL can be a local path. |
| `fld remote remove <name>` | Deletes a remote. |
| `fld remote list` | Shows the remotes with their branch and URL. `fld remote` does the same. |
| `fld push [<name>]` | Pushes the history of the project to the remote, one commit for each revision, without force. A local path that does not exist becomes a new bare repository. |
| `fld pull [<name>]` | Brings the commits of the remote into the project. A conflict becomes a sync task for the agents. |

Without `<name>`, `fld push` and `fld pull` use the only remote. When the project branch has commits made outside fold, `fld pull` without a name brings those in first. `fld push` and `fld pull` act as the human in `FOLD_HUMAN`, or else `USER` or `LOGNAME`. Git uses your own credentials and configuration.

### Web sign-in

On `127.0.0.1` you do not sign in to the web interface. On a different address, each human signs in with a token.

| Command | What it does |
|---|---|
| `fld token create <human>` | Makes a sign-in token for a human and shows it one time. `--label <text>` gives the token a name. |
| `fld token list` | Shows the tokens that are not revoked: id, human, creation and last use. It never shows the token. |
| `fld token revoke <id>` | Revokes a token, such as `wt-3`, and ends each web session of that token. |

### Hooks

Claude Code runs `fld hook session-start`, `fld hook stop` and `fld hook pre-tool-use` from `.claude/settings.json`. You do not type them, and `fld --help` does not show them. They act only in a session that has `FOLD_AGENT` set, such as the main agent and the workers. The `pre-tool-use` hook stops a file edit in the project folder outside a workspace in `.fold/ws/`. This rule includes the main agent. A `claude` session that you start without `FOLD_AGENT` edits the folder as usual.

## The web interface

Open <http://127.0.0.1:7420>. Each project has these screens:

| Screen | What you can do |
|---|---|
| Board | See the tasks in five columns, with filters and search. Open a card to see its description, checklist, dependencies, workspace and proposal. Make a task or a human task, mark it for human review, set or clear a blocker, mark a human task Done, send a task back or archive it. |
| Activity | See each event in the project when it occurs, and the full history. |
| Proposals | Read each proposal and its diff, comment on lines, approve, ask for changes and integrate. See the review batches. |
| Requests | Make a request with attachments. Approve its draft, ask for changes or close it. |
| Agents | See the areas, the leads and the workers. Start a lead, stop a worker, change the worker limits and answer permission questions. |
| Messages | Read the messages of the agents, and answer their questions. |
| Repository | See the revisions of the project, look at their files and compare two revisions. See the Git state, push and pull. |

Each screen shows changes live. You do not reload the page.

## Configuration

Each project keeps its settings in `.fold/config.toml`. fold writes this file with each setting and its default value in a comment. These are the settings that you change most:

| Setting | What it controls | Default |
|---|---|---|
| `[workers] task_coder` | The maximum number of task coders at one time. `0` stops all new coders. | `4` |
| `[workers] batch_reviewer` | The maximum number of batch reviewers at one time. | `3` |
| `[workers] max_per_area` | The maximum number of task coders in one area. | no limit |
| `[integration] automatic` | If `true`, fold integrates an approved proposal itself. With `false`, the review lead, a batch reviewer, the main agent or a human integrates it. | `true` |
| `[git] branch` | The branch that gets one commit for each integration. | `main` |
| `[remotes.<name>]` | A Git remote: `url`, `branch` and `push_on_integrate`. `fld remote set` writes it. | none |
| `[roles.<role>] model` | The Claude model of a worker role, given to Claude Code as `--model`. | Claude Code default |
| `[roles.<role>] idle_minutes` | How long a task coder or batch reviewer can be idle before fold stops it. | `15` |
| `[roles.<role>] handoff_at` | The percent of the context window at which a lead writes a hand-off note and starts again with a new context. | `60` |
| `[roles.<role>] cache_ttl` | The prompt cache time of a role, `"1h"` or `"5m"`. | `"1h"` for leads and batch reviewers, `"5m"` for task coders |
| `[roles.<role>] mcp` | The MCP servers that a role can use: `fold` and the servers in the project `.mcp.json`. | all |

The worker roles are `area_lead`, `review_lead`, `task_coder` and `batch_reviewer`. The worker limits do not apply to leads. Other settings control claim time, stall time, checkpoints, the largest tracked file and the time before `fld gc` deletes a closed workspace.

Each role has a prompt that comes with fold. To use your own prompt, write it in `.fold/roles/<role>.md`. This also applies to the main agent, with `.fold/roles/main.md`. fold always adds its shared authority and recovery rules after your text.

The main agent gives each area a profile: instructions, a model and MCP servers for the workers of that area. Ask the main agent to change a profile.

## Your own agent sessions

You can open a Claude Code session by hand, in its own terminal in the project folder. Give it an agent name in `FOLD_AGENT`:

```bash
FOLD_AGENT=frontend claude --name frontend
```

This session has no role. It can claim tasks, work in its workspace, send proposals and messages, comment and ask for changes. It cannot approve, reject or integrate proposals, define areas, start leads or claim requests. A human approves and integrates in the web interface, or a review lead does it.

## Build from source

Build from source for a different system, such as aarch64 Linux. You must have:

- The Rust toolchain in `rust-toolchain.toml`. rustup installs it the first time you build.
- [bun](https://bun.sh).
- git 2.36 or newer.

In a source checkout, build the web application first, because the binary contains it:

```bash
make web-build
cargo build --release --locked -p fold-cli
mkdir -p ~/.local/bin
cp target/release/fld ~/.local/bin/fld
```

`make release` builds the static release binary for x86_64 Linux into `dist/fld-x86_64-linux`, with a `SHA256SUMS` file. It builds for the `x86_64-unknown-linux-musl` target with link-time optimisation, makes sure that the binary has no shared libraries and no program interpreter, and runs a smoke test. It needs a musl C compiler for the bundled SQLite and zstd: install it with `sudo apt install musl-tools`. rustup installs the musl target from `rust-toolchain.toml`.

To change only the web interface, run `bun run build` in `web/`, then `fld serve --web-dir web/dist`. The daemon then serves that folder, so you do not build a new binary.

## Develop fold

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
make check      # run all checks: format, clippy, tests, coverage, tsc, eslint, vitest and the builds
```

`make build`, `make dev`, the checks and the tests use the dev profile for your system, so they compile fast. Only `make release` does the slow static musl build.

`make help` shows all targets. Other common targets are `make fmt`, `make check-rust`, `make check-web`, `make acceptance` and `make gen-api`.

To run the tests of one file in `crates/<crate>/tests/`, select its module:

```bash
cargo nextest run -p <crate> --test integration -E 'test(/^<module>::/)'
```

[AGENTS.md](AGENTS.md) has the rules for code in this repository.

### Benchmarks

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

## License

fold is free software under the [GNU Affero General Public License v3.0](LICENSE).
