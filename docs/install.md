# Install fold

This page takes a person with a fresh host to a running daemon, an initialised project and the web interface.

## Requirements

- For the release binary: x86_64 Linux with glibc 2.39 or newer, the version of the machine that builds it (Ubuntu 24.04, Debian 13). `ldd --version` prints the version of a host.
- For any other device or operating system, aarch64 Linux included: the Rust toolchain pinned in `rust-toolchain.toml`, [bun](https://bun.sh) and a source checkout.
- Claude Code, installed and signed in.
- systemd, only for the service unit on Linux.
- `git`, for the project folder's own Git repository, which `fld init` creates and every integration commits to, and your own Git credentials, only for `fld remote`, `fld pull` and `fld push`.

## Other targets

In a source checkout, build the web application, then build `fld` for the host:

```bash
make web-build
cargo build --release --locked -p fold-cli
```

The binary is `target/release/fld` (in the directory that `cargo` reports as its target directory when a shared one is set). The web application must be built first, because the binary embeds `web/dist`. From then on the commands in this page are the same. `fld doctor` names any platform feature that is missing on the host, and `fld` runs without it.

## Install

Copy `fld-x86_64-linux` from `dist/` together with `SHA256SUMS`, make sure `~/.local/bin` is on `PATH`, then:

```bash
sha256sum --check --ignore-missing SHA256SUMS
chmod +x fld-x86_64-linux
mkdir -p ~/.local/bin
mv fld-x86_64-linux ~/.local/bin/fld
fld --version
```

`fld --version` prints `fld` and the version number.

## First project

```bash
fld init ~/myapp
cd ~/myapp
```

The folder `~/myapp` is the project: its files are fold main and its `.git` gets one commit per integration. For code that is already in a Git repository, run `fld init --from-git` in that repository instead; it keeps the history. fold keeps its data and the agents' workspaces in `.fold/`.

Then start fold and the main agent from the project root:

```bash
fld claude
```

`fld claude` starts the daemon when it is not running, then your own Claude Code session as the agent `main`, the main agent of the project. Talk to it: tell it what to build, and it turns your words into tasks, defines the areas, and starts an area lead for each area and a review lead. The leads start short-lived coders and reviewers, and fold integrates each approved proposal with one Git commit. When the review lead runs, run `fld agent tag review-lead integrator` once, so its approvals integrate. `fld claude --name <name>` gives the main agent another name, and the arguments after `--` go to `claude`, such as `fld claude -- --model opus`.

To work with your own sessions instead of worker agents, start one Claude Code session per agent, each in its own terminal, from the project root, and run `fld agent tag review integrator` once, so the review agent can integrate:

```bash
FOLD_AGENT=frontend claude --name frontend
FOLD_AGENT=backend claude --name backend
FOLD_AGENT=review claude --name review
```

## Service

On Linux with systemd:

```bash
fld setup
loginctl enable-linger "$USER"
fld doctor
```

`enable-linger` lets the daemon outlive your login. `fld doctor` checks the host and names the fix for every problem. On a host without a supported service manager, `fld doctor` says so, and you start `fld serve` with the host's own tools.

## Check it with real sessions

[docs/acceptance.md](acceptance.md) walks through the acceptance scenarios with three real Claude Code sessions, step by step, with the expected result of each step.

## Web interface

Open `http://127.0.0.1:7420`. To serve another address, run `fld serve --listen <addr>`; people then sign in with a token from `fld token create`.

## Upgrade

Replace the binary at `~/.local/bin/fld`, then restart the unit that `fld setup` wrote.

## Changing only the web interface

Run `bun run build` in `web/`, then `fld serve --web-dir <path-to-dist>`. That folder is served instead of the embedded copy, so a web change needs no new binary.
