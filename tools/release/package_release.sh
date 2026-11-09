#!/usr/bin/env bash
# Copyright 2026 the Ende authors. MIT license.
#
# Packages the release binaries for one target into the asset names the
# clients download:
#
#   ende-<target>.zip              `ende upgrade` (cli/tools/upgrade.rs ARCHIVE_NAME)
#   ende-<target>.sha256sum        SHA-256 of the extracted `ende`, checked after
#                                  a delta upgrade (`<hash>  <file>` form)
#   ende-<target>.zip.sha256sum
#   endert-<target>.zip            `ende compile` base binary
#   libendert-<target>.zip         desktop runtime (cli/standalone/binary.rs
#                                  runtime_archive_name)
#   <zip>.sha256sum                for each runtime zip
#   <patch>.sha256sum              for each `*.bsdiff` already in <out-dir>
#
# Each zip holds its binary, LICENSE.md and NOTICE at its root. LICENSE.md is
# the repo's (NOTICE refers to it). NOTICE is the repo's NOTICE followed by
# <typescript-notice>, the NOTICE.txt of the TypeScript 7 compiler embedded in
# `ende` (in the directory tools/release/fetch_tsc7.sh unpacks). Run it on the
# binaries exactly as they will ship (stripped and signed).
#
# Usage: package_release.sh <target> <bin-dir> <out-dir> <typescript-notice>

set -euo pipefail

if [ $# -ne 4 ]; then
  echo "Usage: package_release.sh <target> <bin-dir> <out-dir> <typescript-notice>" >&2
  exit 1
fi
target=$1
bin_dir=$(cd "$2" && pwd)
mkdir -p "$3"
out_dir=$(cd "$3" && pwd)
typescript_notice=$4
repo_dir=$(cd "$(dirname "$0")/../.." && pwd)
if [ ! -f "$typescript_notice" ]; then
  echo "Missing $typescript_notice" >&2
  exit 1
fi

case "$target" in
  *-windows-*) exe_ext=.exe lib_file=endert.dll ;;
  *-apple-darwin) exe_ext="" lib_file=libendert.dylib ;;
  *-linux-*) exe_ext="" lib_file=libendert.so ;;
  *)
    echo "Unknown target: $target" >&2
    exit 1
    ;;
esac

sha256() {
  if command -v sha256sum > /dev/null; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# write_checksum <path> <checksum-file>: writes `<hash>  <file-name>` (what
# `sha256sum` prints and `fetch_sha256_from_url` in cli/tools/upgrade.rs
# parses) to <out-dir>/<checksum-file>.
write_checksum() {
  local path=$1 checksum_file=$2
  printf '%s  %s\n' "$(sha256 "$path")" "$(basename "$path")" \
    > "$out_dir/$checksum_file"
}

notice_dir=$(mktemp -d)
trap 'rm -rf "$notice_dir"' EXIT
{
  cat "$repo_dir/NOTICE"
  printf '\n%s\n\n' "=========================================================================="
  printf '%s\n' "TypeScript 7 compiler: NOTICE.txt" \
    "---------------------------------" "" \
    "The NOTICE.txt of the TypeScript 7 package (see the TypeScript 7" \
    "section above), unmodified:" ""
  cat "$typescript_notice"
} > "$notice_dir/NOTICE"

# add_to_zip <dir> <archive-name> <file-name>: adds <dir>/<file-name> to
# <out-dir>/<archive-name>, at its root.
add_to_zip() {
  local dir=$1 archive=$2 file=$3
  if command -v zip > /dev/null; then
    (cd "$dir" && zip -q -9 "$out_dir/$archive" "$file")
  else
    # Windows runners have 7-Zip but no `zip`.
    (cd "$dir" && 7z a -tzip -mx=9 "$out_dir/$archive" "$file" > /dev/null)
  fi
}

# zip_file <archive-name> <file-name>: zips <bin-dir>/<file-name>, LICENSE.md
# and NOTICE into <out-dir>/<archive-name> and checksums the zip.
zip_file() {
  local archive=$1 file=$2
  if [ ! -f "$bin_dir/$file" ]; then
    echo "Missing $bin_dir/$file" >&2
    exit 1
  fi
  rm -f "$out_dir/$archive"
  add_to_zip "$bin_dir" "$archive" "$file"
  add_to_zip "$repo_dir" "$archive" LICENSE.md
  add_to_zip "$notice_dir" "$archive" NOTICE
  write_checksum "$out_dir/$archive" "$archive.sha256sum"
  echo "Packaged $archive"
}

ende_file="ende$exe_ext"
zip_file "ende-$target.zip" "$ende_file"
# Checksum of the extracted binary, named after the zip without `.zip`.
write_checksum "$bin_dir/$ende_file" "ende-$target.sha256sum"

zip_file "endert-$target.zip" "endert$exe_ext"
zip_file "libendert-$target.zip" "$lib_file"

for patch in "$out_dir"/*.bsdiff; do
  [ -e "$patch" ] || continue
  write_checksum "$patch" "$(basename "$patch").sha256sum"
  echo "Checksummed $(basename "$patch")"
done
