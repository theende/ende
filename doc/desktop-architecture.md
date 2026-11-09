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

1. **Serve address before runtime build** (`RendererTransport::choose`,
   `publish_env`): pick the app's transport (see
   [Serving the page](#serving-the-page-app-or-loopback)) and publish
   `ENDE_SERVE_ADDRESS` (`memory:<host>` or `tcp:127.0.0.1:<port>`) and
   `ENDE_DESKTOP_WS_URL`. They're runtime-only variables
   (`ende_os::set_process_local_env_var`), not `setenv`: child processes don't
   inherit them. The user's `Ende.serve` / `export default { fetch }` later
   binds this pre-set address with no coordination needed. The one real
   `setenv`, `WEBVIEW2_USER_DATA_FOLDER` on Windows, happens just before, while
   the process is still single-threaded (`setenv` is not thread-safe on glibc;
   Rust 1.81+ marks it `unsafe`).
2. **chdir into the extracted VFS** (`lib.rs:1262`): process-wide, so it must
   happen before any task resolves a relative path. Frameworks (Next `.next/`,
   Vite `dist/`) resolve build output relative to cwd.

Then `run_desktop` runs two futures concurrently under `tokio::select!`
(`lib.rs:1848`): `endert::run::run_with_options(…)` (the Ende runtime, with
`auto_serve: true`, `serve_port`, `op_state_init`) and `laufey::run()` (the
native event loop). A third spawned task, `navigate_fut`, bridges them.

## Navigation: poll with a real GET, not a connect

`navigate_fut` (`lib.rs:1773`) waits for the server before pointing the window
at it. It does a full `GET / HTTP/1.1` (over the memory listener for `app://`)
and checks for a `2xx`/`3xx` status line — **not** a bare TCP connect — because
dev servers like Vite accept the socket before they can actually serve. 60
attempts × 250ms, then `Window::navigate(url)` on the initial window id. Under
`--inspect-brk`/`--inspect-wait` it first blocks until a DevTools client
attaches to the mux.

## Two transports

App content and JS↔native calls travel on completely separate paths.

**Content — HTTP to `Ende.serve`.** The WebView is a real browser pointed at the
app's server: at `app://<host>/` through a scheme handler, or at
`http://127.0.0.1:<port>/`. See
[Serving the page](#serving-the-page-app-or-loopback).

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

## Serving the page: `app://` or loopback

The page's origin keys its storage (localStorage, IndexedDB), so it has to be
the same every launch (denoland/deno#35444). `app_scheme_host` picks one of two
transports:

- **`app://<host>`: the webview backend on macOS and Windows.** `<host>` is the
  app identifier, lowercased (`app_origin_host`, baked at build time). The app
  serves on the in-process memory listener `<host>`, and
  `cli/rt_desktop/scheme_bridge.rs` answers laufey's `app` scheme by forwarding
  each request to it over HTTP/1.1. No TCP port serves the page.
- **`http://127.0.0.1:<random port>`: everything else**, so storage still resets
  every launch there. The Linux webview (laufey 0.7.0's WebKitGTK host drops
  request bodies, so every POST would arrive empty), CEF (its profile is a
  temporary directory per process, and Chromium's Local Network Access likely
  blocks the WebSocket relay), and raw (no page).

**The identifier is the storage key.** Set `desktop.app.identifier`. Without it
the identifier is derived from the app name (`ende desktop` warns), and renaming
the app, or changing the identifier, starts it with empty storage. The WebView2
data folder (`%LOCALAPPDATA%\<identifier>\WebView2`) is keyed by it too.

What the app sees on `app://`:

- `request.url` is `http://<host>/…` (the bridge sends `Host: <host>`), and
  `info.remoteAddr.transport` is `"memory"`. A `Location`, `Content-Location` or
  `Refresh` URL on `http://<host>` is moved to `app://<host>` on the way out, so
  `Response.redirect(new URL("/x", req.url))` works.
- **WebSockets:** `new WebSocket("ws://" + location.host)` can't work, because
  webviews never send `ws:` through a scheme handler. A relay on a random
  loopback port forwards upgrades to the memory listener. Its URL,
  `ws://127.0.0.1:<port>/.ende-desktop-relay/<token>`, is in
  `ENDE_DESKTOP_WS_URL`, which only the runtime sees. Hand it to the page, for
  example from an endpoint
  (`Response.json({ ws: Ende.env.get("ENDE_DESKTOP_WS_URL") })`), and the page
  appends its path. The relay admits only the app's `Origin` and this launch's
  token. Relayed requests carry `x-ende-desktop-relay: 1`.
- **Redirects (macOS):** WKWebView doesn't follow a scheme handler's redirect,
  so the bridge handles one to the app origin. A navigation gets a page that
  does `location.replace(<target>)`, so the window ends up at the target (a form
  POST answered `303` lands on a `GET`). Everything else (`fetch`, XHR, images,
  a `307`/`308` of a form POST) is followed in the bridge, with the Fetch method
  rules (`303`, and `301`/`302` of a POST, become a bodiless `GET`; `307`/`308`
  resend the body) and at most 20 hops, then a `502`. `response.url` stays the
  first URL, `response.redirected` is `false`, and `redirect: "manual"` is
  followed too. A redirect to another origin isn't followed: `fetch` gets the
  3xx and a navigation stays where it was. Windows passes every 3xx through
  (untested whether WebView2 follows).
- **Cookies (macOS):** WKWebView keeps no cookies for the scheme, so the bridge
  keeps a cookie jar for the app (`cli/rt_desktop/cookie_jar.rs`). It stores the
  `Set-Cookie` of the app's responses (each redirect hop it follows included)
  and sends the matching `Cookie` on later requests and on WebSocket upgrades
  through the relay. Rules are RFC 6265bis's for one host: `Domain` must be the
  app host (other domains are refused), `Path`, `Expires`/`Max-Age` (400 days at
  most; a past date or `Max-Age=0` deletes), `Secure` is kept (`app://` counts
  as secure), at most 180 cookies of 4096 bytes. An HttpOnly session cookie set
  at login works as in a browser. **`document.cookie` doesn't:** page JavaScript
  can't read or set these cookies.
  - Session cookies (no `Expires`/`Max-Age`) last until the app quits.
    Persistent ones are kept in
    `~/Library/Application Support/<identifier>/cookies.json` (owner-only,
    written a second after a change and when the app exits), so a login survives
    a relaunch. A damaged file is logged and starts an empty jar.
  - Any page shown in an app window can make requests to `app://<host>`, so
    cookies go by who made the request, like `SameSite=Lax`. WKWebView tells the
    handler only through `Origin` and `Referer`: `fetch()` sends `Referer`, a
    form POST sends `Origin`, cross-origin requests send their own origin. XHR,
    `<img>`, `<script>` and stylesheets send neither, from any page, and neither
    does `fetch(url, { referrer: "" })`. A request on the app origin gets every
    cookie. A cross-origin request only gets cookies on a `GET` navigation (a
    link from a remote page), and then not `SameSite=Strict` ones;
    `SameSite=None` gets no exception. A request with neither header (XHR,
    images, scripts, stylesheets, `navigate()`, the first load) gets all but
    `Strict` cookies with `GET` / `HEAD`, and none with `POST` or any other
    method. `Set-Cookie` is only kept from a request on the app origin or a
    navigation.
  - **Trade-off:** the bridge can't tell the app's header-less `GET` from a
    remote page's, so a remote page an app window navigates to can make `app://`
    `GET`s that carry the user's `Lax` cookies, for example include a
    cookie-authenticated JSON endpoint as a `<script>`. That's weaker than a
    browser's `SameSite=Lax`, accepted so XHR and authenticated images and
    scripts work. Keep state changes off `GET`, and don't serve private data in
    a form a `<script>` tag can run (send `X-Content-Type-Options: nosniff` with
    JSON). `Strict` cookies only go with requests the bridge can tie to the app.
  - What doesn't work: an XHR `POST` (also axios's default XHR adapter) goes
    without cookies and an XHR login's `Set-Cookie` is dropped, so use `fetch()`
    or a form for those. Use `Lax` (or no `SameSite`) for the session cookie:
    `Strict` isn't sent with XHR, images, the first load or `navigate()`. The
    bridge can't tell a frame from a window, so a cross-site frame of an app
    page gets its `Lax` cookies. Pages that shouldn't be framed can send
    `Content-Security-Policy: frame-ancestors 'self'` (untested on `app://`).
  - Windows keeps WebView2's own cookie handling (untested whether it keeps
    cookies for the scheme).
- **Request bodies:** at most 4 MiB (`413`). On macOS a `Blob` body arrives
  empty (WebKit forwards `HTTPBody`, not `HTTPBodyStream`); strings and
  `FormData` arrive.
- **No streaming on Windows:** WebView2 gets the whole response before the page
  sees any of it, so server-sent events and other streamed responses don't
  arrive until they end.
- **Cancelling:** laufey 0.7.0 doesn't report a request the webview cancelled.
  The app's handler keeps running until it answers, or until a write fails (then
  `request.signal` aborts).
- **Opening the app's own pages in another window:** `navigate()` takes an
  absolute URL, and there's no `http://127.0.0.1:<port>` to point it at. In the
  page, build it from `location.origin`. In the app, use `app://<host>/…`, where
  `<host>` is the name in `ENDE_SERVE_ADDRESS` (`memory:<host>`).
- If WebView2 couldn't register the scheme for a window (laufey then retries
  without it), the initial window shows an error page instead of staying blank
  (`LostSchemeGuard`).

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
  hidden instead of destroyed, as in `BrowserWindow.close()`, but it counts as
  closed (`isClosed()`), so the `close` event JS dispatches when it catches up
  can't be canceled. A programmatic
  `close()` fires no `close` event, so the documented "`preventDefault()`, ask,
  then `close()`" pattern can't loop.

- **Hidden windows** (`hidden_by_app` in `cli/rt_desktop/lib.rs`): a window the
  app hid (`{ visible: false }`, `hide()`) is recorded so neither the initial
  window's first-load reveal nor the 10 s navigate fallback shows it; `show()`
  clears it. The fallback only shows an initial window whose first load never
  finished. laufey 0.7.0's CEF backend ignores the `hidden` creation flag, so
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
- **Auto-update** (`apply_pending_update`, `UpdateLaunch` in
  `cli/rt_desktop/lib.rs`; unix only, and on Windows `Ende.autoUpdate` logs
  that it isn't supported yet): `op_desktop_apply_patch` stages
  `<dylib>.update`. The first thing `runtime_main` does is check the files
  beside the dylib. A staged update is swapped in, with the old dylib kept as
  `.backup`. The swap can't change the code already loaded, so libendert then
  `execve`s the host executable with the same argv and environment plus
  `ENDE_DESKTOP_UPDATE_REEXEC` (Ende-written, denoland/deno#36948). The new
  image's first run is the update's trial: it writes `.update-trial`, and only
  it may write `.update-ok` (`UpdateTrial`, once). The trial succeeds once the
  main module has evaluated, top-level await included, and the event loop has
  run one more turn (`confirm_update_trial` in `cli/rt/run.rs`; that turn
  dispatches rejections startup code left unhandled), unless an uncaught error
  has started the desktop error exit; or when the app exits with code 0 before
  that (an `OpExitCallbacks` hook). So an update whose startup JS throws or
  rejects is rolled back; a crash later in the trial launch isn't. A launch that
  finds `.update-trial` without `.update-ok` restores `.backup` and re-execs
  again, so the old code runs and fires `onRollback`. A re-exec'd process never
  swaps or re-execs, so there is no loop. Before the exec every descriptor above
  stderr is marked `FD_CLOEXEC`: without that, the new CEF image kept the old
  one's profile files (LevelDB `LOCK`s, caches) and pipes open. If the exec
  fails, the old code keeps running, logs a warning, and doesn't confirm; the
  next launch is the trial. The backend is already up at that point, so a CEF
  window that showed at creation may flash once. A dylib that can't even be
  loaded never reaches this code, so it can't be rolled back.
- **Window-handle interop** (`get_raw_window_handle`, `lib.rs:467`): laufey's
  native handle is converted to `raw-window-handle` types per platform — AppKit
  / Win32 / X11 / **Wayland** (`LAUFEY_WINDOW_HANDLE_WAYLAND`, `lib.rs:518`).
  Wayland is detected at runtime, giving native Wayland rather than XWayland.
- **`--inspect`**: `cli/tools/desktop_devtools.rs` runs a CDP multiplexer in the
  parent fronting both the Ende inspector and the renderer's debug port behind
  one `/unified` websocket.
