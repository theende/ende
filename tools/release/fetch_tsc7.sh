#!/usr/bin/env bash
# Copyright 2026 the Ende authors. MIT license.
#
# Downloads the pinned TypeScript 7 compiler for one target from the
# dl.ende.sh mirror (laid out by tools/tsc7_mirror.ts), checks it and unpacks
# it. Release builds point ENDE_TSC7_DIR at the result so cli/build.rs embeds
# it; CI points ENDE_TEST_TSC7_BIN at its `tsc` for the tsc7 tests.
#
# The version comes from cli/tsc7_sums.lock. The zip must match its published
# .sha256sum, its `tsc` (`tsc.exe` on Windows) the SHA-256 the lock pins for
# the target, and the other files the lock's `# files-sha256:` manifest
# (cli/build.rs checks those two again). Any failure exits non-zero.
#
# Writes <work-dir>/tsc-<target>.zip{,.sha256sum} and unpacks the zip into
# <work-dir>/<target>/. <work-dir> must not exist yet. On success, prints the
# absolute path of <work-dir>/<target> on stdout (in Windows form on Windows,
# for native tools); everything else goes to stderr.
#
# Usage: fetch_tsc7.sh <target> <work-dir>

set -euo pipefail

die() {
  echo "fetch_tsc7.sh: $*" >&2
  exit 1
}

if [ $# -ne 2 ]; then
  echo "Usage: fetch_tsc7.sh <target> <work-dir>" >&2
  exit 1
fi
target=$1
work_dir=$2

lock="$(cd "$(dirname "$0")/../.." && pwd)/cli/tsc7_sums.lock"
version=$(sed -n 's/^#[[:space:]]*version:[[:space:]]*\([^[:space:]]*\).*/\1/p' "$lock")
files_sha256=$(sed -n 's/^#[[:space:]]*files-sha256:[[:space:]]*\([^[:space:]]*\).*/\1/p' "$lock")
tsc_sha256=$(awk -v t="$target" '$1 == t && NF == 4 { print $4 }' "$lock")
[ -n "$version" ] || die "$lock has no '# version:'"
[ -n "$files_sha256" ] || die "$lock has no '# files-sha256:'"
[ -n "$tsc_sha256" ] || die "$lock pins no TypeScript 7 compiler for $target"
case "$target" in
  *-windows-*) exe=tsc.exe ;;
  *) exe=tsc ;;
esac

sha256() {
  if command -v sha256sum > /dev/null; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

mkdir "$work_dir" || die "can't create $work_dir (it must not exist yet)"
zip="tsc-$target.zip"
url="https://dl.ende.sh/tsc/$version/$zip"
echo "Downloading $url" >&2
curl -fsSL --retry 3 -o "$work_dir/$zip" "$url" || die "failed to download $url"
curl -fsSL --retry 3 -o "$work_dir/$zip.sha256sum" "$url.sha256sum" ||
  die "failed to download $url.sha256sum"

read -r expected name < "$work_dir/$zip.sha256sum" || true
[ "${name:-}" = "$zip" ] ||
  die "$url.sha256sum names '${name:-}', not $zip"
actual=$(sha256 "$work_dir/$zip")
[ "$actual" = "$expected" ] ||
  die "$zip has SHA-256 $actual, but $url.sha256sum says $expected"

dir="$work_dir/$target"
mkdir "$dir"
if command -v unzip > /dev/null; then
  unzip -q "$work_dir/$zip" -d "$dir" || die "failed to unzip $zip"
else
  # Windows runners have 7-Zip but no `unzip`.
  7z x -y -o"$dir" "$work_dir/$zip" > /dev/null || die "failed to unzip $zip"
fi

[ -f "$dir/$exe" ] || die "$zip has no $exe"
actual=$(sha256 "$dir/$exe")
[ "$actual" = "$tsc_sha256" ] ||
  die "$exe in $zip has SHA-256 $actual, but $lock pins $tsc_sha256 for $target"
# The manifest cli/build.rs and tools/tsc7_mirror.ts compute: one
# `<sha256>  <name>` line per file but tsc, sorted by name in byte order.
actual=$(
  cd "$dir"
  LC_ALL=C # glob in byte order
  for name in *; do
    [ "$name" = "$exe" ] && continue
    printf '%s  %s\n' "$(sha256 "$name")" "$name"
  done | sha256 -
)
[ "$actual" = "$files_sha256" ] ||
  die "the files next to $exe in $zip have manifest SHA-256 $actual, but $lock pins $files_sha256"
chmod +x "$dir/$exe"

echo "TypeScript $version for $target: checked against $url.sha256sum and $lock" >&2
if command -v cygpath > /dev/null; then
  cygpath -m "$(cd "$dir" && pwd)"
else
  (cd "$dir" && pwd)
fi
