#!/bin/sh
# Runs scripts/install.sh against a local fake download server. Run: sh scripts/install-test.sh
# shellcheck disable=SC2016,SC2034 # check() evaluates single-quoted expressions; status is read by them
set -eu

here=$(cd "$(dirname "$0")" && pwd)
installer=$here/install.sh
work=$(mktemp -d "${TMPDIR:-/tmp}/fold-install-test.XXXXXX")
server_pid=
cleanup() {
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null
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

# A systemctl stand-in that records restarts; FAKE_ACTIVE says whether the service counts as active.
cat >"$work/bin/systemctl" <<'SH'
#!/bin/sh
case "$*" in
*is-active*) [ "${FAKE_ACTIVE:-no}" = yes ] ;;
*restart*) echo "$*" >>"$FAKE_SYSTEMCTL_LOG" ;;
esac
SH
chmod +x "$work/bin/systemctl"

# publish <root> <tag> [bad]: fake release files for Linux x86_64 under <root>/<tag>.
publish() {
  dir=$1/$2
  mkdir -p "$dir"
  printf '#!/bin/sh\necho "fld %s"\n' "${2#v}" >"$dir/fld-x86_64-linux"
  sum=$(sha256sum "$dir/fld-x86_64-linux" | awk '{print $1}')
  [ "${3:-}" = bad ] && sum=0000000000000000000000000000000000000000000000000000000000000000
  printf '%s  fld-x86_64-linux\n' "$sum" >"$dir/SHA256SUMS"
}

# fresh: new home, new install folder, no releases published.
fresh() {
  rm -rf "${www:?}/r2" "$www/gh" "$www/api" "${work:?}/home" "$work/systemctl.log"
  mkdir -p "$www/r2/fold" "$www/gh/releases/download" "$www/api/releases" "$work/home"
  : >"$work/systemctl.log"
  dest=$work/home/.local/bin
}

set_latest() {
  printf '%s\n' "$1" >"$www/r2/fold/latest.txt"
  printf '{"url": "x", "tag_name": "%s", "name": "x"}\n' "$1" >"$www/api/releases/latest"
}

# run_installer <r2 base> [VAR=value ...]: the script is piped to sh as `curl ... | sh` does; output goes to $work/out, status to $status.
run_installer() {
  base=$1
  shift
  status=0
  env HOME="$work/home" FOLD_INSTALL_DIR="$dest" FOLD_DOWNLOAD_BASE="$base" FOLD_GITHUB_BASE="$gh" \
    FOLD_GITHUB_API="$api" FOLD_UNAME_S=Linux FOLD_UNAME_M=x86_64 PATH="$work/bin:$PATH" \
    FAKE_SYSTEMCTL_LOG="$work/systemctl.log" "$@" sh <"$installer" >"$work/out" 2>&1 || status=$?
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

# update, service active
publish "$www/r2/fold" v0.3.0
set_latest v0.3.0
run_installer "$r2" FAKE_ACTIVE=yes
check "update with active service" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.3.0" ]'
check "update with active service" 'grep -q "restart" "$work/systemctl.log"'
finish "update restarts an active service"

# update, service not active
publish "$www/r2/fold" v0.4.0
set_latest v0.4.0
: >"$work/systemctl.log"
run_installer "$r2" FAKE_ACTIVE=no
check "update without service" '[ "$status" = 0 ] && [ "$(version_of)" = "fld 0.4.0" ]'
check "update without service" '[ ! -s "$work/systemctl.log" ] && grep -q "Restart a running fold daemon" "$work/out"'
finish "update tells the user to restart"

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

# latest.txt that is not a release tag
fresh
publish "$www/r2/fold" v0.2.0
printf 'v0.2.0/../../x\n' >"$www/r2/fold/latest.txt"
run_installer "$r2"
check "bad latest.txt" '[ "$status" != 0 ] && grep -q "unexpected version" "$work/out" && [ ! -e "$dest/fld" ]'
printf '<html>Sign in to the network</html>\n' >"$www/r2/fold/latest.txt"
run_installer "$r2"
check "HTML latest.txt" '[ "$status" != 0 ] && grep -q "unexpected version" "$work/out" && [ ! -e "$dest/fld" ]'
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

# unsupported systems
fresh
publish "$www/r2/fold" v0.2.0
set_latest v0.2.0
run_installer "$r2" FOLD_UNAME_S=Linux FOLD_UNAME_M=riscv64
check "unsupported system" '[ "$status" != 0 ] && grep -q "github.com/omukkhq/fold" "$work/out" && [ ! -e "$dest/fld" ]'
run_installer "$r2" FOLD_UNAME_S=Darwin FOLD_UNAME_M=x86_64
check "unsupported Intel Mac" '[ "$status" != 0 ] && grep -q "source" "$work/out"'
finish "unsupported systems stop with the source build"

if [ "$failures" -gt 0 ]; then
  printf '%s check(s) failed\n' "$failures"
  exit 1
fi
printf 'all %s scenarios passed\n' "$count"
