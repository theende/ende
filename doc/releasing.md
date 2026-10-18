# Releasing

This page describes how an Ende release goes from a version bump to something
`ende upgrade` installs, what is automated, and which secrets and variables the
workflows need. The per-release checklist is generated from
`tools/release/release_doc_template.md` (see `tools/cut_a_release.md`).

## The flow

| Step | Workflow                 | Trigger                     | Result                                                                                                                      |
| ---- | ------------------------ | --------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| 1    | `start_release`          | manual                      | a gist with the release checklist                                                                                           |
| 2    | `version_bump`           | manual                      | a PR setting `cli/lib/version.txt` to today's ChronVer; merge it                                                            |
| 3    | `release_tag`            | manual                      | pushes the `v<version>` tag (with `ENDE_RELEASE_PAT`, so the push starts step 4)                                            |
| 4    | `release` (`release.ts`) | the `v*` tag push           | builds every target and creates a **draft** GitHub release with all assets; uploads the runtimes to dl.ende.sh (if enabled) |
| 5    | none                     | manual                      | review the draft's notes and assets, then **publish** it                                                                    |
| 6    | `post_publish`           | the release being published | updates `dl.ende.sh/release-latest.txt` (if enabled)                                                                        |

The tag must point at a commit whose `cli/lib/version.txt` holds the same
version (`v` + the version); the `release` workflow's first job checks this and
stops otherwise.

## What users see when

- **Draft** (after step 4): only maintainers see it. Nothing points at it.
- **Published** (step 5): the assets are public, so upgrading to that exact
  version works, and the next release's delta patch is generated against it.
- **`release-latest.txt` updated** (step 6): `ende upgrade` (and the periodic
  update check) offer the new version. `ende upgrade` reads the version from
  `https://dl.ende.sh/release-latest.txt` and downloads the archive from the
  GitHub release, so this file must only change after publishing. If dl.ende.sh
  uploads are not enabled, upload the file by hand: its content is the tag, e.g.
  `v2026.10.14`, plus a newline.

To pull a bad release back, point `release-latest.txt` at the previous tag.

## The `release` workflow

Generated from `.github/workflows/release.ts` (regenerate after editing it, as
with every workflow; see [`ci.md`](./ci.md)). Jobs:

1. **verify** — runs `tools/release/verify_release_tag.ts`.
2. **build** — one job per target (`aarch64-apple-darwin`,
   `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
   `x86_64-pc-windows-msvc`), with no shared cache. It builds `ende`, `endert`
   and the desktop runtime (`endert_desktop`, i.e. libendert), strips them,
   signs `ende` when the signing secrets exist, smoke tests it
   (`ende --version`, `Ende.version.ende`), generates a delta patch from the
   latest published GitHub release (skipped when there is none), and runs
   `tools/release/package_release.sh`. Linux builds use the same sysroot setup
   as `ci.ts` (`sysRootConfig` in `.github/workflows/common.ts`), so the
   binaries link against an old glibc and run on older distributions; the job
   checks this by running `ende --version` inside the sysroot.
3. **release** — creates the draft release with
   `gh release create --draft
   --generate-notes`. A re-run replaces the assets
   of an existing draft and refuses to touch a published release.
4. **dl_ende_sh** — uploads `endert-*` and `libendert-*` to
   `release/v<version>/` on dl.ende.sh, which `ende compile` and `ende desktop`
   download from.

To rebuild an existing tag (for example after a flaky job), re-run the failed
jobs, or run the workflow by hand with the `tag` input.

Assets per target (`<target>` as above):

- `ende-<target>.zip` — just `ende` (`ende.exe` on Windows)
- `ende-<target>.zip.sha256sum`
- `ende-<target>.sha256sum` — SHA-256 of the extracted binary
- `endert-<target>.zip`, `endert-<target>.zip.sha256sum`
- `libendert-<target>.zip`, `libendert-<target>.zip.sha256sum` — holds
  `libendert.dylib`, `libendert.so` or `endert.dll`
- `ende-<target>.from-<previous version>.bsdiff` and `.bsdiff.sha256sum` — when
  there is a previous published release

Checksum files read `<sha256>  <file name>`. The QuickJS runtimes
(`endert-quickjs-*`, used by `ende compile --engine quickjs`) are not built yet.

## Signing

Each signing step runs only when its secrets are set; otherwise the binaries
ship unsigned. Only `ende` is signed: `ende compile` rewrites `endert`, so a
signature on it would not survive.

| Secret                         | Used for                                                      |
| ------------------------------ | ------------------------------------------------------------- |
| `APPLE_CERTIFICATE_P12`        | base64 of a Developer ID Application certificate + key (.p12) |
| `APPLE_CERTIFICATE_PASSWORD`   | its password                                                  |
| `APPLE_NOTARY_KEY`             | contents of an App Store Connect API key (`AuthKey_*.p8`)     |
| `APPLE_NOTARY_KEY_ID`          | that key's ID                                                 |
| `APPLE_NOTARY_ISSUER_ID`       | the issuer ID shown with App Store Connect API keys           |
| `WINDOWS_CERTIFICATE_PFX`      | base64 of a code signing certificate + key (.pfx)             |
| `WINDOWS_CERTIFICATE_PASSWORD` | its password                                                  |

On macOS `ende` is signed with the hardened runtime and `cli/entitlements.plist`
(V8 needs the JIT entitlements), then the zip is notarized with `notarytool`. A
bare executable can't be stapled, so Gatekeeper checks the notarization online.
On Windows `ende.exe` is signed with `signtool` and timestamped. New Windows
certificates live on hardware tokens or HSMs and can't be exported as a .pfx;
with one of those, replace the step with a cloud signing service such as Azure
Trusted Signing (what upstream Deno uses).

## dl.ende.sh

dl.ende.sh is served from an S3-compatible bucket (Cloudflare R2, AWS S3, ...).
Uploads are off until the repository variable `DL_ENDE_SH_ENABLED` is `true`.
They need these repository secrets:

- `DL_S3_ENDPOINT` — e.g. `https://<account id>.r2.cloudflarestorage.com`
- `DL_S3_BUCKET`
- `DL_S3_ACCESS_KEY_ID`, `DL_S3_SECRET_ACCESS_KEY` — a key that can write to the
  bucket

and optionally the variable `DL_S3_REGION` (default `auto`, which R2 expects).
`post_publish` also writes `versions.json` (all released versions) and, when
`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ZONE_ID` are set, purges the edge cache
for the files it changed.

## Install scripts

`curl -fsSL https://ende.sh/install.sh | sh` and
`irm https://ende.sh/install.ps1 | iex` run `tools/install/install.sh` and
`tools/install/install.ps1` from `primary`; ende.sh redirects both paths to
their raw GitHub URLs. They read the version from `release-latest.txt` (or take
one as an argument, or `$v` in PowerShell), download the archive from the GitHub
release, check its `.sha256sum`, and install to `~/.ende/bin`
(`$ENDE_INSTALL_ROOT/bin`). They need no change per release; the `install_test`
workflow runs them on every OS when they change.

## Checking the packaging locally

`tools/release/package_release.sh <target> <bin-dir> <out-dir>` produces the
assets above from a directory of built binaries, so it can be run against a
local `cargo build --release` to inspect the archives.
