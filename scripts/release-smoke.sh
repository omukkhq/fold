#!/usr/bin/env bash
# Starts a release binary against a throwaway home and checks what a first install needs.
set -u

step=start
bin=${1:-}
work=
pid=

fail() {
  echo "release-smoke: step '$step' failed: $1" >&2
  exit 1
}

cleanup() {
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill -KILL "$pid" 2>/dev/null
  fi
  if [ -n "$work" ]; then
    rm -rf "$work"
  fi
}
trap cleanup EXIT

[ -n "$bin" ] || fail "usage: release-smoke.sh <binary>"
[ -x "$bin" ] || fail "$bin is not an executable file"
bin=$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")
root=$(cd "$(dirname "$0")/.." && pwd)

step=version
want="fld $(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)"
[ "$want" != "fld " ] || fail "cannot read the workspace version from Cargo.toml"
got=$("$bin" --version 2>&1) || fail "'$bin --version' exited with an error: $got"
[ "$got" = "$want" ] || fail "expected '$want', got '$got'"

# macOS sets no XDG_RUNTIME_DIR, so there the daemon takes its runtime folder below HOME, the path a first install uses. A socket path may hold only 104 bytes on macOS and $TMPDIR is long, so the folder is made in /tmp.
if [ "$(uname -s)" = Darwin ]; then
  work=$(mktemp -d /tmp/fold-smoke.XXXXXX) || fail "cannot create a temporary folder"
  mkdir -p "$work/data" "$work/config"
  unset XDG_RUNTIME_DIR
  export XDG_DATA_HOME="$work/data" XDG_CONFIG_HOME="$work/config" HOME="$work"
  socket="$work/Library/Application Support/fold/run/fold.sock"
else
  work=$(mktemp -d) || fail "cannot create a temporary folder"
  mkdir -p "$work/data" "$work/config" "$work/runtime"
  chmod 700 "$work/runtime"
  export XDG_DATA_HOME="$work/data" XDG_CONFIG_HOME="$work/config" XDG_RUNTIME_DIR="$work/runtime" HOME="$work"
  socket="$work/runtime/fold/fold.sock"
fi

step=free-port
port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])' 2>/dev/null) ||
  fail "cannot find a free port (python3 is needed)"

# The daemon starts before init so that it listens on the chosen port; init creates the project folder and refuses one that exists.
step=serve
"$bin" serve --listen "127.0.0.1:$port" >"$work/serve.log" 2>&1 &
pid=$!
for _ in $(seq 1 100); do
  [ -S "$socket" ] && break
  kill -0 "$pid" 2>/dev/null || fail "the daemon exited at start: $(tail -n 5 "$work/serve.log")"
  sleep 0.1
done
[ -S "$socket" ] || fail "no socket at $socket after 10 seconds"

step=init
"$bin" init "$work/project" >"$work/init.log" 2>&1 || fail "fld init exited with an error: $(tail -n 5 "$work/init.log")"

base="http://127.0.0.1:$port"
step=root
code=$(curl -s -o "$work/index.html" -w '%{http_code}' "$base/") || fail "GET / could not connect"
[ "$code" = 200 ] || fail "GET / returned $code, expected 200"
grep -qi '<!doctype html' "$work/index.html" || fail "GET / did not return the HTML of the web application"

step=fallback
code=$(curl -s -o "$work/fallback.html" -w '%{http_code}' "$base/nonexistent/route") || fail "GET /nonexistent/route could not connect"
[ "$code" = 200 ] || fail "GET /nonexistent/route returned $code, expected 200"
cmp -s "$work/index.html" "$work/fallback.html" || fail "GET /nonexistent/route did not return index.html"

step=api
code=$(curl -s -o "$work/projects.json" -w '%{http_code}' -H 'Accept: application/json' "$base/api/v1/projects") || fail "GET /api/v1/projects could not connect"
[ "$code" = 200 ] || fail "GET /api/v1/projects returned $code, expected 200"
python3 -I -c 'import json, sys; json.load(open(sys.argv[1]))' "$work/projects.json" 2>/dev/null ||
  fail "GET /api/v1/projects did not return JSON"

step=stop
kill -TERM "$pid" || fail "cannot signal the daemon"
for _ in $(seq 1 100); do
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$pid" 2>/dev/null; then
  fail "the daemon still runs 10 seconds after SIGTERM"
fi
wait "$pid" 2>/dev/null
pid=
[ ! -e "$socket" ] || fail "the socket $socket still exists after the daemon stopped"

echo "release-smoke: $bin passed"
