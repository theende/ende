# Ende Desktop — internals

How `ende desktop` actually wires a native window to an Ende runtime. This skips
the obvious (what a WebView is, what `Ende.serve` does) and documents the
non-obvious mechanics: the load model, the ABI handshake, the two transports,
and the lifecycle edges.

## The inversion: Ende is the library, not the host

The usual embedding has the app `dlopen` a UI lib. Desktop is inverted. The
**executable is the native backend** (`laufey` for CEF, `laufey_webview` for the
system WebView); the **Ende runtime is a cdylib** named `libendert`
(`crate-type = ["cdylib"]`, `cli/rt_desktop/`). The backend is launched with
`--runtime <path-to-dylib>`, `dlopen`s it, and calls into it through a C ABI.

Every launcher the packager writes is just that one invocation:

```
exec "$DIR/laufey_webview" --runtime "$DIR/libendert.so" "$@"   # desktop.rs:1407
```

So the binary that owns `main()`, the event loop, and the window is _not_ Ende —
Ende is a guest the backend boots and drives.

## The ABI handshake

The dylib exports the Laufey C ABI — `laufey_runtime_init` / `_start` /
`_shutdown`. Ende defines them itself rather than with `laufey::main!`, so that
`laufey_runtime_init` can keep the backend's API table for calls the `laufey`
crate doesn't wrap (`post_ui_task`, used by `run_on_ui_thread`).
`laufey_runtime_start` runs `runtime_main`, the runtime entrypoint.

Version safety is two-layered:

- **Compile time** (`lib.rs:47`):
  `const _: () = assert!(laufey::LAUFEY_API_VERSION == 26, …)`. If the linked
  `laufey` crate's ABI version drifts from what the shipped backend speaks,
  `cargo build` fails loudly instead of producing a dylib that silently won't
  launch.
- **Build-pin** (`desktop.rs:29`, `LAUFEY_VERSION` injected by `cli/build.rs`):
  the backend binary version is pinned, and downloads are integrity-checked
  against in-repo SHA-256 digests (`cli/laufey_sums.lock`, `LAUFEY_PINNED_SUMS`)
  — no TOFU on the GitHub releases page. `LaufeyBackendResolver` resolves in
  order: `LAUFEY_DEV_DIR` checkout → cached download → fresh download.

The dylib finds _itself_ on disk via `dladdr` on one of its own functions
(`get_dylib_path`, `lib.rs:986`) — needed for the auto-update sentinel and for
locating the embedded payload.

## The embedded payload

User code + assets are not files next to the dylib; they're a **section inside
the dylib** named `d3n0l4nd`, read back with
`libsui::find_section_in_current_image` (`find_section_in_dylib`,
`lib.rs:1512`). `extract_standalone_with_finder` parses it into the standalone
metadata + VFS, exactly like `ende compile`, but sourced from the loaded image
rather than argv0.

## Boot ordering (and why it's strict)

Inside `runtime_main` everything up to the tokio runtime is deliberately
single-threaded, because two process-global operations happen there that are
unsafe once threads exist:

1. **Port publish before runtime build** (`lib.rs:1222`): allocate a random
   loopback port, then `set_var("ENDE_SERVE_ADDRESS", "tcp:127.0.0.1:<port>")`.
   `setenv` is not thread-safe on glibc (Rust 1.81+ marks it `unsafe`), so it
   must precede the mio IO thread and inspector thread. The user's `Ende.serve`
   / `export default { fetch }` later binds this pre-set address with no
   coordination needed.
2. **chdir into the extracted VFS** (`lib.rs:1262`): process-wide, so it must
   happen before any task resolves a relative path. Frameworks (Next `.next/`,
   Vite `dist/`) resolve build output relative to cwd.

Then `run_desktop` runs two futures concurrently under `tokio::select!`
(`lib.rs:1848`): `endert::run::run_with_options(…)` (the Ende runtime, with
`auto_serve: true`, `serve_port`, `op_state_init`) and `laufey::run()` (the
native event loop). A third spawned task, `navigate_fut`, bridges them.

## Navigation: poll with a real GET, not a connect

`navigate_fut` (`lib.rs:1773`) waits for the server before pointing the window
at it. It does a full `GET / HTTP/1.1` and checks for a `2xx`/`3xx` status line
— **not** a bare TCP connect — because dev servers like Vite accept the socket
before they can actually serve. 60 attempts × 250ms, then
`Window::navigate(url)` on the initial window id. Under
`--inspect-brk`/`--inspect-wait` it first blocks until a DevTools client
attaches to the mux.

## Two transports

App content and JS↔native calls travel on completely separate paths.

**Content — loopback HTTP.** The WebView is a real browser pointed at
`http://127.0.0.1:<port>/`. Nothing special.

**Bindings — mpsc events + per-call oneshot.** `Ende.desktop`/window APIs and
webview→Ende function calls do _not_ use HTTP:

- Native → runtime: a single mpsc channel of `DesktopEvent`, sent through
  `DesktopEventSender::send`, which never blocks the UI thread. Continuous
  events (mouse move, wheel, window move and resize) are capped at 1024 waiting
  in the queue and **dropped past that** rather than OOMing the runtime.
  Discrete events (close requests, page loads, focus, keys, clicks, menu clicks,
  bind calls) don't count toward the cap and are never dropped.
- One JS consumer: an async loop in `DESKTOP_JS` awaiting
  `op_desktop_recv_event()` (`desktop.rs:702`), dispatching each event to the
  matching DOM-style `EventTarget`. The op promise is `unrefOpPromise`'d so the
  pump never keeps the event loop alive by itself.

**A bind call round-trip** (the interesting one):

1. JS registers a handler: `window.bind(name, fn)` stores `fn` in a per-window
   `Map` and calls `laufey add_binding_async(name)` (`desktop.rs:229`,
   `lib.rs:361`).
2. Webview calls `window.bindings.<name>(...)`. The native async binding
   serializes args (`laufey::Value` → JSON), allocates a `call_id` from a global
   `AtomicU32` and registers a `oneshot` sender under it (`register_bind_call`,
   `ops/desktop.rs:297`), then sends
   `DesktopEvent::BindCall { window_id, name, args, call_id }`.
3. The JS pump's `"bindCall"` arm looks up the handler, `await`s it, and calls
   `op_desktop_resolve_bind_call(call_id, result)` /
   `op_desktop_reject_bind_call(call_id, err)` (`desktop.rs:736`).
4. The op pops the `oneshot` for that `call_id` and fires it
   (`ops/desktop.rs:1051`); back in the binding future, `resp_rx.await` resolves
   and calls `js_call.resolve(...)` / `.reject(...)` (`lib.rs:391`).

The `call_id` keying is what lets many concurrent binding calls from the same
window multiplex over the one event channel and route their replies back
correctly.

The JS namespace the webview sees is `bindings` — `set_js_namespace("bindings")`
at `lib.rs:1212`, so calls are `window.bindings.foo()`.

## HMR: three outcomes, decided by V8

`cli/rt/hmr.rs` watches the source dir (`notify`, debounced) and classifies each
change into `ChangeOutcome` (`hmr.rs:331`), which the desktop side maps to a
`ReloadKind` callback (`lib.rs:1642`):

- **Replaced** — a script edit that V8 accepts via `Debugger.setScriptSource`
  after `deno_ast` transpile. Hot-patched in place, no reload.
- **SoftReload → ReloadKind::Soft** — a static asset changed, or a module was
  removed. Runs `location.reload()` on **every** window in `open_windows`
  (`lib.rs:1648`), which re-fetches from the still-running server.
- **Restart → ReloadKind::Restart** — V8 rejected the edit as a top-level
  ES-module change (imports / exported bindings / top-level `let` changed). The
  module graph can't be patched and a reload wouldn't help (the stale runtime
  keeps serving), so `restart_desktop_app()` calls `exit(75)` (`lib.rs:1537`).
  The `ende desktop --hmr` supervisor owns the child, catches code 75, and
  relaunches — exiting via sentinel rather than re-exec keeps the process group,
  Ctrl-C handling, and temp-entrypoint cleanup with the supervisor
  (`HMR_RESTART_EXIT_CODE`, `lib.rs:1529`).

For framework dev servers (`is_framework_dev`), Ende-level HMR is disabled
entirely — the framework's own websocket HMR runs and cwd is left at the source
dir so its watcher sees real files.

## Lifecycle edges worth knowing

- **Closing a window** (`on_close_requested`, `CloseRequests` in
  `cli/rt_desktop/lib.rs`): laufey 0.7.0 holds a window open once a
  close-requested handler is registered, until `Window::close()`. The handler
  sends `CloseRequested`, and DESKTOP_JS dispatches a cancelable `close` event,
  then closes the window unless a listener called `preventDefault()` (a window
  with no `BrowserWindow` closes right away). **Divergence from upstream**:
  upstream's documented pattern waits for that answer forever, so a hung JS
  thread made the window impossible to close. Ende's watchdog thread closes it
  if JS hasn't answered within 5 s (`CLOSE_ANSWER_TIMEOUT`). The clock stops
  while `alert`/`confirm`/`prompt` block the JS thread. A WebGPU window is
  hidden instead of destroyed, as in `BrowserWindow.close()`. A programmatic
  `close()` fires no `close` event, so the documented "`preventDefault()`, ask,
  then `close()`" pattern can't loop.

- **Hidden windows** (`hidden_by_app` in `cli/rt_desktop/lib.rs`): a window the
  app hid (`{ visible: false }`, `hide()`) is recorded so neither the initial
  window's first-load reveal nor the 10 s navigate fallback shows it; `show()`
  clears it. laufey 0.7.0's CEF backend ignores the `hidden` creation flag, so
  libendert also hides such windows explicitly (CEF can flash them).
- **Initial window** (`create_initial_window`): its size and creation-time style
  come from ende.json `desktop.initialWindow`, baked into the metadata
  (`InitialWindowConfig`); an invalid value fails `ende desktop`. The first
  `BrowserWindow` adopts it, so its `frameless`/`transparent`/... options are
  ignored, and `width`/`height` resize it only when given.
  `showOnFirstLoad:
  false` puts it in `hidden_by_app`, so nothing reveals it
  until `show()`.
- **`printToPdf()`**: the backend only renders. A `path` is never handed to
  laufey, which would write it unchecked on the UI thread. The op checks write
  permission the way `Ende.writeFile` does, before rendering, then writes the
  bytes through the runtime's `FileSystem`. The raw backend has no page and
  rejects with `NotSupported`.
- **Fork re-entry guard** (`lib.rs:1145`): framework dev servers fork worker
  processes that re-exec _this same dylib_. A worker is detected by the
  combination of argv shaped like `<exe> run … script.js` **and** a parent
  worker env var (`NODE_CHANNEL_FD` / `NEXT_PRIVATE_WORKER`) — the env var alone
  was a false-positive trap when a user shell already had it set. Detected
  workers run headless (no window).
- **Window-handle interop** (`get_raw_window_handle`, `lib.rs:467`): laufey's
  native handle is converted to `raw-window-handle` types per platform — AppKit
  / Win32 / X11 / **Wayland** (`LAUFEY_WINDOW_HANDLE_WAYLAND`, `lib.rs:518`).
  Wayland is detected at runtime, giving native Wayland rather than XWayland.
- **`--inspect`**: `cli/tools/desktop_devtools.rs` runs a CDP multiplexer in the
  parent fronting both the Ende inspector and the renderer's debug port behind
  one `/unified` websocket.
