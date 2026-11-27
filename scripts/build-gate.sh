#!/usr/bin/env bash
# Runs one build or test command of this tree in one of a few machine-wide slots, so the agents of every workspace share the processors, the memory and the disk instead of fighting over them.
# Usage: scripts/build-gate.sh <command...>, such as scripts/build-gate.sh make clippy.
#
# - Build output goes to this tree's own target/, which overrides a shared target-dir in ~/.cargo/config.toml. Cargo gives a workspace crate the same build hash in every copy of the tree, so a shared folder would let one workspace link the fold crates another workspace built, and parallel builds would wait on its one lock.
# - sccache, when installed, caches compilations. A hit needs the same absolute paths, so it serves later builds of the same tree, such as the integration checks in .fold/checks/main, and not a first build in a new workspace.
# - FOLD_GATE_SLOTS, FOLD_GATE_JOBS, FOLD_GATE_TEST_THREADS and FOLD_GATE_MIN_FREE_GB override the defaults, which come from the processors and the memory of the machine.
set -u

tree=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)

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
test_threads=${FOLD_GATE_TEST_THREADS:-$jobs}
min_free_gb=${FOLD_GATE_MIN_FREE_GB:-20}

export CARGO_TARGET_DIR="$tree/target"
export CARGO_INCREMENTAL=0
export CARGO_BUILD_JOBS="$jobs"
export NEXTEST_TEST_THREADS="$test_threads"
export VITEST_MAX_WORKERS="$test_threads"
export FOLD_GATE_HELD=1

if [ -z "${RUSTC_WRAPPER:-}" ] && command -v sccache >/dev/null 2>&1; then
  export RUSTC_WRAPPER=sccache
  export SCCACHE_CACHE_SIZE="${SCCACHE_CACHE_SIZE:-100G}"
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
      echo "build-gate: slot taken" >&2
      # The command inherits the locked descriptor, so the slot stays held until the command and everything it started have ended.
      exec nice -n 10 "$@"
    fi
    exec {fd}>&-
  done
  if [ "$told" -eq 0 ]; then
    echo "build-gate: all ${slots} slots are in use; waiting for one" >&2
    told=1
  fi
  sleep 2
done
