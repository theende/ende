# Package management: `ende add` / `ende install`

This is the map for how Ende adds dependencies to a project: how the CLI flags
are parsed, how the configuration file is rewritten, and how packages are cached
and recorded in the lockfile. Read this before touching `ende add`,
`ende install <pkg>`, or the flags that control how a dependency is written
(`--no-save`, `--save-exact`, `--unscoped`).

Ende packages come from JSR (and plain URLs). npm packages are not supported:
`ende add npm:<pkg>` fails with a "not supported in Ende" error.

## The flag parser

Command shape is declared in `libs/cli_parser/src/defs.rs` (`ADD_SUBCOMMAND`,
`INSTALL_SUBCOMMAND`) and converted to flags in `src/convert.rs` (`add_parse`,
and the install branch that produces `InstallFlagsLocal::Add`). `AddFlags`
itself is defined in `libs/cli_parser/src/flags.rs` and re-exported through
`crate::args`. When you add a field, update the struct, the parser, and every
`AddFlags { .. }` literal in `libs/cli_parser/src/tests_full.rs` (the shared
test case is `add_or_install_subcommand`, which loops over both `add` and
`install`).

There is a lint (`ensureNoNonPermissionCapitalLetterShortFlags` in
`tools/lint.js`) that forbids capital-letter short flags unless they are on an
explicit allowlist with a documented precedent.

## Where a dependency gets written: the config writer

`cli/tools/pm/mod.rs` is the core. The `add()` entry point resolves each
requested package to a concrete version
(`find_package_and_select_version_for_req`) and then writes it to the `imports`
of the member `ende.json`. `load_config()` discovers that file and will _create_
one if none exists.

The actual rewrite is `ConfigUpdater::add(selected)`; `ConfigUpdater::remove()`
mirrors it.

## How packages actually get cached

After the config is (optionally) rewritten and committed, `add()` calls
`install_after_modification()`, which builds a fresh `CliFactory` (to pick up
the edited config from disk) and calls `cache_deps::cache_top_level_deps()`.

`cache_top_level_deps()` (in `cli/tools/pm/cache_deps.rs`) is the shared routine
used by `add`, `remove`, `install`, `outdated` and friends. Its model is: derive
the set of graph roots from the project's **import map** (`ende.json` imports)
plus any `additional_roots`, build the module graph, and cache everything it
reaches, updating the lockfile.

## The flags, end to end

- `--no-save`: resolve and cache the package and update the lockfile, but do
  **not** rewrite or commit any config file. Implemented by skipping the
  `ConfigUpdater::add`/`commit` calls and pushing the package to
  `additional_roots` (the graph-build section runs when there is either an
  import map _or_ additional roots).
- `--save-exact`: write the exact version instead of a caret range.
- `--unscoped`: use the package name without its scope as the import alias.

## Tests

- Spec tests live under `tests/specs/add/`. Relevant ones: `no_save/`,
  `exact_version/`, `unscoped/`. Run a subset with `./x test-spec add::`. A spec
  test that only needs the config result (not the download noise) uses
  `"output": "[WILDCARD]"` for the `add` step and asserts the file contents in a
  following `eval` step.
- Parser unit tests: `add_or_install_subcommand` in
  `libs/cli_parser/src/tests_full.rs`
  (`cargo test -p ende_cli_parser --features ende_core/v8 add_or_install`).

## Related work / next steps

- `--no-save` in a directory with no config still creates an empty config file
  (a side effect of `load_config`), which slightly contradicts "no save"; the
  common case (existing project) is unaffected.
