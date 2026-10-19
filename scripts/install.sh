#!/bin/sh
# Installs fld, or updates it when it is already installed; --uninstall removes it.
#   curl -fsSL <url>/install.sh | sh
#   curl -fsSL <url>/install.sh | sh -s -- --uninstall
# FOLD_VERSION=v0.x.y picks a release, FOLD_INSTALL_DIR picks the folder (default ~/.local/bin).
# Everything that acts runs from main, called on the last line, so a download cut short by `curl | sh` runs nothing.
set -eu

# The one value to change when the public download host moves.
DEFAULT_DOWNLOAD_BASE="https://dl.omukk.dev"
SOURCE_URL="https://github.com/omukkhq/fold"

say() { printf '%s\n' "$*"; }
die() {
  printf 'fold install: %s\n' "$*" >&2
  exit 1
}

tmp=
staged=
cleanup() {
  [ -n "$tmp" ] && rm -rf "$tmp"
  [ -n "$staged" ] && rm -f "$staged"
  return 0
}

# fetch <url> <file> [progress]. With a third argument, the transfer shows how much is downloaded, when main found that stderr is a terminal and the downloader can show it; the argument only changes the output. curl never follows an https URL to plain http; only a base the caller overrode to http is fetched over http.
fetch() {
  case "$1" in
  https://*) protos='=https' ;;
  *) protos='=http,https' ;;
  esac
  case "$downloader" in
  curl)
    if [ -n "${3:-}" ] && [ -n "$show_progress" ]; then
      curl -fSL --progress-bar --proto "$protos" --tlsv1.2 --connect-timeout 10 --max-time 600 --retry 2 -o "$2" "$1"
    else
      curl -fsSL --proto "$protos" --tlsv1.2 --connect-timeout 10 --max-time 600 --retry 2 -o "$2" "$1"
    fi
    ;;
  gnu-wget)
    if [ -n "${3:-}" ] && [ -n "$show_progress" ] && [ -n "$wget_bar" ]; then
      wget -q --show-progress --connect-timeout=10 --read-timeout=600 --tries=2 -O "$2" "$1"
    else
      wget -q --connect-timeout=10 --read-timeout=600 --tries=2 -O "$2" "$1"
    fi
    ;;
  *) wget -q -T 60 -O "$2" "$1" ;;
  esac
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# The binary and SHA256SUMS always come from the same base. Only the binary shows progress. Its errors stay hidden unless stderr is a terminal, so a person sees why a host failed.
get_from() {
  if [ -n "$show_progress" ]; then
    fetch "$1/$asset" "$tmp/$asset" progress
  else
    fetch "$1/$asset" "$tmp/$asset" 2>/dev/null
  fi && fetch "$1/SHA256SUMS" "$tmp/SHA256SUMS" 2>/dev/null
}

# An XDG variable counts only when it holds an absolute path, as fld reads it.
xdg_or() {
  case "$1" in
  /*) printf '%s\n' "$1" ;;
  *) printf '%s\n' "$2" ;;
  esac
}

# The folders fld uses: settings, data with the project registry, and the runtime folder of the socket and the locks.
host_paths() {
  config_dir=$(xdg_or "${XDG_CONFIG_HOME:-}" "$HOME/.config")
  data_dir=$(xdg_or "${XDG_DATA_HOME:-}" "$HOME/.local/share")/fold
  runtime_dir=$(xdg_or "${XDG_RUNTIME_DIR:-}" "")
  if [ -n "$runtime_dir" ]; then
    runtime_dir=$runtime_dir/fold
  elif [ "$os" = Darwin ]; then
    runtime_dir="$HOME/Library/Application Support/fold/run"
  fi
}

# The pids of this user's `<fld> serve` processes; a script stand-in runs under its interpreter, so the binary may follow another word. The path goes through the environment and is matched as text, so a path with spaces or backslashes matches too.
daemon_pids() {
  ps -A -o pid= -o uid= -o args= 2>/dev/null |
    FOLD_EXE="$1" awk -v uid="$(id -u)" '$2 == uid {
      args = $0
      sub(/^[ \t]*[0-9]+[ \t]+[0-9]+[ \t]+/, "", args)
      if (index(" " args " ", " " ENVIRON["FOLD_EXE"] " serve ") > 0) print $1
    }'
}

# A daemon that `fld claude` started stays a zombie of that session after it exits, so a zombie counts as gone.
alive() {
  state=$(ps -o stat= -p "$1" 2>/dev/null | tr -d ' ')
  case "$state" in
  '' | Z*) return 1 ;;
  *) return 0 ;;
  esac
}

daemon_running() {
  [ -n "$(daemon_pids "$1")" ] || { [ -n "$runtime_dir" ] && [ -S "$runtime_dir/fold.sock" ]; }
}

# ask <question> <default y or n>: yes or no from the terminal on fd 3. Enter takes the default; the end of the input or two answers that are neither count as no.
ask() {
  tries=0
  while [ "$tries" -lt 2 ]; do
    tries=$((tries + 1))
    if [ "$2" = y ]; then printf '%s [Y/n] ' "$1"; else printf '%s [y/N] ' "$1"; fi
    if ! IFS= read -r answer <&3; then
      say ""
      return 1
    fi
    case $(printf '%s' "$answer" | tr '[:upper:]' '[:lower:]') in
    '') [ "$2" = y ] && return 0 || return 1 ;;
    y | yes) return 0 ;;
    n | no) return 1 ;;
    esac
  done
  return 1
}

# Stops the daemon through fld's own clean stop, so its workers keep their sessions; when that stop fails, fld says why on stderr. An fld without that stop, or a daemon that is still there after it, gets SIGTERM from here, which fld serve takes the same way. Returns 1 when a daemon may still run.
stop_daemon() {
  stop_failed=
  if [ -x "$target" ] && "$target" restart --help </dev/null >/dev/null 2>&1; then
    if "$target" restart --stop </dev/null; then
      return 0
    fi
    stop_failed=y
  fi
  pids=$(daemon_pids "$target")
  if [ -z "$pids" ]; then
    if [ -n "$stop_failed" ]; then
      return 1
    fi
    return 0
  fi
  # shellcheck disable=SC2086 # one pid per word
  kill -TERM $pids 2>/dev/null || true
  waited=0
  still_running=
  for pid in $pids; do
    while alive "$pid" && [ "$waited" -lt 30 ]; do
      sleep 1
      waited=$((waited + 1))
    done
    if alive "$pid"; then
      say "The fold daemon (pid $pid) did not stop within 30 seconds of the SIGTERM; stop it with \`kill $pid\`."
      still_running=y
    else
      say "Stopped the fold daemon (pid $pid)"
    fi
  done
  if [ -n "$still_running" ]; then
    return 1
  fi
  return 0
}

# Removes the systemd user unit or the launchd agent that `fld setup` installed.
remove_service() {
  if [ "$os" = Darwin ]; then
    agent="$HOME/Library/LaunchAgents/dev.omukk.fold.plist"
    [ -e "$agent" ] || return 0
    if command -v launchctl >/dev/null 2>&1; then
      launchctl bootout "gui/$(id -u)/dev.omukk.fold" >/dev/null 2>&1 || true
    fi
    rm -f "$agent"
    say "Removed the launchd agent $agent"
    return 0
  fi
  unit="$config_dir/systemd/user/fold.service"
  [ -e "$unit" ] || return 0
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now fold.service >/dev/null 2>&1 || true
  fi
  rm -f "$unit" "$config_dir/systemd/user/default.target.wants/fold.service"
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
  say "Removed the systemd user unit $unit"
}

# fld removes its own entries from Claude Code's user configuration and keeps every other entry; an fld older than that command leaves them.
remove_claude() {
  claude_files="$HOME/.claude.json and $HOME/.claude/settings.json"
  case "${CLAUDE_CONFIG_DIR:-}" in
  /*) claude_files="$CLAUDE_CONFIG_DIR/.claude.json and $CLAUDE_CONFIG_DIR/settings.json" ;;
  esac
  if [ -x "$target" ] && removed=$("$target" setup --remove </dev/null 2>/dev/null); then
    say "$removed"
  else
    say "fld could not remove fold from Claude Code: remove the fold MCP server, the mcp__fold permission and the fld hook commands from $claude_files yourself."
  fi
}

# remove_dir <path> <what>
remove_dir() {
  [ -n "$1" ] && [ -e "$1" ] || return 0
  case "$1" in
  /?*) ;;
  *) return 0 ;;
  esac
  rm -rf "${1:?}"
  say "Removed $2 $1"
}

uninstall() {
  [ -n "${HOME:-}" ] || die "HOME is not set."
  os=${FOLD_UNAME_S:-$(uname -s)}
  host_paths
  target="$install_dir/fld"
  # Without FOLD_INSTALL_DIR, an fld that is not in the default folder is the one on PATH.
  if [ -z "${FOLD_INSTALL_DIR:-}" ] && [ ! -e "$target" ]; then
    on_path=$(command -v fld 2>/dev/null || true)
    case "$on_path" in
    /*)
      target=$on_path
      install_dir=$(dirname "$on_path")
      ;;
    esac
  fi
  remove_bin=y
  remove_conf=
  remove_data=
  # FOLD_TTY exists so the tests can answer the questions; `curl | sh` gives the script on stdin, so the answers come from the terminal.
  tty=${FOLD_TTY:-/dev/tty}
  if (exec 3<"$tty") 2>/dev/null; then
    exec 3<"$tty"
    ask "Stop the fold daemon, remove its service and remove fld from $install_dir?" y || remove_bin=
    ask "Remove fold from your Claude Code configuration (the fold MCP server, the fold tools' permission and fold's hooks)?" y && remove_conf=y
    ask "Remove fold's settings in $config_dir/fold and fold's data with the project registry in $data_dir?" n && remove_data=y
    exec 3<&-
  else
    say "No terminal to ask on: removing only fld and its service. fold's entries in your Claude Code configuration and fold's settings and data stay; run the uninstaller on a terminal to choose them too."
  fi

  [ -n "$remove_conf" ] && remove_claude
  # The daemon holds the socket and the locks in the runtime folder and the registry in the data folder, so it stops before either goes, also when fld stays.
  daemon_stopped=y
  if [ -n "$remove_bin" ] || [ -n "$remove_data" ]; then
    stop_daemon || daemon_stopped=
  fi
  if [ -n "$remove_bin" ]; then
    remove_service
    if [ -e "$target" ]; then
      rm -f "$target"
      say "Removed $target"
    else
      say "No fld in $install_dir"
    fi
  fi
  if [ -n "$remove_data" ]; then
    remove_dir "$config_dir/fold" "fold's settings"
    if [ -z "$daemon_stopped" ]; then
      say "A fold daemon may still run, so fold's data in $data_dir and its runtime folder stay. Stop the daemon, then run the uninstaller again to remove them."
    else
      remove_dir "$data_dir" "fold's data"
      remove_dir "$runtime_dir" "fold's runtime folder"
      if [ "$os" = Darwin ]; then
        rmdir "$HOME/Library/Application Support/fold" 2>/dev/null || true
        if [ -e "$HOME/Library/Logs/fold.log" ]; then
          rm -f "$HOME/Library/Logs/fold.log"
          say "Removed the daemon log $HOME/Library/Logs/fold.log"
        fi
      fi
    fi
  fi
  say "Each project's .fold folder stays with the project; delete it yourself to remove that project's fold data."
}

main() {
  download_base=${FOLD_DOWNLOAD_BASE:-$DEFAULT_DOWNLOAD_BASE}
  github_base=${FOLD_GITHUB_BASE:-$SOURCE_URL}
  github_api=${FOLD_GITHUB_API:-https://api.github.com/repos/omukkhq/fold}
  if [ -n "${FOLD_INSTALL_DIR:-}" ]; then
    install_dir=$FOLD_INSTALL_DIR
  elif [ -n "${HOME:-}" ]; then
    install_dir=$HOME/.local/bin
  else
    die "HOME is not set; set FOLD_INSTALL_DIR."
  fi

  case "${1:-}" in
  '') ;;
  --uninstall)
    uninstall
    exit 0
    ;;
  *) die "unknown option $1; the only option is --uninstall." ;;
  esac

  trap cleanup EXIT
  trap 'exit 1' HUP INT TERM

  if command -v curl >/dev/null 2>&1; then
    downloader=curl
  elif command -v wget >/dev/null 2>&1; then
    # BusyBox wget knows only a few options.
    if wget --version 2>/dev/null | grep -q 'GNU Wget'; then
      downloader=gnu-wget
    else
      downloader=wget
    fi
  else
    die "curl or wget is needed."
  fi

  # Progress goes to stderr and only to a person: a pipe or a log keeps the quiet output.
  show_progress=
  wget_bar=
  if [ -t 2 ]; then
    show_progress=1
    # GNU Wget before 1.16 has no --show-progress.
    if [ "$downloader" = gnu-wget ] && wget --help 2>&1 | grep -q -- '--show-progress'; then
      wget_bar=1
    fi
  fi

  # FOLD_UNAME_S and FOLD_UNAME_M exist so the tests can fake another system.
  os=${FOLD_UNAME_S:-$(uname -s)}
  arch=${FOLD_UNAME_M:-$(uname -m)}
  case "$os/$arch" in
  Linux/x86_64 | Linux/amd64) asset=fld-x86_64-linux ;;
  Linux/aarch64 | Linux/arm64) asset=fld-aarch64-linux ;;
  Darwin/arm64) asset=fld-aarch64-macos ;;
  *) die "no prebuilt fld for $os $arch. Build it from source: $SOURCE_URL" ;;
  esac

  command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 ||
    die "sha256sum or shasum is needed to check the download."

  tmp=$(mktemp -d "${TMPDIR:-/tmp}/fold-install.XXXXXX")

  tag=${FOLD_VERSION:-}
  if [ -z "$tag" ]; then
    # Each platform has its own latest version: a release whose build failed for one platform leaves that platform on its last release.
    say "Looking for the latest version for $asset"
    if fetch "$download_base/fold/latest/$asset.txt" "$tmp/latest" 2>/dev/null; then
      tag=$(tr -d ' \t\r\n' <"$tmp/latest")
    else
      say "  $download_base did not answer; asking GitHub"
      fetch "$github_api/releases/latest" "$tmp/latest.json" 2>/dev/null ||
        die "could not find the latest version. Set FOLD_VERSION=v0.x.y to pick one."
      tag=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$tmp/latest.json" | head -n 1)
      grep -Eq "\"name\"[[:space:]]*:[[:space:]]*\"$asset\"" "$tmp/latest.json" ||
        die "the latest GitHub release $tag has no $asset. Set FOLD_VERSION to an earlier version that has one."
    fi
  fi
  # The tag goes into download URLs, so it must be a plain release tag.
  printf '%s\n' "$tag" | grep -Eqx 'v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?' ||
    die "unexpected version '$tag'. A version looks like v0.1.0."
  version=${tag#v}

  target="$install_dir/fld"
  current=
  if [ -x "$target" ]; then
    current=$("$target" --version </dev/null 2>/dev/null | awk '{print $NF}' || true)
    if [ "$current" = "$version" ]; then
      say "fld $version is already installed in $install_dir."
      exit 0
    fi
  fi

  say "Downloading fld $version ($asset)"
  if ! get_from "$download_base/fold/$tag"; then
    say "  $download_base did not answer; using GitHub"
    get_from "$github_base/releases/download/$tag" ||
      die "could not download $asset $tag. Check the version and your network."
  fi

  expected=$(awk -v name="$asset" '{ f = $2; sub(/\r$/, "", f); sub(/^\*/, "", f); if (f == name) { print $1; exit } }' "$tmp/SHA256SUMS")
  [ -n "$expected" ] || die "SHA256SUMS has no entry for $asset."
  actual=$(sha256 "$tmp/$asset")
  if [ "$actual" != "$expected" ]; then
    die "checksum of $asset does not match SHA256SUMS. Nothing was installed."
  fi
  say "Checksum is correct"

  mkdir -p "$install_dir"
  staged=$(mktemp "$install_dir/.fld.XXXXXX")
  cp "$tmp/$asset" "$staged"
  chmod 755 "$staged"
  if [ "$os" = Darwin ] && command -v xattr >/dev/null 2>&1 && xattr -p com.apple.quarantine "$staged" >/dev/null 2>&1; then
    xattr -d com.apple.quarantine "$staged" 2>/dev/null || true
    say "Removed the macOS quarantine mark; an unsigned binary may need this to start."
  fi
  mv -f "$staged" "$target"
  staged=

  if [ -n "$current" ]; then
    say "Updated fld $current -> $version in $install_dir"
    if [ -n "${HOME:-}" ]; then
      host_paths
      if daemon_running "$target"; then
        say "A fold daemon is running: run \`fld restart\` to use the new version."
      fi
    fi
  else
    say "Installed fld $version in $install_dir"
  fi

  case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *) say "Add $install_dir to your PATH, for example: export PATH=\"$install_dir:\$PATH\"" ;;
  esac
}

main "$@"
