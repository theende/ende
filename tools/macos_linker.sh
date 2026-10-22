#!/usr/bin/env bash
# Linker for aarch64-apple-darwin, set as `linker` in .cargo/config.toml. It
# links with `cc` as rustc would, except for two things:
#
# - The bundled ld64.lld is put first on PATH, as CI does, so the committed
#   `-fuse-ld=lld` finds it without any PATH setup. Install it with
#   `./tools/install_prebuilt.js ld64.lld`.
# - When `-fuse-ld=lld` is passed but the bundled LLD can't read the macOS SDK
#   being linked against (LLD 20 fails on the macOS 27 SDK's libSystem.tbd with
#   "unknown architecture arm64e.x1-macos"), it drops `-fuse-ld=lld` and the
#   LLD-only `-Wl,--icf=safe` so `cc` links with Apple's ld instead.
#
# The choice is made up front by asking LLD to link an empty dylib against the
# SDK's libSystem, never by retrying a failed link, so real link errors still
# fail. The notice below goes to stderr, which cargo only shows when a link
# fails; `otool -l` on the output shows which linker was used (LC_BUILD_VERSION
# lists the tool as LLD or LD).
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
export PATH="$root/third_party/prebuilt/mac:$PATH"

uses_lld=false
for arg in "$@"; do
  if [ "$arg" = "-fuse-ld=lld" ]; then
    uses_lld=true
  fi
done

if $uses_lld; then
  if ! command -v ld64.lld >/dev/null; then
    echo "error: ld64.lld not found; run ./tools/install_prebuilt.js ld64.lld" >&2
    exit 1
  fi
  # Without SDKROOT, cc (the xcrun shim) links against xcrun's default SDK.
  sdk="${SDKROOT:-$(xcrun --sdk macosx --show-sdk-path)}"
  if ! probe="$(ld64.lld -arch arm64 -platform_version macos 12.0 12.0 \
    -syslibroot "$sdk" -dylib -lSystem -o /dev/null 2>&1)"; then
    echo "note: bundled ld64.lld can't link against $sdk, using Apple ld (no --icf=safe): $(head -n 1 <<<"$probe")" >&2
    args=()
    for arg in "$@"; do
      case "$arg" in
        -fuse-ld=lld | -Wl,--icf=safe) ;;
        *) args+=("$arg") ;;
      esac
    done
    set -- "${args[@]}"
  fi
fi

exec cc "$@"
