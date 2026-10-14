- [ ] Fork this gist and follow the instructions there.

See `doc/releasing.md` in the repository for what each workflow does.

## Pre-flight

**During this process the `primary` branch should be frozen and no commits
should land until the release is finished.**

- [ ] Check that CI on `primary` is green:
      https://github.com/theende/ende/actions/workflows/ci.generated.yml

## Phase 1: Bumping the version

- [ ] Go to the "version_bump" workflow in the repo's actions:
      https://github.com/theende/ende/actions/workflows/version_bump.generated.yml
  1. Click on the "Run workflow" button.
  1. In the drop down, select the `primary` branch.
  1. Run the workflow. It sets the version to the UTC date it runs on
     (`$VERSION` today), with `.N` appended if that date already has a release.

- [ ] Wait for the workflow to complete and for a pull request to be
      automatically opened. Review the pull request, make any necessary changes,
      and merge it.
  - ⛔ **DO NOT** create a release tag manually. That will automatically happen.

  <details>
     <summary>Failure Steps</summary>

  1. Checkout the branch the release is being made on.
  2. Manually run `./tools/release/01_bump_crate_versions.ts`
     1. Ensure `cli/lib/version.txt` holds `$VERSION`
     2. Ensure `Releases.md` was updated correctly
  3. Open a PR with the changes and continue with the steps below.
  </details>

## Phase 2: Tagging

- [ ] Go to the "release_tag" workflow in the repo's actions:
      https://github.com/theende/ende/actions/workflows/release_tag.generated.yml
  1. Run it on the same branch that you used before and wait for it to complete.
     It creates and pushes the `v$VERSION` tag.

  <details>
     <summary>Failure Steps</summary>

  1. The workflow is restartable. Try restarting it.
  2. If that doesn't work, check out the release commit and manually create and
     push the `v$VERSION` tag. Push it with your own credentials: a tag pushed
     with the workflow's default token does not start other workflows.
  </details>

## Phase 3: Building

- [ ] The tag starts the "release" workflow:
      https://github.com/theende/ende/actions/workflows/release.generated.yml
      Wait for it to complete. It builds every target and creates a **draft**
      release at https://github.com/theende/ende/releases.

  <details>
     <summary>Failure Steps</summary>

  1. "verify tag" failed: the tag doesn't match `cli/lib/version.txt` at the
     tagged commit. Delete the tag and redo Phase 2 from the right commit.
  2. A build failed: re-run the failed jobs. To start over, run the workflow by
     hand with the `tag` input set to `v$VERSION`. A re-run replaces the assets
     of the draft.
  </details>

- ⛔ Verify that:
  - [ ] The v$VERSION draft has 7 assets for each of the 4 targets
        (`aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`,
        `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`):
    - `ende-<target>.zip`
    - `ende-<target>.zip.sha256sum`
    - `ende-<target>.sha256sum`
    - `endert-<target>.zip`
    - `endert-<target>.zip.sha256sum`
    - `libendert-<target>.zip`
    - `libendert-<target>.zip.sha256sum`
  - [ ] If v$PAST_VERSION is published, each target also has a delta patch,
        `ende-<target>.from-$PAST_VERSION.bsdiff`, plus its checksum file.
  - [ ] Downloading the archive for your platform and running `ende --version`
        prints `ende $VERSION`.
  - [ ] If signing is set up: on macOS, `codesign -dv ende` shows the Developer
        ID authority; on Windows, the file properties of `ende.exe` show a valid
        digital signature.

## Phase 4: Publishing

- [ ] Edit the draft's release notes if needed, then publish it.
- [ ] Publishing starts the "post_publish" workflow, which points
      https://dl.ende.sh/release-latest.txt at v$VERSION. Until then
      `ende upgrade` keeps offering v$PAST_VERSION. Check that
      `curl -s https://dl.ende.sh/release-latest.txt` prints `v$VERSION`.
  - If dl.ende.sh uploads are not enabled (the `DL_ENDE_SH_ENABLED` variable),
    upload `release-latest.txt` (content `v$VERSION` and a newline) and the
    `endert-*`/`libendert-*` archives (to `release/v$VERSION/`) by hand.

## Add `ende upgrade` banner

- [ ] You can optionally add a banner that will be printed when users run
      `ende upgrade`. This is useful in situation when you want to inform users
      about a need to run a command to enjoy a new feature or a breaking change.
  - Create `banner.txt` file with the content you want to print - _it must be
    plaintext_.
  - Upload it to `release/v$VERSION/banner.txt` in the dl.ende.sh bucket.

## All done!

- [ ] Unfreeze `primary`.

## Downgrading

In case something went wrong:

1. Point https://dl.ende.sh/release-latest.txt back at `v$PAST_VERSION`.
