#!/usr/bin/env bash
# The commit hook gets the staged Rust paths as arguments. A path outside crates/<crate>/ (the workspace manifest, the lockfile, the toolchain file, .sqlx/) can change every crate, so it lints the whole workspace.
set -euo pipefail

cargo fmt --all --check
make --no-print-directory check-inner-allow

whole=0
crates=()
for path in "$@"; do
	case "$path" in
	crates/*/*)
		rest=${path#crates/}
		crates+=("${rest%%/*}")
		;;
	*) whole=1 ;;
	esac
done

if [ "$whole" -eq 1 ] || [ "${#crates[@]}" -eq 0 ]; then
	exec cargo clippy --workspace --all-targets --all-features -- -D warnings
fi

packages=()
while IFS= read -r crate; do
	packages+=(-p "$crate")
done < <(printf '%s\n' "${crates[@]}" | sort -u)
exec cargo clippy "${packages[@]}" --all-targets --all-features -- -D warnings
