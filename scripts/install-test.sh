#!/bin/sh
# Runs scripts/install.sh against a local fake download server. Run: sh scripts/install-test.sh
# shellcheck disable=SC2016,SC2034 # check() evaluates single-quoted expressions; status is read by them
set -eu

here=$(cd "$(dirname "$0")" && pwd)
installer=$here/install.sh
work=$(mktemp -d "${TMPDIR:-/tmp}/fold-install-test.XXXXXX")
server_pid=
daemon_pid=
cleanup() {
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null
  [ -n "$daemon_pid" ] && kill "$daemon_pid" 2>/dev/null
  rm -rf "$work"
  return 0
}
trap cleanup EXIT

www=$work/www
mkdir -p "$www" "$work/bin"

port=$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
(cd "$www" && exec python3 -I -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) &
server_pid=$!
i=0
until curl -s -o /dev/null "http://127.0.0.1:$port/" 2>/dev/null; do
  i=$((i + 1))
  [ "$i" -lt 100 ] || { echo "fake server did not start" >&2; exit 1; }
  sleep 0.1
done
dead_port=$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')

r2=http://127.0.0.1:$port/r2
gh=http://127.0.0.1:$port/gh
api=http://127.0.0.1:$port/api

# systemctl and launchctl stand-ins that record every call.
printf '#!/bin/sh\necho "$*" >>"$FAKE_SYSTEMCTL_LOG"\n' >"$work/bin/systemctl"
printf '#!/bin/sh\necho "$*" >>"$FAKE_LAUNCHCTL_LOG"\n' >"$work/bin/launchctl"
chmod +x "$work/bin/systemctl" "$work/bin/launchctl"

# publish <root> <tag> [bad]: fake release files for Linux x86_64 under <root>/<tag>.
publish() {
  dir=$1/$2
  mkdir -p "$dir"
  printf '#!/bin/sh\necho "fld %s"\n' "${2#v}" >"$dir/fld-x86_64-linux"
  sum=$(sha256sum "$dir/fld-x86_64-linux" | awk '{print $1}')
  [ "${3:-}" = bad ] && sum=0000000000000000000000000000000000000000000000000000000000000000
  printf '%s  fld-x86_64-linux\n' "$sum" >"$dir/SHA256SUMS"
}

# fresh: new home, new runtime folder, new install folder, no releases published.
fresh() {
  rm -rf "${www:?}/r2" "${www:?}/gh" "${www:?}/api" "${work:?}/home" "${work:?}/run"
  mkdir -p "$www/r2/fold" "$www/gh/releases/download" "$www/api/releases" "$work/home" "$work/run"
  : >"$work/systemctl.log"
  : >"$work/launchctl.log"
  : >"$work/fld.log"
  dest=$work/home/.local/bin
}

# A daemon socket in the runtime folder, as a running daemon has. It is bound by a relative path, because a long TMPDIR makes the full path longer than a socket address may be.
fake_socket() {
  mkdir -p "$work/run/fold"
  (cd "$work/run/fold" && python3 -I -c 'import socket; socket.socket(socket.AF_UNIX).bind("fold.sock")')
}

# set_latest <tag> [asset]: the latest version of one platform (default Linux x86_64) on R2, and the latest GitHub release with that asset.
set_latest() {
  mkdir -p "$www/r2/fold/latest"
  printf '%s\n' "$1" >"$www/r2/fold/latest/${2:-fld-x86_64-linux}.txt"
  printf '{"url": "x", "tag_name": "%s", "name": "x", "assets": [{"name": "%s"}, {"name": "SHA256SUMS"}]}\n' "$1" "${2:-fld-x86_64-linux}" >"$www/api/releases/latest"
}

# run_installer <r2 base> [VAR=value ...]: the script is piped to sh as `curl ... | sh` does; output goes to $work/out, status to $status.
run_installer() {
  base=$1
  shift
  status=0
  env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u CLAUDE_CONFIG_DIR HOME="$work/home" XDG_RUNTIME_DIR="$work/run" \
    FOLD_INSTALL_DIR="$dest" FOLD_DOWNLOAD_BASE="$base" FOLD_GITHUB_BASE="$gh" \
    FOLD_GITHUB_API="$api" FOLD_UNAME_S=Linux FOLD_UNAME_M=x86_64 PATH="$work/bin:$PATH" \
    FAKE_SYSTEMCTL_LOG="$work/systemctl.log" FAKE_LAUNCHCTL_LOG="$work/launchctl.log" "$@" sh <"$installer" >"$work/out" 2>&1 || status=$?
}

# run_uninstall [VAR=value ...]: `curl ... | sh -s -- --uninstall` with every host folder below $work. FOLD_TTY names the file the answers come from; the default is no terminal.
run_uninstall() {
  status=0
  env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u CLAUDE_CONFIG_DIR HOME="$work/home" XDG_RUNTIME_DIR="$work/run" \
    FOLD_INSTALL_DIR="$dest" FOLD_UNAME_S=Linux FOLD_TTY="$work/no-terminal" PATH="$work/bin:$PATH" \
    FAKE_SYSTEMCTL_LOG="$work/systemctl.log" FAKE_LAUNCHCTL_LOG="$work/launchctl.log" FAKE_FLD_LOG="$work/fld.log" \
    "$@" sh -s -- --uninstall <"$installer" >"$work/out" 2>&1 || status=$?
}

# An installed fld stand-in that records its calls. FAKE_FLD_OLD=yes makes it an fld without `restart --stop` and `setup --remove`; FAKE_FLD_STOP_FAILS=yes makes its `restart --stop` fail; `serve` runs until SIGTERM.
stub_fld() {
  mkdir -p "$dest"
  cat >"$dest/fld" <<'SH'
#!/bin/sh
echo "$*" >>"$FAKE_FLD_LOG"
case "$1" in
--version) echo "fld 0.2.0" ;;
restart)
  [ "${FAKE_FLD_OLD:-}" = yes ] && exit 2
  [ "$2" = --help ] && exit 0
  if [ "${FAKE_FLD_STOP_FAILS:-}" = yes ]; then
    echo "fld: the daemon with pid 1 did not stop within 60 seconds" >&2
    exit 1
  fi
  echo "stopped the fold daemon 0.2.0"
  ;;
setup)
  [ "${FAKE_FLD_OLD:-}" = yes ] && exit 2
  echo "removed fold from Claude Code in a and b"
  ;;
serve)
  trap 'exit 0' TERM
  while :; do sleep 1; done
  ;;
esac
SH
  chmod +x "$dest/fld"
}

# A home with everything fld setup, fld init and the daemon leave: the unit and its link, the launchd agent and its log, settings, data and a project.
seed_home() {
  h=$work/home
  mkdir -p "$h/.config/systemd/user/default.target.wants" "$h/.config/fold" "$h/.local/share/fold" "$h/app/.fold" \
    "$h/Library/LaunchAgents" "$h/Library/Logs" "$h/Library/Application Support/fold/run" "$h/.config/other" "$work/run/fold"
  : >"$h/.config/systemd/user/fold.service"
  ln -s ../fold.service "$h/.config/systemd/user/default.target.wants/fold.service"
  : >"$h/Library/LaunchAgents/dev.omukk.fold.plist"
  : >"$h/Library/Logs/fold.log"
  echo 'listen = "127.0.0.1:7420"' >"$h/.config/fold/daemon.toml"
  : >"$h/.local/share/fold/registry.db"
  : >"$h/app/.fold/project.db"
  : >"$h/.config/other/keep"
  : >"$work/run/fold/daemon.lock"
}

# answers <line> ...: the answers file that FOLD_TTY points at.
answers() {
  printf '%s\n' "$@" >"$work/answers"
}

failures=0
check() {
  if ! eval "$2"; then
    printf 'FAIL %s: %s\n--- output ---\n' "$1" "$2"
    cat "$work/out"
    failures=$((failures + 1))
  fi
}
pass() { printf 'ok   %s\n' "$1"; }
version_of() { "$dest/fld" --version; }
count=0
mark=0
finish() {
  count=$((count + 1))
  [ "$failures" = "$mark" ] && pass "$1"
  mark=$failures
  return 0
}

# new install
fresh
publish "$www/r2/fold" v0.2.0
set_latest v0.2.0
run_installer "$r2"
check "new install" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.2.0" ]'
check "new install" 'grep -q "Installed fld 0.2.0" "$work/out"'
check "new install" 'grep -q "PATH" "$work/out"'
check "new install" '[ ! -s "$work/systemctl.log" ]'
check "new install leaves no staging file" '[ "$(ls -A "$dest")" = fld ]'
finish "new install"

# already current
run_installer "$r2"
check "already current" '[ "$status" = 0 ] && grep -q "already installed" "$work/out"'
finish "already current"

# update while a daemon runs
publish "$www/r2/fold" v0.3.0
set_latest v0.3.0
fake_socket
run_installer "$r2"
check "update with a running daemon" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.3.0" ]'
check "update with a running daemon" 'grep -q "Updated fld 0.2.0 -> 0.3.0" "$work/out" && grep -q "run \`fld restart\` to use the new version" "$work/out"'
check "update with a running daemon restarts nothing" '[ ! -s "$work/systemctl.log" ] && [ ! -s "$work/launchctl.log" ]'
finish "update with a running daemon names fld restart and restarts nothing"

# update while no daemon runs
rm -f "$work/run/fold/fold.sock"
publish "$www/r2/fold" v0.4.0
set_latest v0.4.0
run_installer "$r2"
check "update without a daemon" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.4.0" ]'
check "update without a daemon" '! grep -q "fld restart" "$work/out" && [ ! -s "$work/systemctl.log" ]'
finish "update without a daemon names no restart"

# FOLD_VERSION picks a version
publish "$www/r2/fold" v0.3.0
run_installer "$r2" FOLD_VERSION=v0.3.0
check "pinned version" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.3.0" ]'
finish "FOLD_VERSION picks a version"

# bad checksum keeps the old binary
fresh
publish "$www/r2/fold" v0.2.0
set_latest v0.2.0
run_installer "$r2"
publish "$www/r2/fold" v0.3.0 bad
set_latest v0.3.0
run_installer "$r2"
check "bad checksum" '[ "$status" != 0 ] && grep -q "checksum" "$work/out"'
check "bad checksum keeps the old binary" '[ "$(version_of)" = "fld 0.2.0" ] && [ "$(ls -A "$dest")" = fld ]'
finish "bad checksum is refused"

# SHA256SUMS that names only other files, one of them a longer name that starts with the asset name
fresh
publish "$www/r2/fold" v0.2.0
set_latest v0.2.0
printf '%s  fld-x86_64-linux.sig\n%s  fld-aarch64-linux\n' "$sum" "$sum" >"$www/r2/fold/v0.2.0/SHA256SUMS"
run_installer "$r2"
check "missing checksum entry" '[ "$status" != 0 ] && grep -q "no entry for fld-x86_64-linux" "$work/out" && [ ! -e "$dest/fld" ]'
finish "a missing checksum entry is refused"

# latest file that is not a release tag
fresh
publish "$www/r2/fold" v0.2.0
set_latest 'v0.2.0/../../x'
run_installer "$r2"
check "bad latest file" '[ "$status" != 0 ] && grep -q "unexpected version" "$work/out" && [ ! -e "$dest/fld" ]'
set_latest '<html>Sign in to the network</html>'
run_installer "$r2"
check "HTML latest file" '[ "$status" != 0 ] && grep -q "unexpected version" "$work/out" && [ ! -e "$dest/fld" ]'
run_installer "$r2" FOLD_VERSION="v0.2.0 x"
check "bad FOLD_VERSION" '[ "$status" != 0 ] && grep -q "unexpected version" "$work/out" && [ ! -e "$dest/fld" ]'
finish "a version that is not a release tag is refused"

# R2 down, GitHub answers
fresh
publish "$www/gh/releases/download" v0.5.0
set_latest v0.5.0
run_installer "http://127.0.0.1:$dead_port"
check "R2 down" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.5.0" ] && grep -q "GitHub" "$work/out"'
finish "R2 down falls back to GitHub"

# the latest for this platform is older than the latest for another platform
fresh
publish "$www/r2/fold" v0.3.0
set_latest v0.4.0 fld-aarch64-linux
set_latest v0.3.0
run_installer "$r2"
check "older platform latest" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.3.0" ]'
finish "each platform installs its own latest version"

# R2 down, and the latest GitHub release has no binary for this platform
fresh
publish "$www/gh/releases/download" v0.5.0
set_latest v0.5.0 fld-aarch64-linux
run_installer "http://127.0.0.1:$dead_port"
check "GitHub release without this platform" '[ "$status" != 0 ] && grep -q "has no fld-x86_64-linux" "$work/out" && [ ! -e "$dest/fld" ]'
finish "a GitHub release without this platform stops clearly"

# R2 lacks the files of a pinned release, GitHub has them
fresh
publish "$www/gh/releases/download" v0.6.0
run_installer "$r2" FOLD_VERSION=v0.6.0
check "R2 missing files" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.6.0" ]'
finish "missing R2 files fall back to GitHub"

# nothing answers
fresh
run_installer "http://127.0.0.1:$dead_port" FOLD_GITHUB_API="http://127.0.0.1:$dead_port"
check "everything down" '[ "$status" != 0 ] && [ ! -e "$dest/fld" ]'
finish "no source fails cleanly"

# download progress: shown on a terminal, absent in a pipe. util-linux `script` gives the installer a terminal on stderr; the BSD `script` of macOS takes other options, so the scenario runs only where `script -qec` works.
if script -qec true /dev/null >/dev/null 2>&1 </dev/null; then
  fresh
  publish "$www/r2/fold" v0.2.0
  set_latest v0.2.0
  status=0
  env HOME="$work/home" FOLD_INSTALL_DIR="$dest" FOLD_DOWNLOAD_BASE="$r2" FOLD_GITHUB_BASE="$gh" \
    FOLD_GITHUB_API="$api" FOLD_UNAME_S=Linux FOLD_UNAME_M=x86_64 PATH="$work/bin:$PATH" \
    FAKE_SYSTEMCTL_LOG="$work/systemctl.log" script -qec "sh $installer" /dev/null >"$work/out" 2>&1 </dev/null || status=$?
  check "progress on a terminal" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.2.0" ] && grep -q "100.0%" "$work/out"'
  fresh
  publish "$www/r2/fold" v0.2.0
  set_latest v0.2.0
  run_installer "$r2"
  check "no progress in a pipe" '[ "$status" = 0 ] && ! grep -q "%" "$work/out" && ! grep -q "#" "$work/out"'
  finish "download progress only on a terminal"
fi

# unsupported systems
fresh
publish "$www/r2/fold" v0.2.0
set_latest v0.2.0
run_installer "$r2" FOLD_UNAME_S=Linux FOLD_UNAME_M=riscv64
check "unsupported system" '[ "$status" != 0 ] && grep -q "github.com/omukkhq/fold" "$work/out" && [ ! -e "$dest/fld" ]'
run_installer "$r2" FOLD_UNAME_S=Darwin FOLD_UNAME_M=x86_64
check "unsupported Intel Mac" '[ "$status" != 0 ] && grep -q "source" "$work/out"'
finish "unsupported systems stop with the source build"

# unknown option
fresh
status=0
env HOME="$work/home" FOLD_INSTALL_DIR="$dest" sh -s -- --remove <"$installer" >"$work/out" 2>&1 || status=$?
check "unknown option" '[ "$status" != 0 ] && grep -q "unknown option --remove" "$work/out" && [ ! -e "$dest/fld" ]'
finish "an unknown option is refused"

# uninstall without a terminal: only fld and its service go
fresh
seed_home
stub_fld
run_uninstall
check "uninstall without a terminal" '[ "$status" = 0 ] && [ ! -e "$dest/fld" ] && grep -q "No terminal" "$work/out"'
check "uninstall without a terminal stops the daemon" 'grep -qx "restart --stop" "$work/fld.log" && grep -q "stopped the fold daemon 0.2.0" "$work/out"'
check "uninstall without a terminal keeps Claude Code" '! grep -q "setup" "$work/fld.log"'
check "uninstall without a terminal removes the unit" '[ ! -e "$h/.config/systemd/user/fold.service" ] && [ ! -L "$h/.config/systemd/user/default.target.wants/fold.service" ]'
check "uninstall without a terminal disables the unit" 'grep -qx -- "--user disable --now fold.service" "$work/systemctl.log" && grep -qx -- "--user daemon-reload" "$work/systemctl.log"'
check "uninstall without a terminal keeps settings and data" '[ -e "$h/.config/fold/daemon.toml" ] && [ -e "$h/.local/share/fold/registry.db" ] && [ -e "$work/run/fold/daemon.lock" ]'
check "uninstall keeps projects" '[ -e "$h/app/.fold/project.db" ] && grep -q "project.s .fold folder stays" "$work/out"'
check "uninstall without a terminal leaves the launchd agent on Linux" '[ -e "$h/Library/LaunchAgents/dev.omukk.fold.plist" ] && [ ! -s "$work/launchctl.log" ]'
finish "uninstall without a terminal removes only fld and its service"

# uninstall on a terminal, the default answers and yes for the data
fresh
seed_home
stub_fld
answers "" "" "y"
run_uninstall FOLD_TTY="$work/answers"
check "uninstall all" '[ "$status" = 0 ] && [ ! -e "$dest/fld" ] && [ ! -e "$h/.config/systemd/user/fold.service" ]'
check "uninstall all asks three times" '[ "$(grep -o "\[[Yy]/[Nn]\]" "$work/out" | wc -l | tr -d " ")" = 3 ]'
check "uninstall all removes fold from Claude Code before fld goes" 'grep -qx "setup --remove" "$work/fld.log" && grep -q "removed fold from Claude Code" "$work/out"'
check "uninstall all removes settings, data and the runtime folder" '[ ! -e "$h/.config/fold" ] && [ ! -e "$h/.local/share/fold" ] && [ ! -e "$work/run/fold" ]'
check "uninstall all keeps the rest" '[ -e "$h/.config/other/keep" ] && [ -d "$h/.local/share" ] && [ -e "$h/app/.fold/project.db" ]'
finish "uninstall on a terminal removes each part that the answers allow"

# uninstall on a terminal, every answer no
fresh
seed_home
stub_fld
answers n no n
run_uninstall FOLD_TTY="$work/answers"
check "uninstall nothing" '[ "$status" = 0 ] && [ -e "$dest/fld" ] && [ -e "$h/.config/systemd/user/fold.service" ] && [ -e "$h/.config/fold/daemon.toml" ]'
check "uninstall nothing calls nothing" '[ ! -s "$work/fld.log" ] && [ ! -s "$work/systemctl.log" ]'
# An answer that is neither yes nor no is asked once more; the end of the input counts as no.
answers maybe y
run_uninstall FOLD_TTY="$work/answers"
check "uninstall unclear answer" '[ "$status" = 0 ] && [ ! -e "$dest/fld" ] && [ -e "$h/.config/fold/daemon.toml" ] && ! grep -q "setup" "$work/fld.log"'
finish "uninstall answers no, unclear answers and the end of the input"

# uninstall of the data only: the daemon stops first, and fld stays
fresh
seed_home
stub_fld
answers n n y
run_uninstall FOLD_TTY="$work/answers"
check "uninstall data only" '[ "$status" = 0 ] && [ -e "$dest/fld" ] && [ -e "$h/.config/systemd/user/fold.service" ]'
check "uninstall data only stops the daemon" 'grep -qx "restart --stop" "$work/fld.log" && grep -q "stopped the fold daemon 0.2.0" "$work/out"'
check "uninstall data only removes the data" '[ ! -e "$h/.config/fold" ] && [ ! -e "$h/.local/share/fold" ] && [ ! -e "$work/run/fold" ]'
finish "uninstall of the data stops the daemon first"

# uninstall of the data while the daemon does not stop: the data and the runtime folder stay, and fld says why
fresh
seed_home
stub_fld
answers n n y
run_uninstall FOLD_TTY="$work/answers" FAKE_FLD_STOP_FAILS=yes
check "uninstall with a daemon that does not stop" '[ "$status" = 0 ] && grep -q "did not stop within 60 seconds" "$work/out"'
check "uninstall with a daemon that does not stop keeps the data" '[ -e "$h/.local/share/fold/registry.db" ] && [ -e "$work/run/fold/daemon.lock" ] && grep -q "may still run" "$work/out"'
check "uninstall with a daemon that does not stop removes the settings" '[ ! -e "$h/.config/fold" ]'
finish "uninstall keeps the data of a daemon that does not stop"

# uninstall with an fld that has neither the stop nor the Claude Code removal: SIGTERM from the script, and the files to edit by hand
fresh
seed_home
stub_fld
FAKE_FLD_LOG="$work/fld.log" "$dest/fld" serve &
daemon_pid=$!
i=0
until ps -o args= -p "$daemon_pid" 2>/dev/null | grep -q "fld serve"; do
  i=$((i + 1))
  [ "$i" -lt 100 ] || break
  sleep 0.1
done
answers y y n
run_uninstall FOLD_TTY="$work/answers" FAKE_FLD_OLD=yes
check "uninstall with an old fld" '[ "$status" = 0 ] && [ ! -e "$dest/fld" ]'
check "uninstall with an old fld stops the daemon" 'grep -q "Stopped the fold daemon (pid $daemon_pid)" "$work/out"'
check "uninstall with an old fld names the Claude Code files" 'grep -q "remove the fold MCP server" "$work/out" && grep -q "$work/home/.claude.json" "$work/out"'
# A stand-in that the installer did not stop ends here, so the test never waits forever.
kill "$daemon_pid" 2>/dev/null || true
wait "$daemon_pid" 2>/dev/null || true
daemon_pid=
finish "uninstall with an older fld stops its daemon and names the Claude Code files"

# uninstall without FOLD_INSTALL_DIR: the fld on PATH, in a folder with a space, and its daemon found by that path
fresh
seed_home
dest="$work/my apps/bin"
stub_fld
FAKE_FLD_LOG="$work/fld.log" "$dest/fld" serve &
daemon_pid=$!
i=0
until ps -o args= -p "$daemon_pid" 2>/dev/null | grep -q "fld serve"; do
  i=$((i + 1))
  [ "$i" -lt 100 ] || break
  sleep 0.1
done
answers y n n
run_uninstall FOLD_TTY="$work/answers" FOLD_INSTALL_DIR= FAKE_FLD_OLD=yes PATH="$work/bin:$dest:$PATH"
check "uninstall of the fld on PATH" '[ "$status" = 0 ] && [ ! -e "$dest/fld" ] && grep -q "Removed $dest/fld" "$work/out"'
check "uninstall of the fld on PATH stops its daemon" 'grep -q "Stopped the fold daemon (pid $daemon_pid)" "$work/out"'
# A stand-in that the installer did not stop ends here, so the test never waits forever.
kill "$daemon_pid" 2>/dev/null || true
wait "$daemon_pid" 2>/dev/null || true
daemon_pid=
finish "uninstall finds the fld on PATH and its daemon in a folder with a space"

# uninstall on macOS: the launchd agent, the runtime folder in Library and the daemon log
fresh
seed_home
stub_fld
answers y y y
run_uninstall FOLD_TTY="$work/answers" FOLD_UNAME_S=Darwin XDG_RUNTIME_DIR=
check "uninstall on macOS" '[ "$status" = 0 ] && [ ! -e "$dest/fld" ] && [ ! -e "$h/Library/LaunchAgents/dev.omukk.fold.plist" ]'
check "uninstall on macOS boots out the agent" 'grep -qx "bootout gui/$(id -u)/dev.omukk.fold" "$work/launchctl.log" && [ ! -s "$work/systemctl.log" ]'
check "uninstall on macOS removes the runtime folder and the log" '[ ! -e "$h/Library/Application Support/fold" ] && [ ! -e "$h/Library/Logs/fold.log" ] && [ -d "$h/Library/Logs" ]'
check "uninstall on macOS keeps the systemd unit" '[ -e "$h/.config/systemd/user/fold.service" ]'
finish "uninstall on macOS removes the launchd agent"

if [ "$failures" -gt 0 ]; then
  printf '%s check(s) failed\n' "$failures"
  exit 1
fi
printf 'all %s scenarios passed\n' "$count"
