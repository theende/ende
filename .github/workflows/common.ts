// Copyright 2018-2026 the Deno authors. MIT license.
// Steps shared by the generated workflows (ci.ts, release.ts). Not a workflow
// generator itself.
import {
  type Condition,
  type ConfigValue,
  expr,
  step,
} from "jsr:@david/gagen@0.3.1";

// Note that you may need to add more version to the `apt-get remove` line below if you change this
const llvmVersion = 22;
const installPkgsCommand =
  `sudo apt-get install -y --no-install-recommends clang-${llvmVersion} lld-${llvmVersion} clang-tools-${llvmVersion} clang-format-${llvmVersion} clang-tidy-${llvmVersion}`;
export const sysRootConfig = {
  name: "Set up incremental LTO and sysroot build",
  // This normally takes under a minute, but `apt-get update` has been seen to
  // hang indefinitely when a mirror is unreachable, burning the job's whole
  // timeout before a single test runs. Fail fast instead.
  timeoutMinutes: 10,
  run: `# Setting up sysroot
export DEBIAN_FRONTEND=noninteractive
# Avoid running man-db triggers, which sometimes takes several minutes
# to complete.
sudo apt-get -qq remove --purge -y man-db > /dev/null 2> /dev/null
# Remove older clang before we install
sudo apt-get -qq remove \
  'clang-12*' 'clang-13*' 'clang-14*' 'clang-15*' 'clang-16*' 'clang-17*' 'clang-18*' 'clang-19*' 'clang-20*' 'clang-21*' 'llvm-12*' 'llvm-13*' 'llvm-14*' 'llvm-15*' 'llvm-16*' 'llvm-17*' 'llvm-18*' 'llvm-19*' 'llvm-20*' 'llvm-21*' 'lld-12*' 'lld-13*' 'lld-14*' 'lld-15*' 'lld-16*' 'lld-17*' 'lld-18*' 'lld-19*' 'lld-20*' 'lld-21*' > /dev/null 2> /dev/null

# Install clang-XXX, lld-XXX, and debootstrap.
echo "deb http://apt.llvm.org/noble/ llvm-toolchain-noble-${llvmVersion} main" |
  sudo dd of=/etc/apt/sources.list.d/llvm-toolchain-noble-${llvmVersion}.list
curl https://apt.llvm.org/llvm-snapshot.gpg.key |
  gpg --dearmor                                 |
sudo dd of=/etc/apt/trusted.gpg.d/llvm-snapshot.gpg
sudo apt-get update
# this was unreliable sometimes, so try again if it fails
${installPkgsCommand} || (echo 'Failed. Trying again.' && sudo apt-get clean && sudo apt-get update && ${installPkgsCommand})
# Fix alternatives
(yes '' | sudo update-alternatives --force --all) > /dev/null 2> /dev/null || true

clang-${llvmVersion} -c -o /tmp/memfd_create_shim.o tools/memfd_create_shim.c -fPIC
clang-${llvmVersion} -c -o /tmp/glibc_math_shim.o tools/glibc_math_shim.c -fPIC

echo "Decompressing sysroot..."
wget -q https://github.com/denoland/deno_sysroot_build/releases/download/sysroot-20250207/sysroot-\`uname -m\`.tar.xz -O /tmp/sysroot.tar.xz
cd /
xzcat /tmp/sysroot.tar.xz | sudo tar -x
sudo mount --rbind /dev /sysroot/dev
sudo mount --rbind /sys /sysroot/sys
sudo mount --rbind /home /sysroot/home
sudo mount -t proc /proc /sysroot/proc
cd

echo "Done."

# Configure the build environment. Both Rust and Clang will produce
# llvm bitcode only, so we can use lld's incremental LTO support.

# Load the sysroot's env vars
echo "sysroot env:"
cat /sysroot/.env
. /sysroot/.env

# Important notes:
#   1. -ldl seems to be required to avoid a failure in FFI tests. This flag seems
#      to be in the Rust default flags in the smoketest, so uncertain why we need
#      to be explicit here.
#   2. RUSTFLAGS and RUSTDOCFLAGS must be specified, otherwise the doctests fail
#      to build because the object formats are not compatible.
echo "
CARGO_PROFILE_BENCH_INCREMENTAL=false
CARGO_PROFILE_RELEASE_INCREMENTAL=false
RUSTFLAGS<<__1
  -C linker-plugin-lto=true
  -C linker=clang-${llvmVersion}
  -C link-arg=-fuse-ld=lld-${llvmVersion}
  -C link-arg=-Wl,--icf=safe
  -C link-arg=-ldl
  -C link-arg=-Wl,--allow-shlib-undefined
  -C link-arg=-Wl,--thinlto-cache-dir=$(pwd)/target/release/lto-cache
  -C link-arg=-Wl,--thinlto-cache-policy,cache_size_bytes=700m
  -C link-arg=/tmp/memfd_create_shim.o
  -C link-arg=/tmp/glibc_math_shim.o
  -C link-arg=-Wl,--wrap=expf
  -C link-arg=-Wl,--wrap=powf
  -C link-arg=-Wl,--wrap=exp2f
  -C link-arg=-Wl,--wrap=log2f
  -C link-arg=-Wl,--wrap=logf
  --cfg tokio_unstable
  $RUSTFLAGS
__1
RUSTDOCFLAGS<<__1
  -C linker-plugin-lto=true
  -C linker=clang-${llvmVersion}
  -C link-arg=-fuse-ld=lld-${llvmVersion}
  -C link-arg=-Wl,--icf=safe
  -C link-arg=-ldl
  -C link-arg=-Wl,--allow-shlib-undefined
  -C link-arg=-Wl,--thinlto-cache-dir=$(pwd)/target/release/lto-cache
  -C link-arg=-Wl,--thinlto-cache-policy,cache_size_bytes=700m
  -C link-arg=/tmp/memfd_create_shim.o
  -C link-arg=/tmp/glibc_math_shim.o
  -C link-arg=-Wl,--wrap=expf
  -C link-arg=-Wl,--wrap=powf
  -C link-arg=-Wl,--wrap=exp2f
  -C link-arg=-Wl,--wrap=log2f
  -C link-arg=-Wl,--wrap=logf
  --cfg tokio_unstable
  $RUSTFLAGS
__1
CC=/usr/bin/clang-${llvmVersion}
CFLAGS=$CFLAGS
" > $GITHUB_ENV`,
};

export const cloneSubmodule = (path: string) =>
  step({
    name: `Clone submodule ${path}`,
    run: `git submodule update --init --recursive --depth=1 -- ${path}`,
  });
export const cloneStdSubmoduleStep = cloneSubmodule("./tests/util/std");
export const installDenoStep = step({
  name: "Install Deno",
  uses: "denoland/setup-deno@v2",
  with: { "deno-version": "v2.x" },
});

// Pin rustup-init to 1.28.2: sh.rustup.rs currently serves 1.29.0, which has
// a broken proxy multi-call dispatch (cargo/rustc identify as rustup-init).
// Pre-installing rustup makes `dsherret/rust-toolchain-file@v1`'s internal
// `command -v rustup` check short-circuit and skip the broken curl install.
export const installRustStep = step(
  step({
    name: "Pre-install rustup 1.28.2 (workaround broken 1.29.0)",
    shell: "bash",
    run: [
      "if command -v rustup >/dev/null 2>&1; then",
      "  if ! rustup --version 2>&1 | grep -q '1\\.29\\.0'; then exit 0; fi",
      '  echo "Detected broken rustup 1.29.0, replacing with 1.28.2"',
      "fi",
      'case "${RUNNER_OS}-${RUNNER_ARCH}" in',
      "  Linux-X64)     target=x86_64-unknown-linux-gnu; ext= ;;",
      "  Linux-ARM64)   target=aarch64-unknown-linux-gnu; ext= ;;",
      "  macOS-X64)     target=x86_64-apple-darwin; ext= ;;",
      "  macOS-ARM64)   target=aarch64-apple-darwin; ext= ;;",
      "  Windows-X64)   target=x86_64-pc-windows-msvc; ext=.exe ;;",
      "  Windows-ARM64) target=aarch64-pc-windows-msvc; ext=.exe ;;",
      '  *) echo "Unsupported: ${RUNNER_OS}-${RUNNER_ARCH}"; exit 1 ;;',
      "esac",
      "curl --proto '=https' --tlsv1.2 --retry 10 --retry-connrefused -fsSL \\",
      '  "https://static.rust-lang.org/rustup/archive/1.28.2/${target}/rustup-init${ext}" \\',
      '  -o "rustup-init${ext}"',
      'chmod +x "rustup-init${ext}"',
      '"./rustup-init${ext}" -y --default-toolchain none --no-modify-path',
      'rm "rustup-init${ext}"',
      'echo "${CARGO_HOME:-$HOME/.cargo}/bin" >> "$GITHUB_PATH"',
    ].join("\n"),
  }),
  step({
    uses: "dsherret/rust-toolchain-file@v1",
  }),
);

export function getOsSpecificSteps({
  isWindows,
  isMacos,
  isAarch64,
}: {
  isWindows: Condition;
  isMacos: Condition;
  isAarch64: Condition;
}) {
  const installPythonStep = step({
    name: "Install Python",
    uses: "actions/setup-python@v6",
    with: {
      "python-version": 3.11,
    },
  }, {
    name: "Remove unused versions of Python",
    if: isWindows,
    shell: "pwsh",
    run: [
      '$env:PATH -split ";" |',
      '  Where-Object { Test-Path "$_\\python.exe" } |',
      "  Select-Object -Skip 1 |",
      '  ForEach-Object { Move-Item "$_" "$_.disabled" }',
    ],
  });
  const setupPrebuiltMacStep = step({
    if: isMacos,
    env: {
      GITHUB_TOKEN: "${{ secrets.GITHUB_TOKEN }}",
    },
    run: "echo $GITHUB_WORKSPACE/third_party/prebuilt/mac >> $GITHUB_PATH",
  });
  const installLldStep = step
    .dependsOn(
      cloneStdSubmoduleStep,
      installDenoStep,
      setupPrebuiltMacStep,
    )({
      name: "Install macOS aarch64 lld",
      if: isMacos.and(isAarch64),
      env: {
        GITHUB_TOKEN: "${{ secrets.GITHUB_TOKEN }}",
      },
      run: "./tools/install_prebuilt.js ld64.lld",
    });
  return {
    installPythonStep,
    setupPrebuiltMacStep,
    installLldStep,
  };
}

// dl.ende.sh: the bucket behind it serves `release-latest.txt` (read by
// `ende upgrade`) and `release/v<version>/{endert,libendert}-<target>.zip`
// (downloaded by `ende compile`). Uploads are off until the repository
// variable DL_ENDE_SH_ENABLED is set to `true`.
export const dlEndeShEnabled = expr("vars.DL_ENDE_SH_ENABLED").equals("true");

// Credentials for the S3-compatible bucket (Cloudflare R2, AWS S3, ...), as
// repository secrets:
//   DL_S3_ENDPOINT           e.g. https://<account id>.r2.cloudflarestorage.com
//   DL_S3_BUCKET             bucket name
//   DL_S3_ACCESS_KEY_ID      key with write access to the bucket
//   DL_S3_SECRET_ACCESS_KEY
// and optionally the repository variable DL_S3_REGION (R2 uses `auto`).
export const dlS3Env: Readonly<Record<string, ConfigValue>> = {
  AWS_ACCESS_KEY_ID: "${{ secrets.DL_S3_ACCESS_KEY_ID }}",
  AWS_SECRET_ACCESS_KEY: "${{ secrets.DL_S3_SECRET_ACCESS_KEY }}",
  AWS_ENDPOINT_URL_S3: "${{ secrets.DL_S3_ENDPOINT }}",
  AWS_DEFAULT_REGION: "${{ vars.DL_S3_REGION || 'auto' }}",
  // aws-cli >= 2.23 adds CRC checksums to every upload by default, which some
  // S3-compatible stores reject. Only send them when the API requires it.
  AWS_REQUEST_CHECKSUM_CALCULATION: "when_required",
  AWS_RESPONSE_CHECKSUM_VALIDATION: "when_required",
  DL_S3_BUCKET: "${{ secrets.DL_S3_BUCKET }}",
};
