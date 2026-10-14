# Architecture overview

Ende is built as a stack of layers. Each layer depends only on the ones below
it, which keeps the system testable and lets the lower layers be reused outside
of the `ende` binary. From the top down:

```
+-----------------------------------------------------------+
|  cli/            the `ende` binary: subcommands, tooling   |
+-----------------------------------------------------------+
|  runtime/        ende_runtime: assembles the JS runtime    |
+-----------------------------------------------------------+
|  ext/*           extensions: native capabilities for JS    |
+-----------------------------------------------------------+
|  libs/*          ende_core + supporting crates (V8 bridge) |
+-----------------------------------------------------------+
|  V8 + Tokio      JavaScript engine and async runtime       |
+-----------------------------------------------------------+
```

## The CLI layer (`cli/`)

The `ende` crate in `cli/` is everything a user touches directly. It owns flag
parsing, the subcommands (`run`, `test`, `fmt`, `lint`, `compile`, `install`,
`publish`, and so on), package-management tooling, the LSP, and the module
loader that ties module resolution to the runtime.

Key entry points:

- `cli/main.rs` — process entry point and command routing.
- `cli/args/flags.rs` — the full `clap` flag and subcommand definition. Adding a
  flag or a subcommand starts here.
- `cli/tools/<tool>/` — one module per subcommand (for example
  `cli/tools/fmt.rs` for a simple command, `cli/tools/test/` for a complex one).
- `cli/module_loader.rs` — resolves and loads modules, bridging the resolver and
  the graph to the runtime.

The CLI is intentionally heavy: it pulls in TypeScript checking, JSR resolution,
and the lockfile. Lower layers must not depend back up into it.

## The runtime layer (`runtime/`)

The `ende_runtime` crate assembles a working JavaScript runtime out of
`ende_core` plus a curated set of extensions. It is the piece embedders use when
they want "Ende the runtime" without "Ende the CLI".

Key files:

- `runtime/worker.rs` — constructs the main worker: an isolate, the op set, and
  the bootstrap sequence.
- `runtime/web_worker.rs` — the Web Worker variant.
- `runtime/permissions/` — the permission model that gates every sensitive op
  (read, write, net, env, run, ffi, sys). Permissions are checked in Rust at the
  op boundary, never in JavaScript.

## The extension layer (`ext/*`)

Each directory under `ext/` is a self-contained extension: a Rust crate that
defines **ops** (native functions callable from JS) plus the JavaScript that
exposes a higher-level API on top of them. This is where the platform actually
lives. Examples:

- Web platform: `ext/web`, `ext/fetch`, `ext/url`, `ext/crypto`, `ext/console`,
  `ext/webidl`, `ext/websocket`, `ext/webgpu`, `ext/canvas`.
- System access: `ext/fs`, `ext/net`, `ext/io`, `ext/os`, `ext/process`,
  `ext/signals`, `ext/tls`.
- Ende-specific: `ext/cron`, `ext/cache`, `ext/ffi`.

The typical shape of an extension is:

1. Rust `#[op2]` functions that do the privileged work and take a permission
   check where needed.
2. A `00_*.js` / `01_*.js` set of JavaScript modules that build the public API
   and call the ops through `Ende.core.ops`.
3. Registration of the extension in `runtime/worker.rs` (and the CLI's snapshot)
   so it is part of the assembled runtime.

When adding native functionality, add the op in the relevant `ext/<name>/`
crate; do not reach into the runtime or CLI for it.

## The core layer (`libs/*`)

`libs/` holds `ende_core` and the crates merged in from the former standalone
`ende_core` repository. This is the bridge between Rust and V8: it owns the op
infrastructure, the module loader trait, the snapshotting machinery, the
JsRuntime event loop, and the `serde_v8` serialization layer.

Notable members:

- `libs/core` — `ende_core` itself: `JsRuntime`, op registration, the module
  map, the inspector integration.
- `libs/ops` — the `#[op2]` proc-macro that generates the Rust/V8 glue.
- `libs/serde_v8` — zero-copy-ish serialization between Rust types and V8
  values.
- `libs/resolver`, `libs/lockfile`, `libs/config` — the resolution and
  package-management building blocks the CLI composes.

These crates are deliberately free of CLI concerns so they can be unit-tested in
isolation and reused by other tools.

## Cross-cutting concepts

- **Ops** are the only way JavaScript reaches native code. An op is a Rust
  function exposed to JS; sync ops return immediately, async ops return a future
  that resolves on the event loop.
- **Extensions** bundle ops with their JavaScript and are the unit of
  composition for a runtime.
- **Workers** are isolated JavaScript execution contexts (the main worker and
  Web Workers); each has its own V8 isolate.
- **Resources** are managed handles (open files, sockets, readers) tracked by
  `ende_core` and passed across the Rust/JS boundary by integer id.
- **Permissions** are enforced in Rust at the op boundary. A capability that is
  not granted causes the op to error before doing any work.

## Where to make a change

| You want to…                         | Start in…                         |
| ------------------------------------ | --------------------------------- |
| Add or change a CLI flag/subcommand  | `cli/args/flags.rs`, `cli/tools/` |
| Add a native capability to JS        | `ext/<name>/` (op + JS)           |
| Change how the runtime is assembled  | `runtime/worker.rs`               |
| Touch the Rust/V8 bridge or op macro | `libs/core`, `libs/ops`           |
| Change module/JSR resolution         | `libs/resolver`, CLI              |

See [`codebase-map.md`](./codebase-map.md) for a finer-grained directory map and
[`testing.md`](./testing.md) for how to test each layer.
