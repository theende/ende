#!/usr/bin/env bash
# Copyright 2026 the Ende authors. MIT license.
#
# Downloads the pinned dprint CLI for one target from dl.ende.sh, checks it
# and unpacks it. tools/format.js runs it into target/dprint/<version>/ and
# formats with the result.
#
# The version comes from tools/dprint_version.lock. The zip must match both its
# published .zip.sha256sum and the SHA-256 the lock pins for the target, and
# the binary in it the binary SHA-256 the lock pins. Any failure exits
# non-zero; there is no fallback to another version or to npm.
#
# Writes <work-dir>/dprint-<target>.zip{,.sha256sum} and unpacks the zip into
# <work-dir>/<target>/. <work-dir> must not exist yet, or hold the result of an
# earlier run: then nothing is downloaded, and its binary is checked against
# the lock again, quietly. On success, prints the absolute path of <work-dir>/<target>
# (which holds `dprint`, `dprint.exe` on Windows) on stdout, in Windows form on
# Windows for native tools; everything else goes to stderr.
#
# Usage: fetch_dprint.sh <target> <work-dir>

set -euo pipefail

die() {
  echo "fetch_dprint.sh: $*" >&2
  exit 1
}

if [ $# -ne 2 ]; then
  echo "Usage: fetch_dprint.sh <target> <work-dir>" >&2
  exit 1
fi
target=$1
work_dir=$2

lock="$(cd "$(dirname "$0")/../.." && pwd)/tools/dprint_version.lock"
version=$(sed -n 's/^#[[:space:]]*version:[[:space:]]*\([^[:space:]]*\).*/\1/p' "$lock")
zip_sha256=$(awk -v t="$target" '$1 == t && NF == 3 { print $2 }' "$lock")
bin_sha256=$(awk -v t="$target" '$1 == t && NF == 3 { print $3 }' "$lock")
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  die "$lock has no '# version: X.Y.Z' (found '$version')"
[[ "$zip_sha256" =~ ^[0-9a-f]{64}$ && "$bin_sha256" =~ ^[0-9a-f]{64}$ ]] ||
  die "$lock pins no dprint for $target"
case "$target" in
  *-windows-*) exe=dprint.exe ;;
  *) exe=dprint ;;
esac

# sha256 < <file>: prints the file's SHA-256. It reads stdin because GNU
# sha256sum escapes a file name with a backslash (a Windows path) by putting
# `\` before the hash.
sha256() {
  if command -v sha256sum > /dev/null; then
    sha256sum
  else
    shasum -a 256
  fi | cut -d ' ' -f 1
}

print_dir() {
  if command -v cygpath > /dev/null; then
    cygpath -m "$(cd "$1" && pwd)"
  else
    (cd "$1" && pwd)
  fi
}

dir="$work_dir/$target"
if [ -e "$work_dir" ]; then
  [ -f "$dir/$exe" ] ||
    die "$work_dir exists but has no $target/$exe; delete it and retry"
  actual=$(sha256 < "$dir/$exe")
  [ "$actual" = "$bin_sha256" ] ||
    die "$dir/$exe has SHA-256 $actual, but $lock pins $bin_sha256 for $target; delete $work_dir and retry"
  print_dir "$dir"
  exit 0
fi

mkdir "$work_dir" || die "can't create $work_dir"
zip="dprint-$target.zip"
url="https://dl.ende.sh/dprint/$version/$zip"
echo "Downloading $url" >&2
curl -fsSL --retry 3 -o "$work_dir/$zip" "$url" || die "failed to download $url"
curl -fsSL --retry 3 -o "$work_dir/$zip.sha256sum" "$url.sha256sum" ||
  die "failed to download $url.sha256sum"

read -r expected name < "$work_dir/$zip.sha256sum" || true
[ "${name:-}" = "$zip" ] ||
  die "$url.sha256sum names '${name:-}', not $zip"
actual=$(sha256 < "$work_dir/$zip")
[ "$actual" = "$expected" ] ||
  die "$zip has SHA-256 $actual, but $url.sha256sum says $expected"
[ "$actual" = "$zip_sha256" ] ||
  die "$zip has SHA-256 $actual, but $lock pins $zip_sha256 for $target"

mkdir "$dir"
if command -v unzip > /dev/null; then
  unzip -q "$work_dir/$zip" -d "$dir" || die "failed to unzip $zip"
else
  # Windows runners have 7-Zip but no `unzip`.
  7z x -y -o"$dir" "$work_dir/$zip" > /dev/null || die "failed to unzip $zip"
fi
[ -f "$dir/$exe" ] || die "$zip has no $exe"
actual=$(sha256 < "$dir/$exe")
[ "$actual" = "$bin_sha256" ] ||
  die "$exe in $zip has SHA-256 $actual, but $lock pins $bin_sha256 for $target"
chmod +x "$dir/$exe"

echo "dprint $version for $target: checked against $url.sha256sum and $lock" >&2
print_dir "$dir"
