# Install fold

This page takes a person with a fresh host to a running daemon, an initialised project and the web interface.

## Requirements

- For the release binary: x86_64 Linux with glibc 2.39 or newer, the version of the machine that builds it (Ubuntu 24.04, Debian 13). `ldd --version` prints the version of a host.
- For any other device or operating system, aarch64 Linux included: the Rust toolchain pinned in `rust-toolchain.toml`, [bun](https://bun.sh) and a source checkout.
- Claude Code, installed and signed in.
- systemd, only for the service unit on Linux.
- `git` and your own Git credentials, only for `fold init --from-git`, `fold export-git`, `fold remote`, `fold pull` and `fold push`.

## Other targets

In a source checkout, build the web application, then build `fold` for the host:

```bash
make web-build
cargo build --release --locked -p fold-cli
```

The binary is `target/release/fold` (in the directory that `cargo` reports as its target directory when a shared one is set). The web application must be built first, because the binary embeds `web/dist`. From then on the commands in this page are the same. `fold doctor` names any platform feature that is missing on the host, and `fold` runs without it.

## Install

Copy `fold-x86_64-linux` from `dist/` together with `SHA256SUMS`, make sure `~/.local/bin` is on `PATH`, then:

```bash
sha256sum --check --ignore-missing SHA256SUMS
chmod +x fold-x86_64-linux
mkdir -p ~/.local/bin
mv fold-x86_64-linux ~/.local/bin/fold
fold --version
```

`fold --version` prints `fold` and the version number.

## First project

```bash
fold init ~/myapp
cd ~/myapp
```

Then start one Claude Code session per agent, each in its own terminal, from the project root:

```bash
FOLD_AGENT=frontend claude --name frontend
FOLD_AGENT=backend claude --name backend
FOLD_AGENT=review claude --name review
```

Run `fold agent tag review integrator` once, so the review agent can integrate.

## Service

On Linux with systemd:

```bash
fold service install
loginctl enable-linger "$USER"
fold doctor
```

`enable-linger` lets the daemon outlive your login. `fold doctor` checks the host and names the fix for every problem. On a host without a supported service manager, `fold doctor` says so, and you start `fold serve` with the host's own tools.

## Check it with real sessions

[docs/acceptance.md](acceptance.md) walks through the acceptance scenarios with three real Claude Code sessions, step by step, with the expected result of each step.

## Web interface

Open `http://127.0.0.1:7420`. To serve another address, run `fold serve --listen <addr>`; people then sign in with a token from `fold token create`.

## Upgrade

Replace the binary at `~/.local/bin/fold`, then restart the unit that `fold service install` wrote.

## Changing only the web interface

Run `bun run build` in `web/`, then `fold serve --web-dir <path-to-dist>`. That folder is served instead of the embedded copy, so a web change needs no new binary.
