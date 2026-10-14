#!/usr/bin/env -S deno run --check --allow-write=. --allow-read=. --lock=./tools/deno.lock.json
// Copyright 2026 the Ende authors. MIT license.

// Builds, signs and packages a release when a `v*` tag is pushed (the
// `release_tag` workflow pushes it), then attaches everything to a DRAFT
// GitHub release. Publishing the draft is a manual step; see
// doc/releasing.md. This is separate from the upstream release path in ci.ts
// (gated off by its `fullMatrix`).

import {
  createWorkflow,
  defineMatrix,
  expr,
  job,
  step,
} from "jsr:@david/gagen@0.3.1";
import {
  cloneStdSubmoduleStep,
  dlEndeShEnabled,
  dlS3Env,
  getOsSpecificSteps,
  installDenoStep,
  installRustStep,
  sysRootConfig,
} from "./common.ts";

// The tag being released: the pushed tag, or the `tag` input on a manual
// re-run. Steps read it from the RELEASE_TAG env var, never by interpolating
// it into a script.
const releaseTag = "${{ inputs.tag || github.ref_name }}";
const releaseRef =
  "${{ inputs.tag && format('refs/tags/{0}', inputs.tag) || github.ref }}";

const checkoutTagStep = step({
  name: "Clone repository",
  uses: "actions/checkout@v6",
  with: {
    ref: releaseRef,
    "fetch-depth": 1,
    submodules: false,
  },
});

// === verify ===

const versionStep = step({
  id: "version",
  name: "Check the tag matches cli/lib/version.txt",
  run: [
    './tools/release/verify_release_tag.ts "$RELEASE_TAG"',
    'echo "version=${RELEASE_TAG#v}" >> "$GITHUB_OUTPUT"',
  ],
  outputs: ["version"] as const,
});

const verifyJob = job("verify", {
  name: "verify tag",
  runsOn: "ubuntu-24.04",
  timeoutMinutes: 10,
  steps: [checkoutTagStep, installDenoStep, versionStep],
  outputs: {
    version: versionStep.outputs.version,
  },
});

// === build ===

const matrix = defineMatrix({
  include: [
    {
      target: "aarch64-apple-darwin",
      runner: "macos-14",
      os: "macos",
      arch: "aarch64",
    },
    {
      target: "x86_64-unknown-linux-gnu",
      runner: "ubuntu-24.04",
      os: "linux",
      arch: "x86_64",
    },
    {
      target: "aarch64-unknown-linux-gnu",
      runner: "ubuntu-24.04-arm",
      os: "linux",
      arch: "aarch64",
    },
    {
      target: "x86_64-pc-windows-msvc",
      runner: "windows-2022",
      os: "windows",
      arch: "x86_64",
    },
  ],
});
const isLinux = matrix.os.equals("linux");
const isMacos = matrix.os.equals("macos");
const isWindows = matrix.os.equals("windows");

// Signing runs only when its secrets are configured. Step `if:`s can't read
// secrets, so the job exposes whether each one is set (not its value: the
// build runs third-party build scripts, which must not see the secrets).
const hasAppleCertificate = expr("env.HAS_APPLE_CERTIFICATE").equals("true");
const hasAppleNotaryKey = expr("env.HAS_APPLE_NOTARY_KEY").equals("true");
const hasWindowsCertificate = expr("env.HAS_WINDOWS_CERTIFICATE").equals(
  "true",
);

const { installPythonStep, setupPrebuiltMacStep, installLldStep } =
  getOsSpecificSteps({
    isWindows,
    isMacos,
    isAarch64: matrix.arch.equals("aarch64"),
  });

const buildSteps = step.dependsOn(
  installLldStep,
  installDenoStep,
  installPythonStep,
  installRustStep,
)(
  {
    // Same sysroot as ci.ts's release builds: links against an old glibc so
    // the binaries run on older distros (e.g. Ubuntu 22.04, Debian 12).
    if: isLinux,
    ...sysRootConfig,
  },
  {
    name: "Build release",
    env: { ENDE_SNAPSHOT_MINIFY_SOURCES: "1" },
    run: [
      "df -h",
      "cargo build --release --locked -p ende -p endert --bin ende --bin endert --features=ende/panic-trace",
      // The desktop runtime (libendert cdylib) that `ende compile` downloads
      // for desktop apps. Built separately, as in ci.ts: on Windows both
      // packages write endert.pdb and racing linkers corrupt it.
      "cargo build --release --locked -p endert_desktop",
      "df -h",
    ],
  },
  {
    name: "Strip binaries (linux)",
    if: isLinux,
    run: [
      "cd target/release",
      // Debug info only: `ende compile` rewrites endert's ELF and needs its
      // symbol table to keep relocations intact (see ci.ts).
      "strip --strip-debug ende endert libendert.so",
    ],
  },
  {
    name: "Strip binaries (mac)",
    if: isMacos,
    run: [
      "cd target/release",
      "strip -x -S ende endert libendert.dylib",
    ],
  },
  {
    name: "Check ende binary runs on an old glibc (linux)",
    if: isLinux,
    run: 'sudo chroot /sysroot "$(pwd)/target/release/ende" --version',
  },
);

// Only `ende` is signed. `ende compile` rewrites endert (and libsui re-signs
// it ad hoc on macOS), so a signature on it would not survive anyway.
const macSignSteps = step.if(isMacos.and(hasAppleCertificate))(
  {
    name: "Sign ende (mac)",
    env: {
      APPLE_CERTIFICATE_P12: "${{ secrets.APPLE_CERTIFICATE_P12 }}",
      APPLE_CERTIFICATE_PASSWORD: "${{ secrets.APPLE_CERTIFICATE_PASSWORD }}",
    },
    run: [
      'KEYCHAIN="$RUNNER_TEMP/release.keychain-db"',
      'KEYCHAIN_PASSWORD="$(openssl rand -base64 24)"',
      'echo "$APPLE_CERTIFICATE_P12" | base64 --decode > "$RUNNER_TEMP/cert.p12"',
      'security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"',
      'security set-keychain-settings -lut 21600 "$KEYCHAIN"',
      'security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"',
      'security import "$RUNNER_TEMP/cert.p12" -k "$KEYCHAIN" -f pkcs12 -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign',
      'rm "$RUNNER_TEMP/cert.p12"',
      'security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" > /dev/null',
      'security list-keychains -d user -s "$KEYCHAIN" "$HOME/Library/Keychains/login.keychain-db"',
      'IDENTITY="$(security find-identity -v -p codesigning "$KEYCHAIN" | grep -m 1 "Developer ID Application" | awk \'{print $2}\')"',
      'if [ -z "$IDENTITY" ]; then echo "::error::No Developer ID Application identity in APPLE_CERTIFICATE_P12"; exit 1; fi',
      // The hardened runtime needs the JIT entitlements V8 relies on.
      'codesign --force --options runtime --timestamp --entitlements cli/entitlements.plist --keychain "$KEYCHAIN" --sign "$IDENTITY" target/release/ende',
      "codesign --verify --strict --verbose=2 target/release/ende",
    ],
  },
);

const windowsSignStep = step({
  // Signs with a PFX. Newer OV/EV certificates are only issued on hardware
  // tokens/HSMs, so instead use a cloud signing service such as Azure Trusted
  // Signing (what upstream Deno uses: azure/login +
  // Azure/artifact-signing-action, see the ci.ts history) in place of this
  // step.
  name: "Sign ende.exe (windows)",
  if: isWindows.and(hasWindowsCertificate),
  shell: "pwsh",
  env: {
    WINDOWS_CERTIFICATE_PFX: "${{ secrets.WINDOWS_CERTIFICATE_PFX }}",
    WINDOWS_CERTIFICATE_PASSWORD: "${{ secrets.WINDOWS_CERTIFICATE_PASSWORD }}",
  },
  run: [
    '$Pfx = Join-Path $env:RUNNER_TEMP "cert.pfx"',
    "[IO.File]::WriteAllBytes($Pfx, [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE_PFX))",
    '$SignTool = (Get-ChildItem -Path "C:\\Program Files*\\Windows Kits\\*\\bin\\*\\x64\\signtool.exe" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1).FullName',
    "& $SignTool sign /f $Pfx /p $env:WINDOWS_CERTIFICATE_PASSWORD /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 target\\release\\ende.exe",
    '$SignExit = $LASTEXITCODE; Remove-Item $Pfx; if ($SignExit -ne 0) { throw "signtool sign failed" }',
    "& $SignTool verify /pa /v target\\release\\ende.exe",
    'if ($LASTEXITCODE -ne 0) { throw "signtool verify failed" }',
  ],
});

const smokeTestStep = step({
  name: "Smoke test",
  env: { NO_COLOR: 1 },
  run: [
    'out="$(target/release/ende --version | tr -d "\\r")"',
    'echo "$out"',
    'expected="ende $ENDE_VERSION (release, $TARGET)"',
    'read -r first_line <<< "$out"',
    'if [ "$first_line" != "$expected" ]; then echo "::error::ende --version does not start with: $expected"; exit 1; fi',
    'actual="$(target/release/ende eval "console.log(Ende.version.ende)" | tr -d "\\r")"',
    'if [ "$actual" != "$ENDE_VERSION" ]; then echo "::error::Ende.version.ende is $actual, expected $ENDE_VERSION"; exit 1; fi',
  ],
});

// Delta patch from the latest published GitHub release, for `ende upgrade`'s
// delta path (cli/tools/upgrade.rs). Skipped when there is none.
const deltaStep = step({
  name: "Generate delta patch",
  env: {
    GH_TOKEN: "${{ github.token }}",
    GH_REPO: "${{ github.repository }}",
  },
  run: [
    'PREV_TAG="$(gh release view --json tagName --jq .tagName 2> /dev/null || true)"',
    'if [ -z "$PREV_TAG" ] || [ "$PREV_TAG" = "$RELEASE_TAG" ]; then',
    '  echo "No previous published release, skipping delta"',
    "  exit 0",
    "fi",
    'if ! gh release download "$PREV_TAG" --pattern "ende-$TARGET.zip" --dir prev-release; then',
    '  echo "$PREV_TAG has no ende-$TARGET.zip, skipping delta"',
    "  exit 0",
    "fi",
    "cd prev-release",
    'if command -v unzip > /dev/null; then unzip -o "ende-$TARGET.zip"; else 7z x -y "ende-$TARGET.zip"; fi',
    "cd ..",
    "cargo build --release --locked -p bsdiff_helper",
    'EXE=""; if [ "$RUNNER_OS" = Windows ]; then EXE=.exe; fi',
    "mkdir -p dist",
    '"target/release/bsdiff_helper$EXE" "prev-release/ende$EXE" "target/release/ende$EXE" "dist/ende-$TARGET.from-${PREV_TAG#v}.bsdiff"',
  ],
});

const packageStep = step({
  name: "Package",
  run: './tools/release/package_release.sh "$TARGET" target/release dist',
});

// Notarizes the exact zip being shipped. A bare executable can't be stapled,
// so Gatekeeper looks the ticket up online on first run.
const notarizeStep = step({
  name: "Notarize ende (mac)",
  if: isMacos.and(hasAppleCertificate).and(hasAppleNotaryKey),
  env: {
    APPLE_NOTARY_KEY: "${{ secrets.APPLE_NOTARY_KEY }}",
    APPLE_NOTARY_KEY_ID: "${{ secrets.APPLE_NOTARY_KEY_ID }}",
    APPLE_NOTARY_ISSUER_ID: "${{ secrets.APPLE_NOTARY_ISSUER_ID }}",
  },
  run: [
    'KEY="$RUNNER_TEMP/notary_key.p8"',
    'printf "%s" "$APPLE_NOTARY_KEY" > "$KEY"',
    'AUTH=(--key "$KEY" --key-id "$APPLE_NOTARY_KEY_ID" --issuer "$APPLE_NOTARY_ISSUER_ID")',
    'xcrun notarytool submit "dist/ende-$TARGET.zip" "${AUTH[@]}" --wait --timeout 30m --output-format json > notary.json || true',
    "cat notary.json",
    'if [ "$(jq -r .status notary.json)" != Accepted ]; then',
    '  ID="$(jq -r ".id // empty" notary.json)"',
    '  if [ -n "$ID" ]; then xcrun notarytool log "$ID" "${AUTH[@]}"; fi',
    '  rm "$KEY"',
    '  echo "::error::Notarization failed"',
    "  exit 1",
    "fi",
    'rm "$KEY"',
  ],
});

const buildJob = job("build", {
  name: `build ${matrix.target}`,
  needs: [verifyJob],
  runsOn: matrix.runner,
  timeoutMinutes: 240,
  strategy: {
    matrix,
    failFast: false,
  },
  defaults: {
    // GH actions does not fail fast by default on Windows
    run: { shell: "bash" },
  },
  env: {
    CARGO_TERM_COLOR: "always",
    RUST_BACKTRACE: "full",
    TARGET: matrix.target,
    ENDE_VERSION: verifyJob.outputs.version,
    HAS_APPLE_CERTIFICATE: "${{ secrets.APPLE_CERTIFICATE_P12 != '' }}",
    HAS_APPLE_NOTARY_KEY: "${{ secrets.APPLE_NOTARY_KEY != '' }}",
    HAS_WINDOWS_CERTIFICATE: "${{ secrets.WINDOWS_CERTIFICATE_PFX != '' }}",
  },
  steps: [
    step({
      name: "Configure git",
      run: [
        "git config --global core.symlinks true",
        "git config --global fetch.parallel 32",
      ],
    }),
    checkoutTagStep,
    step({
      // cURL's --ipv4 flag is busted on the macOS runners (see ci.ts)
      name: "Remove macOS cURL --ipv4 flag",
      if: isMacos,
      run: "rm ~/.curlrc || true",
    }),
    cloneStdSubmoduleStep,
    setupPrebuiltMacStep,
    buildSteps,
    macSignSteps,
    windowsSignStep,
    smokeTestStep,
    deltaStep,
    packageStep,
    notarizeStep,
    step({
      name: "Upload release assets",
      uses: "actions/upload-artifact@v6",
      with: {
        name: `release-${matrix.target}`,
        path: "dist/*",
        "if-no-files-found": "error",
        "retention-days": 7,
      },
    }),
  ],
});

const downloadAssetsStep = step({
  name: "Download release assets",
  uses: "actions/download-artifact@v6",
  with: {
    pattern: "release-*",
    path: "dist",
    "merge-multiple": true,
  },
});

// === release ===

const releaseJob = job("release", {
  name: "draft GitHub release",
  needs: [buildJob],
  runsOn: "ubuntu-24.04",
  timeoutMinutes: 30,
  permissions: { contents: "write" },
  steps: [
    downloadAssetsStep,
    step({
      name: "Create draft release",
      env: {
        GH_TOKEN: "${{ github.token }}",
        GH_REPO: "${{ github.repository }}",
      },
      run: [
        "ls -l dist",
        "cd dist",
        'IS_DRAFT="$(gh release view "$RELEASE_TAG" --json isDraft --jq .isDraft 2> /dev/null || true)"',
        'if [ -z "$IS_DRAFT" ]; then',
        '  gh release create "$RELEASE_TAG" --draft --verify-tag --title "$RELEASE_TAG" --generate-notes ./*',
        'elif [ "$IS_DRAFT" = true ]; then',
        '  echo "Draft for $RELEASE_TAG exists, replacing its assets"',
        '  gh release upload "$RELEASE_TAG" --clobber ./*',
        "else",
        '  echo "::error::$RELEASE_TAG is already published; refusing to replace its assets"',
        "  exit 1",
        "fi",
      ],
    }),
  ],
});

// === dl.ende.sh ===

// Uploads the runtime archives `ende compile` downloads for this version.
// They can go up before the release is published: nothing points at
// `release/v<version>/` until an `ende` of that version exists.
// `release-latest.txt` (what `ende upgrade` checks) must not change until the
// GitHub release is published, so the post_publish workflow updates it.
const dlJob = job("dl_ende_sh", {
  name: "upload runtimes to dl.ende.sh",
  needs: [buildJob],
  if: dlEndeShEnabled,
  runsOn: "ubuntu-24.04",
  timeoutMinutes: 30,
  steps: [
    downloadAssetsStep,
    step({
      name: "Upload endert and libendert archives",
      env: dlS3Env,
      run: [
        'aws s3 cp dist/ "s3://$DL_S3_BUCKET/release/$RELEASE_TAG/" --recursive --exclude "*" --include "endert-*" --include "libendert-*"',
      ],
    }),
  ],
});

const workflow = createWorkflow({
  name: "release",
  on: {
    push: { tags: ["v*"] },
    // Re-run the release for an existing tag, e.g. after a flaky build.
    workflow_dispatch: {
      inputs: {
        tag: {
          description: "Existing release tag to build, e.g. v2026.10.14",
          type: "string",
          required: true,
        },
      },
    },
  },
  permissions: { contents: "read" },
  concurrency: {
    group: `release-${releaseTag}`,
    cancelInProgress: false,
  },
  env: {
    RELEASE_TAG: releaseTag,
  },
  jobs: [verifyJob, buildJob, releaseJob, dlJob],
});

const header = "# GENERATED BY ./release.ts -- DO NOT DIRECTLY EDIT";

export function generate() {
  return workflow.toYamlString({ header });
}

export const RELEASE_YML_URL = new URL(
  "./release.generated.yml",
  import.meta.url,
);

if (import.meta.main) {
  workflow.writeOrLint({ filePath: RELEASE_YML_URL, header });
}
