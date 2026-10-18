#!/bin/sh
# Copyright 2026 the Ende authors. MIT license.
#
# Installs Ende on macOS and Linux. Served at https://ende.sh/install.sh.
#
#   curl -fsSL https://ende.sh/install.sh | sh
#   curl -fsSL https://ende.sh/install.sh | sh -s v2026.10.17
#
# The binary goes in $ENDE_INSTALL_ROOT/bin (default ~/.ende/bin), the same
# directory `ende install -g` puts its shims in.

set -eu

die() {
	echo "error: $*" >&2
	exit 1
}

command -v curl >/dev/null || die "curl is required to install Ende."
command -v unzip >/dev/null || die "unzip is required to install Ende."

case "$(uname -sm)" in
"Darwin arm64") target=aarch64-apple-darwin ;;
"Darwin x86_64")
	# A shell running under Rosetta reports x86_64 on Apple silicon.
	if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
		target=aarch64-apple-darwin
	else
		die "Ende has no release for Intel Macs."
	fi
	;;
"Linux x86_64") target=x86_64-unknown-linux-gnu ;;
"Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-gnu ;;
MINGW* | MSYS* | CYGWIN*)
	die "use PowerShell on Windows: irm https://ende.sh/install.ps1 | iex"
	;;
*) die "Ende has no release for $(uname -sm)." ;;
esac

version="${1:-}"
if [ -z "$version" ]; then
	version="$(curl -fsSL https://dl.ende.sh/release-latest.txt | tr -d '[:space:]')"
fi
case "$version" in
v*) ;;
*) version="v$version" ;;
esac

root="${ENDE_INSTALL_ROOT:-$HOME/.ende}"
bin_dir="$root/bin"
zip="ende-$target.zip"
url="https://github.com/theende/ende/releases/download/$version/$zip"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading Ende $version for $target..."
curl -fL --progress-bar -o "$tmp/$zip" "$url" ||
	die "could not download $url (does release $version exist?)"
curl -fsSL -o "$tmp/$zip.sha256sum" "$url.sha256sum"

expected="$(cut -d ' ' -f 1 <"$tmp/$zip.sha256sum")"
if command -v sha256sum >/dev/null; then
	actual="$(sha256sum "$tmp/$zip" | cut -d ' ' -f 1)"
else
	actual="$(shasum -a 256 "$tmp/$zip" | cut -d ' ' -f 1)"
fi
[ "$actual" = "$expected" ] ||
	die "checksum mismatch for $zip (expected $expected, got $actual)"

unzip -q -o -d "$tmp" "$tmp/$zip"
mkdir -p "$bin_dir"
mv -f "$tmp/ende" "$bin_dir/ende"
chmod +x "$bin_dir/ende"

echo "Ende was installed to $bin_dir/ende"
"$bin_dir/ende" --version

case ":$PATH:" in
*":$bin_dir:"*) echo "Run 'ende --help' to get started." ;;
*)
	echo
	echo "Add Ende to your PATH by adding these lines to your shell profile"
	echo "(for example ~/.zshrc or ~/.bashrc):"
	echo
	echo "  export ENDE_INSTALL_ROOT=\"$root\""
	echo "  export PATH=\"\$ENDE_INSTALL_ROOT/bin:\$PATH\""
	;;
esac
