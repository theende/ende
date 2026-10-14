# Contributor documentation

This directory holds long-form, mostly-stable context about how the Ende
codebase is structured. It is meant to be read by contributors before they start
working in the repository: the architectural "why" and the map of where things
live, the kind of background that saves a lot of exploration time up front.

## Contents

- [`architecture.md`](./architecture.md) — the layered design of the runtime:
  the `ende` CLI crate, the `ende_runtime` crate, the `ext/*` extensions, and
  the `ende_core` / `libs/*` foundation. Read this first.
- [`codebase-map.md`](./codebase-map.md) — a directory-by-directory map plus the
  handful of files worth understanding before anything else.
- [`testing.md`](./testing.md) — the test taxonomy (spec, unit, WPT) and which
  command runs each one.
- [`ci.md`](./ci.md) — how the CI workflow is generated and how it decides which
  jobs to run, how it caches, and how docs-only changes skip CI.
- [`releasing.md`](./releasing.md) — the release flow from version bump to
  `ende upgrade`, the `release` workflow and its assets, signing secrets, and
  dl.ende.sh uploads.
- [`desktop-architecture.md`](./desktop-architecture.md) — how `ende desktop`
  wires a native window to an Ende runtime: the load model, the ABI handshake,
  the two transports, and the lifecycle edges.
- [`package-management.md`](./package-management.md) — how `ende add` /
  `ende install <pkg>` add dependencies: the two flag parsers, the config
  writer, the shared install path, and the `--dev` / `--save-optional` /
  `--no-save` flags (including why `optionalDependencies` need special
  handling).

## Editing these docs

These files are plain Markdown and are formatted by `./tools/format.js` like the
rest of the repository. Run `tools/format.js` before committing. Keep prose
hard-wrapped at 80 columns to match the existing style.

A pull request that touches **only** `doc/` runs the `lint` job and nothing
else. The build, test, bench and `ende_core` jobs are skipped, because editing
Markdown cannot break the binary. See [`ci.md`](./ci.md) for how that is wired
up. If your PR changes anything outside `doc/`, the full pipeline runs as usual.
