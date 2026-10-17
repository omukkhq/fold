#!/bin/sh
# Installs fld, or updates it when it is already installed.
#   curl -fsSL <url>/install.sh | sh
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

# fetch <url> <file>. curl never follows an https URL to plain http; only a base the caller overrode to http is fetched over http.
fetch() {
  case "$1" in
  https://*) protos='=https' ;;
  *) protos='=http,https' ;;
  esac
  case "$downloader" in
  curl) curl -fsSL --proto "$protos" --tlsv1.2 --connect-timeout 10 --max-time 600 --retry 2 -o "$2" "$1" ;;
  gnu-wget) wget -q --connect-timeout=10 --read-timeout=600 --tries=2 -O "$2" "$1" ;;
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

# The binary and SHA256SUMS always come from the same base.
get_from() {
  fetch "$1/$asset" "$tmp/$asset" && fetch "$1/SHA256SUMS" "$tmp/SHA256SUMS"
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
  if ! get_from "$download_base/fold/$tag" 2>/dev/null; then
    say "  $download_base did not answer; using GitHub"
    get_from "$github_base/releases/download/$tag" 2>/dev/null ||
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
    if command -v systemctl >/dev/null 2>&1 && systemctl --user is-active --quiet fold.service 2>/dev/null; then
      if systemctl --user restart fold.service; then
        say "Restarted fold.service"
      else
        say "Could not restart fold.service; run: systemctl --user restart fold.service"
      fi
    else
      say "Restart a running fold daemon to use the new version."
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
