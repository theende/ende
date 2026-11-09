#!/bin/sh
# Copyright 2026 the Ende authors. MIT license.
#
# Rebuilds `launcher` (universal arm64 + x86_64) from launcher.c with Apple's
# clang. Run on macOS, then update the SHA-256s in README.md and cli/build.rs.
set -eu
cd "$(dirname "$0")"
/usr/bin/xcrun clang -Os -Wall -Wextra -Werror \
  -arch arm64 -arch x86_64 -mmacosx-version-min=10.15 \
  -framework CoreFoundation \
  -o launcher launcher.c
/usr/bin/shasum -a 256 launcher.c launcher
