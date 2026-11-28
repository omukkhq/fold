#!/usr/bin/env bash
# Runs one build or test command of this tree in one of a few machine-wide slots, so the agents of every workspace share the processors, the memory and the disk instead of fighting over them.
# Usage: scripts/build-gate.sh <command...>, such as scripts/build-gate.sh make clippy.
#
# - Build output goes to this tree's own target/, which overrides a shared target-dir in ~/.cargo/config.toml. Cargo gives a workspace crate the same build hash in every copy of the tree, so a shared folder would let one workspace link the fold crates another workspace built, and parallel builds would wait on its one lock.
# - Where unshare works, the command runs in a private mount namespace that bind-mounts the tree at /tmp/fold-build-<uid>/tree, the same path for every tree, under the real user id. sccache's key holds absolute paths, so only then does it hit across workspaces. FOLD_GATE_FIXED_PATH=0, a missing flock or unshare, or a failing unshare builds at the tree's own path.
# - sccache, when installed, caches compilations. On the fixed path each run starts its own server inside its namespace, where the mount is visible, on a socket in a private tmpfs, and the server uses the slot's own cache folder: two sccache servers never share one folder, since each deletes the other's temporary files at start and keeps its own size count. Each cache, the fallback's shared one and each slot's, holds up to SCCACHE_CACHE_SIZE, 40G by default.
# - A debug binary built through the gate has the fixed path compiled in, such as rust-embed's folder and env!("CARGO_MANIFEST_DIR") fixtures, so run it through the gate as well.
# - FOLD_GATE_SLOTS, FOLD_GATE_JOBS, FOLD_GATE_TEST_THREADS and FOLD_GATE_MIN_FREE_GB override the defaults, which come from the processors and the memory of the machine and, for the test threads, from the slots in use when the command starts.
set -u

tree=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)

# The script runs itself again inside the namespaces: FOLD_GATE_STAGE=mount as root of the first user namespace, which may mount, then FOLD_GATE_STAGE=run under the real ids, so file permissions are checked as for the user.
case "${FOLD_GATE_STAGE:-}" in
  mount)
    mount --bind "$tree" "$FOLD_GATE_ROOT/tree" || exit 125
    mount -t tmpfs -o mode=0700 tmpfs "$FOLD_GATE_ROOT/run" || exit 125
    export FOLD_GATE_STAGE=run
    exec unshare --user --map-user="$FOLD_GATE_UID" --map-group="$FOLD_GATE_GID" "$tree/scripts/build-gate.sh" "$@"
    ;;
  run)
    cd "$FOLD_GATE_CWD" || exit 125
    use_sccache=$FOLD_GATE_SCCACHE
    unset FOLD_GATE_STAGE FOLD_GATE_ROOT FOLD_GATE_UID FOLD_GATE_GID FOLD_GATE_CWD FOLD_GATE_SCCACHE
    child=""
    server=""
    caught=""
    # The command runs in the background so a TERM or INT sent to this process alone still reaches it and the server is stopped after it. A signal before the command starts stops the server and ends the run.
    on_signal() {
      caught=$1
      if [ -n "$child" ]; then
        kill -"$1" "$child" 2>/dev/null
        return
      fi
      if [ -n "$server" ]; then
        kill -KILL "$server" 2>/dev/null
        wait "$server" 2>/dev/null
      fi
      exit $(( 128 + $2 ))
    }
    trap 'on_signal TERM 15' TERM
    trap 'on_signal INT 2' INT
    if [ "$use_sccache" = 1 ]; then
      # Not daemonized, so the server stays in the command's process group and a SIGKILL to the group also ends it: a server left over on the slot's cache folder would share it with the next run's server. It ignores INT and TERM and is stopped after the command, because bash 5.1's wait loses the command's status when the server dies in the same moment. A compile that finds no socket starts a daemonized server of its own, so the command waits for the socket and builds without sccache when it never comes.
      ( trap '' INT TERM; SCCACHE_START_SERVER=1 SCCACHE_NO_DAEMON=1 exec sccache ) </dev/null >/dev/null 2>&1 &
      server=$!
      waited=0
      while [ ! -S "$SCCACHE_SERVER_UDS" ] && [ "$waited" -lt 600 ] && kill -0 "$server" 2>/dev/null; do
        sleep 0.1
        waited=$(( waited + 1 ))
      done
      if [ ! -S "$SCCACHE_SERVER_UDS" ]; then
        kill -KILL "$server" 2>/dev/null
        wait "$server" 2>/dev/null
        server=""
        use_sccache=0
        unset RUSTC_WRAPPER SCCACHE_DIR SCCACHE_CACHE_SIZE SCCACHE_SERVER_UDS SCCACHE_IDLE_TIMEOUT
        echo "build-gate: the sccache server did not start; building without sccache" >&2
      fi
    fi
    ( trap - INT QUIT; exec "$@" ) <&0 &
    child=$!
    # A trapped signal ends the wait early, sometimes after the command has already ended, so the wait runs again for the command's own status.
    while :; do
      caught=""
      wait "$child"
      status=$?
      [ -n "$caught" ] || break
    done
    if [ "$use_sccache" = 1 ]; then
      sccache --stop-server >/dev/null 2>&1 || kill -KILL "$server" 2>/dev/null
      wait "$server" 2>/dev/null
    fi
    exit "$status"
    ;;
esac

if [ "$#" -eq 0 ]; then
  echo "usage: scripts/build-gate.sh <command...>" >&2
  exit 2
fi

# A gated command that starts another gated command, such as a make target that calls this script, already holds a slot.
if [ -n "${FOLD_GATE_HELD:-}" ]; then
  exec "$@"
fi

cpus=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)
if [ -r /proc/meminfo ]; then
  mem_gb=$(awk '/^MemTotal:/ { print int($2 / 1048576) }' /proc/meminfo)
else
  mem_gb=$(( $(sysctl -n hw.memsize 2>/dev/null || echo 8589934592) / 1073741824 ))
fi

# One slot for each 5 processors and each 6 GB of memory, the smaller of the two. Each slot gets about one and a half processors' worth of cargo jobs, at most 8: one fold build cannot keep more busy, because the daemon and its test program are single large crates.
slots_default=$(( cpus / 5 < mem_gb / 6 ? cpus / 5 : mem_gb / 6 ))
[ "$slots_default" -ge 1 ] || slots_default=1
slots=${FOLD_GATE_SLOTS:-$slots_default}
jobs_default=$(( cpus * 3 / 2 / slots ))
[ "$jobs_default" -le 8 ] || jobs_default=8
[ "$jobs_default" -ge 2 ] || jobs_default=2
jobs=${FOLD_GATE_JOBS:-$jobs_default}
min_free_gb=${FOLD_GATE_MIN_FREE_GB:-20}

export CARGO_TARGET_DIR="$tree/target"
export CARGO_INCREMENTAL=0
export CARGO_BUILD_JOBS="$jobs"
export FOLD_GATE_HELD=1

fixed_root="/tmp/fold-build-$(id -u)"
fixed=""
if [ "${FOLD_GATE_FIXED_PATH:-1}" = 0 ]; then
  why="FOLD_GATE_FIXED_PATH=0"
elif ! command -v flock >/dev/null 2>&1; then
  why="no flock"
elif ! command -v unshare >/dev/null 2>&1; then
  why="no unshare"
elif ! mkdir -p "$fixed_root/tree" "$fixed_root/run" 2>/dev/null; then
  why="cannot create $fixed_root"
elif ! why=$(unshare -r -m sh -c 'mount --bind "$1" "$2/tree" && mount -t tmpfs tmpfs "$2/run" && exec unshare --user --map-user="$3" --map-group="$4" true' sh "$tree" "$fixed_root" "$(id -u)" "$(id -g)" 2>&1); then
  why=$(printf '%s' "${why:-unshare failed}" | head -n 1)
else
  fixed="$fixed_root/tree"
fi
[ -n "$fixed" ] || echo "build-gate: fixed path unavailable (${why}); building at ${tree}" >&2

sccache_size=${SCCACHE_CACHE_SIZE:-}
use_sccache=0
set_wrapper=0
if [ -z "${RUSTC_WRAPPER:-}" ] && command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
  set_wrapper=1
fi
case "${RUSTC_WRAPPER:-}" in
  sccache | */sccache) use_sccache=1 ;;
esac
if [ "$set_wrapper" = 1 ] && [ -z "$fixed" ]; then
  export SCCACHE_CACHE_SIZE="${sccache_size:-40G}"
  # Started before a slot is taken, so the server that outlives the command never holds the slot's lock.
  sccache --start-server >/dev/null 2>&1 || true
fi

free_gb() { df -Pk "$tree" | awk 'NR == 2 { print int($4 / 1048576) }'; }
told=0
while [ "$(free_gb)" -lt "$min_free_gb" ]; do
  if [ "$told" -eq 0 ]; then
    echo "build-gate: waiting for ${min_free_gb} GB of free disk; $(free_gb) GB is free. Remove old workspaces or build folders to continue." >&2
    told=1
  fi
  sleep 30
done

if ! command -v flock >/dev/null 2>&1; then
  export NEXTEST_TEST_THREADS="${FOLD_GATE_TEST_THREADS:-$jobs}"
  export VITEST_MAX_WORKERS="$NEXTEST_TEST_THREADS"
  exec nice -n 10 "$@"
fi

# A fixed folder, not $XDG_RUNTIME_DIR or $TMPDIR, so the daemon's check runs and the agents' shells share the same locks whatever environment each one has.
locks="/tmp/fold-build-gate-$(id -u)"
mkdir -p "$locks"
told=0
while :; do
  for i in $(seq 1 "$slots"); do
    exec {fd}>"$locks/slot-$i.lock"
    if flock -n "$fd"; then
      # Most fold tests wait on SQLite and files rather than the processors, so a command that starts while few slots are in use shares the processors between those slots instead of keeping its slot's share. A command that starts later never lowers the threads of one that already runs.
      busy=1
      for j in $(seq 1 "$slots"); do
        [ "$j" -eq "$i" ] && continue
        exec {probe}>"$locks/slot-$j.lock"
        flock -n "$probe" || busy=$(( busy + 1 ))
        exec {probe}>&-
      done
      # At least 12 even when every slot is busy: the tests mostly wait on SQLite and files, so threads beyond the slot's share of processors still finish them sooner.
      test_floor=$(( jobs > 12 ? jobs : 12 ))
      test_threads=$(( cpus / busy > test_floor ? cpus / busy : test_floor ))
      test_threads=${FOLD_GATE_TEST_THREADS:-$test_threads}
      export NEXTEST_TEST_THREADS="$test_threads"
      export VITEST_MAX_WORKERS="$test_threads"
      # The command inherits the locked descriptor, so the slot stays held until the command and everything it started have ended.
      if [ -z "$fixed" ]; then
        echo "build-gate: slot taken, ${busy} of ${slots} in use, ${test_threads} test threads, building at ${tree}" >&2
        exec nice -n 10 "$@"
      fi
      echo "build-gate: slot taken, ${busy} of ${slots} in use, ${test_threads} test threads, building at ${fixed}" >&2
      cwd=$(pwd -P)
      case "$cwd/" in
        "$tree"/*) cwd="$fixed${cwd#"$tree"}" ;;
      esac
      export CARGO_TARGET_DIR="$fixed/target"
      if [ "$use_sccache" = 1 ]; then
        # Outside ~/.cache/sccache, the folder of the fallback's shared server, which would count and evict these files as its own.
        export SCCACHE_DIR="$HOME/.cache/sccache-slots/slot-$i"
        export SCCACHE_CACHE_SIZE="${sccache_size:-40G}"
        export SCCACHE_SERVER_UDS="$fixed_root/run/sccache.sock"
        # The run stops its server itself. The idle timer restarts only when a compile starts, so a shorter timeout could stop the server during one long compile, and the next compile would start a daemonized server outside the run's process group.
        export SCCACHE_IDLE_TIMEOUT=3600
      fi
      export FOLD_GATE_STAGE=mount FOLD_GATE_ROOT="$fixed_root" FOLD_GATE_UID="$(id -u)" FOLD_GATE_GID="$(id -g)" FOLD_GATE_CWD="$cwd" FOLD_GATE_SCCACHE="$use_sccache"
      exec nice -n 10 unshare -r -m "$tree/scripts/build-gate.sh" "$@"
    fi
    exec {fd}>&-
  done
  if [ "$told" -eq 0 ]; then
    echo "build-gate: all ${slots} slots are in use; waiting for one" >&2
    told=1
  fi
  sleep 2
done
