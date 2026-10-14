# Cutting an Ende release

Ende versions are release dates in ChronVer form, `YYYY.MM.DD` (UTC), with `.N`
appended for additional releases on the same day (e.g. `2026.10.14.1`). The
version lives in `cli/lib/version.txt`; the release tooling sets it and tags the
release `v<version>`. Crate versions in `Cargo.toml` are separate and are not
bumped for releases.

1. Go to this workflow:
   https://github.com/theende/ende/actions/workflows/start_release.yml
1. Select the primary branch, then run the workflow.
1. Wait for the "Create Gist URL" step to complete and look at its output for
   the Gist URL.
1. Follow the instructions in the gist.
