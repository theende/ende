# tsc

This directory contains the typescript compiler and a small compiler host for
the runtime snapshot.

## How to upgrade TypeScript.

The files in this directory are mostly from the TypeScript repository. We
currently (unfortunately) have a rather manual process for upgrading TypeScript.
It works like this currently:

1. Checkout denoland/TypeScript repo in a separate directory.
1. Add Microsoft/TypeScript as a remote and fetch its latest tags
1. Checkout a new branch based on this tag.
1. Cherry pick the custom commit we made in a previous release to the new one.
1. This commit has a "deno.ts" file in it. Read the instructions in it.
1. Copy typescript.js into this repo.
1. Copy d.ts files into dts directory.
1. Review the copied files, removing and reverting what's necessary

So that might look something like this:

```
git clone https://github.com/denoland/TypeScript.git
cd typescript
git remote add upstream https://github.com/Microsoft/TypeScript
git fetch upstream
git checkout v3.9.7
git checkout -b branch_v3.9.7
git cherry pick <previous-release-branch-commit-we-did>
npm install
npx hereby
rsync built/local/typescript.js ~/src/ende/cli/tsc/00_typescript.js
rsync --exclude=protocol.d.ts --exclude=tsserverlibrary.d.ts --exclude=typescriptServices.d.ts built/local/*.d.ts ~/src/ende/cli/tsc/dts/
```

## TypeScript 7 (`--unstable-tsc7`)

`ende check --unstable-tsc7` uses the native TypeScript 7 compiler, `tsc`,
pinned in `cli/tsc7_sums.lock`. It isn't in this directory: build `ende` with
`ENDE_TSC7_DIR` set to the directory for the build's target in the output of
`tools/tsc7_mirror.ts` (`tsc/<version>/<target>/`), and `cli/build.rs` checks
the files against the lock file and embeds them. `tsc7.rs` extracts them to
`<ENDE_DIR>/tsc/<version>/<hash>/` on first use. Without `ENDE_TSC7_DIR` nothing
is embedded, and `--unstable-tsc7` needs `ENDE_TSC_BIN`, the path of a
TypeScript 7 `tsc`.
