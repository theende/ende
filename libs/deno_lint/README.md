# deno_lint (vendored)

This is [deno_lint](https://github.com/denoland/deno_lint) 0.84.1 from
crates.io, patched into the workspace with `[patch.crates-io]` in the root
`Cargo.toml`.

It's vendored so `ende lint` can accept `ende-lint-ignore` and
`ende-lint-ignore-file` as well as `deno-lint-ignore` and
`deno-lint-ignore-file`, which upstream can't do:

- `LinterContext::new` (`src/linter.rs`) built the per-line directive from
  `custom_ignore_file_directive` instead of `custom_ignore_diagnostic_directive`,
  so a custom per-line directive was ignored.
- A linter accepted a single spelling of each directive.

## Changes from upstream

- `src/linter.rs`: the custom directives are accepted in addition to the
  defaults (`LinterContext` holds a list of each), and the per-line one comes
  from `custom_ignore_diagnostic_directive`.
- `src/ignore_directives.rs`, `src/context.rs`: directive parsing takes a list
  of accepted directives.
- `src/rules/ban_untagged_ignore.rs`: the hint suggests `// ende-lint-ignore`.
- `src/lib.rs`: tests for the custom directives.
- `Cargo.toml`: the `dlint` example and the dev-dependencies it alone used are
  dropped (`examples/` isn't vendored), and `publish = false` is set.

Everything else, including upstream's copyright headers, LICENSE and
`.rustfmt.toml`, is unchanged. The crate isn't a workspace member, so it keeps
upstream's code style and lints; `.dprint.json` and
`tools/copyright_checker.js` skip it. Its tests run with
`cargo test -p deno_lint --lib`.
