# Testing

Ende has several test suites, each aimed at a different layer. This page is a
map of what each one is for and how to run it.

## The suites at a glance

| Suite      | Location       | Tests…                                 |
| ---------- | -------------- | -------------------------------------- |
| Spec tests | `tests/specs/` | CLI commands end-to-end (the main one) |
| Unit tests | `tests/unit/`  | Runtime/web APIs from JS/TS            |
| WPT        | `tests/wpt/`   | Web Platform Tests (web standards)     |
| Rust tests | throughout     | Crate-level Rust logic                 |

## Spec tests — the main integration tests

Spec tests live in `tests/specs/`. Each test directory contains a
`__test__.jsonc` file describing one or more steps; a step is an `ende`
invocation whose output is captured and matched against an expectation. The test
name is the directory name. The expectation language supports wildcards
(`[WILDCARD]`, `[WILDLINE]`), unordered blocks, and inline comments; the schema
is `tests/specs/schema.json`.

Reach for a spec test whenever a change is observable from the command line: a
new flag, a subcommand behavior, an error message, a resolution outcome.

To create one:

1. Make a directory under `tests/specs/` with a descriptive name.
2. Add `__test__.jsonc` with the step(s).
3. Add any input files the test needs.
4. Put expected output inline or in a `.out` file.

## Unit tests (`tests/unit/`)

JavaScript/TypeScript tests for runtime and web APIs, named `*_test.ts`. Use
these for behavior that is best asserted from inside the runtime (a Web API's
semantics, an `Ende.*` method). They run under the cargo test harness, not under
a bare `ende test`, because they rely on harness setup.

## Web Platform Tests (`tests/wpt/`)

The upstream Web Platform Tests, used to check standards conformance for web
APIs. Run on Linux in CI.

## Rust tests

Standard `cargo test`, plus `cargo nextest` for the `ende_core` / `libs/*`
crates in CI. Lower-layer crates under `libs/` are designed to be testable in
isolation; some need a sibling crate co-selected to compile.

## Running the suites

The `./x` helper wraps the common commands:

- `./x test-spec <name>` — spec tests.
- `./x test-unit <name>` — unit tests.

Run `./x --help` to see everything. Before pushing, run `tools/format.js` and
the appropriate `tools/lint.js` invocation (`--js` when only JS/TS changed).

## What CI runs

On every push to `primary` and every pull request, CI builds Ende on Linux and
runs lint plus the spec, unit and integration suites (specs and integration
split across two shards). macOS runs weekly and on manual dispatch. Changes that
only touch Markdown or `doc/` skip CI entirely, except Markdown that tests or
code read. See [`ci.md`](./ci.md) for the details, including the full matrix
behind the `fullMatrix` switch.
