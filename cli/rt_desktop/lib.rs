// Copyright 2018-2026 the Deno authors. MIT license.

//! Desktop runtime for Ende (libendert).
//!
//! This is a cdylib that exports the Laufey C ABI (laufey_runtime_init,
//! laufey_runtime_start, laufey_runtime_shutdown) and boots the full Ende
//! standalone runtime. A Laufey backend (CEF, WebView) loads this
//! shared library and provides the browser/window layer.
//!
//! The user's code uses `Ende.serve()` or `export default { fetch }`
//! to serve an HTTP app. On the macOS and Windows webview backends the
//! desktop runtime serves it over an in-process memory listener and loads
//! the webview from the app's stable origin, `app://<host>`, whose requests
//! [`scheme_bridge`] forwards to that listener. Everywhere else
//! ([`app_scheme_host`]) it serves the app on a random loopback port and
//! navigates the webview there.

mod cookie_jar;
mod scheme_bridge;

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
// Only used by the unix-only `apply_pending_update`; gated to avoid an unused
// import on other platforms.
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use deno_terminal::colors;
use ende_core::anyhow::Context;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::v8;
use ende_lib::util::net::allocate_random_port;
use ende_lib::util::result::js_error_downcast_ref;
use ende_lib::version::otel_runtime_config;
use ende_runtime::fmt_errors::format_js_error;
use endert::desktop::DesktopApi;
use endert::run::RunOptions;

/// Compile-time check: the laufey crate we're linking against must use the
/// same C ABI version as the prebuilt backend we ship. If laufey bumps
/// `LAUFEY_API_VERSION` without our side updating, `init_api` would reject the
/// backend at startup with `-2`. Catching the mismatch at `cargo build` time
/// makes the failure mode obvious instead of "the desktop app silently won't
/// launch".
const _: () = assert!(
  laufey::LAUFEY_API_VERSION == 34,
  "LAUFEY_API_VERSION mismatch: update this assert and the prebuilt backend release pin in cli/tools/desktop.rs when laufey bumps its API version",
);

/// How long JS has to answer a native close request (the window's close
/// button, Cmd+W, Alt+F4) before the window closes anyway.
///
/// JS answers by dispatching the cancelable `close` event: the window closes
/// unless a listener calls `preventDefault()`. Upstream's documented pattern
/// waits for that answer forever, so a hung or busy JS thread would leave a
/// window the user can't close. Ende closes it after this long instead. Time
/// spent in `alert`/`confirm`/`prompt` doesn't count: they block the JS
/// thread on purpose.
const CLOSE_ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

/// Native close requests that JS hasn't answered yet, and the watchdog
/// thread that closes their windows when the answer is too late.
struct CloseRequests {
  state: Mutex<CloseRequestsState>,
  wake: Condvar,
  timeout: Duration,
  on_timeout: Box<dyn Fn(u32) + Send + Sync>,
}

struct CloseRequestsState {
  /// Window id → the answer time it has left, as of `settled_at`.
  pending: HashMap<u32, Duration>,
  /// Modal dialogs open right now. Budgets don't run while one is.
  dialogs: usize,
  settled_at: Instant,
}

impl CloseRequestsState {
  /// Charge the time since the last call to every pending budget, unless a
  /// dialog was open. Call before changing `pending` or `dialogs`.
  fn settle(&mut self, now: Instant) {
    if self.dialogs == 0 {
      let elapsed = now.saturating_duration_since(self.settled_at);
      for left in self.pending.values_mut() {
        *left = left.saturating_sub(elapsed);
      }
    }
    self.settled_at = now;
  }
}

impl CloseRequests {
  /// `on_timeout` runs on the watchdog thread for each window whose request
  /// wasn't answered in time.
  fn new(
    timeout: Duration,
    on_timeout: impl Fn(u32) + Send + Sync + 'static,
  ) -> Arc<Self> {
    let requests = Arc::new(Self {
      state: Mutex::new(CloseRequestsState {
        pending: HashMap::new(),
        dialogs: 0,
        settled_at: Instant::now(),
      }),
      wake: Condvar::new(),
      timeout,
      on_timeout: Box::new(on_timeout),
    });
    let watched = requests.clone();
    std::thread::Builder::new()
      .name("ende-desktop-close-watchdog".to_string())
      .spawn(move || watched.watch())
      .expect("failed to spawn the window close watchdog thread");
    requests
  }

  /// Start the answer timeout for a close request. Returns `false` if one is
  /// already pending for this window: JS gets one `close` event per request
  /// it answers, not one per click.
  fn begin(&self, window_id: u32) -> bool {
    let mut state = self.state.lock().unwrap();
    state.settle(Instant::now());
    if state.pending.contains_key(&window_id) {
      return false;
    }
    state.pending.insert(window_id, self.timeout);
    self.wake.notify_one();
    true
  }

  /// JS answered the request for this window, or the window closed: stop its
  /// timeout. A no-op if none is pending.
  fn answer(&self, window_id: u32) {
    let mut state = self.state.lock().unwrap();
    state.settle(Instant::now());
    state.pending.remove(&window_id);
  }

  /// Pause every timeout until the returned guard drops. Hold it while a
  /// modal dialog blocks the JS thread.
  fn dialog(&self) -> CloseRequestsDialog<'_> {
    let mut state = self.state.lock().unwrap();
    state.settle(Instant::now());
    state.dialogs += 1;
    CloseRequestsDialog(self)
  }

  fn watch(&self) {
    let mut state = self.state.lock().unwrap();
    loop {
      state.settle(Instant::now());
      let mut wait = None;
      let mut expired = Vec::new();
      if state.dialogs == 0 {
        for (&window_id, &left) in &state.pending {
          if left.is_zero() {
            expired.push(window_id);
          } else {
            wait = Some(wait.map_or(left, |w: Duration| w.min(left)));
          }
        }
      }
      if !expired.is_empty() {
        for window_id in &expired {
          state.pending.remove(window_id);
        }
        drop(state);
        for window_id in expired {
          log::warn!(
            "[desktop] window {window_id}: JS didn't answer the close request within {:?}; closing it anyway",
            self.timeout
          );
          (self.on_timeout)(window_id);
        }
        state = self.state.lock().unwrap();
        continue;
      }
      state = match wait {
        Some(wait) => self.wake.wait_timeout(state, wait).unwrap().0,
        None => self.wake.wait(state).unwrap(),
      };
    }
  }
}

/// Keeps close timeouts paused; see [`CloseRequests::dialog`].
struct CloseRequestsDialog<'a>(&'a CloseRequests);

impl Drop for CloseRequestsDialog<'_> {
  fn drop(&mut self) {
    let mut state = self.0.state.lock().unwrap();
    state.settle(Instant::now());
    state.dialogs -= 1;
    self.0.wake.notify_one();
  }
}

/// Stop tracking a notification and close it, with the map unlocked: the
/// backend can deliver the notification's `Closed` event before `close`
/// returns, and that handler (see `show_notification`) locks the same map.
///
/// Then report its `close` event with `closed`: laufey 0.7.0's
/// `NotificationHandle::close` drops the notification's event handler right
/// after asking the backend to close it, so an asynchronous `Closed` is lost,
/// and the Notifications spec fires `close` for `close()`. Only whoever
/// untracks a notification reports its close, so it's reported once.
fn close_tracked_notification<H>(
  notifications: &Mutex<HashMap<u32, H>>,
  notification_id: u32,
  close: impl FnOnce(&H),
  closed: impl FnOnce(),
) {
  let handle = notifications.lock().unwrap().remove(&notification_id);
  if let Some(handle) = handle {
    close(&handle);
    closed();
  }
}

/// Stop tracking a notification. Returns whether it was tracked.
fn untrack_notification<H>(
  notifications: &Mutex<HashMap<u32, H>>,
  notification_id: u32,
) -> bool {
  notifications
    .lock()
    .unwrap()
    .remove(&notification_id)
    .is_some()
}

/// Record a window as closed.
fn record_closed_window(
  closed_windows: &Mutex<HashSet<u32>>,
  open_windows: &Mutex<HashSet<u32>>,
  window_id: u32,
) {
  closed_windows.lock().unwrap().insert(window_id);
  open_windows.lock().unwrap().remove(&window_id);
}

/// Close a window natively and record it as closed.
fn close_native_window(
  closed_windows: &Mutex<HashSet<u32>>,
  open_windows: &Mutex<HashSet<u32>>,
  window_id: u32,
) {
  record_closed_window(closed_windows, open_windows, window_id);
  laufey::Window::from_id(window_id).close();
}

/// How the close timeout closes a window JS didn't answer for.
#[derive(Debug, PartialEq, Eq)]
enum TimedOutClose {
  Destroy,
  /// A WebGPU window: destroying it would dangle the surface's handles; see
  /// `BrowserWindow.close()` in ende_runtime.
  Hide,
}

/// Decides how the close timeout closes `window_id`, and records it as
/// closed. A hidden WebGPU window counts as closed too: its `isClosed()` is
/// true, so the `close` event JS dispatches when it catches up can't be
/// canceled, and a late listener can't keep the window around hidden.
fn time_out_close(
  closed_windows: &Mutex<HashSet<u32>>,
  open_windows: &Mutex<HashSet<u32>>,
  surface_windows: &Mutex<HashSet<u32>>,
  hidden_by_app: &Mutex<HashSet<u32>>,
  window_id: u32,
) -> TimedOutClose {
  record_closed_window(closed_windows, open_windows, window_id);
  if surface_windows.lock().unwrap().contains(&window_id) {
    hidden_by_app.lock().unwrap().insert(window_id);
    TimedOutClose::Hide
  } else {
    TimedOutClose::Destroy
  }
}

/// Laufey-backed implementation of [`endert::desktop::DesktopApi`].
struct WefDesktopApi {
  event_tx: ende_runtime::ops::desktop::DesktopEventSender,
  pending_responses: ende_runtime::ops::desktop::PendingBindResponses,
  closed_windows: Arc<Mutex<HashSet<u32>>>,
  /// Windows whose WebGPU surface holds their native handles. A close that
  /// times out hides them instead of destroying them under the surface.
  surface_windows: Arc<Mutex<HashSet<u32>>>,
  close_requests: Arc<CloseRequests>,
  /// IDs of every window currently displayed. Shared with the HMR reload
  /// callback so it can refresh all windows, not just the initial one.
  open_windows: Arc<Mutex<HashSet<u32>>>,
  /// Windows the app has explicitly hidden (via `hide` or `{ visible: false }`)
  /// and not re-shown. The initial window's deferred first-paint reveal
  /// consults this so it never overrides an app-requested hide. Shared with the
  /// navigate fallback for the same reason.
  hidden_by_app: Arc<Mutex<HashSet<u32>>>,
  trays: Arc<Mutex<HashMap<u32, laufey::TrayIcon>>>,
  notifications: Arc<Mutex<HashMap<u32, laufey::NotificationHandle>>>,
  /// See [`notifications_supported`].
  notifications_supported: bool,
  /// The ende.json `desktop.backend` baked into the app.
  backend: Option<String>,
  /// Singleton for the unified-mux DevTools window. Without this, every
  /// `openDevtools()` call would spawn another DevTools window.
  devtools_window: Mutex<Option<u32>>,
  /// Set on Windows when the app is served from `app://`.
  lost_scheme_guard: Option<Arc<scheme_bridge::LostSchemeGuard>>,
}

impl WefDesktopApi {
  /// Set up all event handlers on a newly created window, wiring events
  /// into the shared event channel.
  /// `loaded` is set once the window's first page load has finished.
  fn setup_window_events(
    &self,
    window: laufey::Window,
    show_on_first_load: bool,
    loaded: Arc<std::sync::atomic::AtomicBool>,
  ) -> laufey::Window {
    let kb_tx = self.event_tx.clone();
    let mouse_click_tx = self.event_tx.clone();
    let mouse_move_tx = self.event_tx.clone();
    let wheel_tx = self.event_tx.clone();
    let cursor_tx = self.event_tx.clone();
    let focus_tx = self.event_tx.clone();
    let resize_tx = self.event_tx.clone();
    let move_tx = self.event_tx.clone();
    let page_load_tx = self.event_tx.clone();
    let close_tx = self.event_tx.clone();
    let closed_windows = self.closed_windows.clone();
    let open_windows_on_close = self.open_windows.clone();
    let close_requests = self.close_requests.clone();
    let hidden_by_app = self.hidden_by_app.clone();
    let lost_scheme_guard = self.lost_scheme_guard.clone();

    window
      .on_keyboard_event(move |ev| {
        let _ =
          kb_tx.send(ende_runtime::ops::desktop::DesktopEvent::KeyboardEvent {
            window_id: ev.window_id,
            r#type: match ev.state {
              laufey::KeyState::Pressed => "keydown".to_string(),
              laufey::KeyState::Released => "keyup".to_string(),
            },
            key: ev.key,
            code: ev.code,
            shift: ev.modifiers.shift,
            control: ev.modifiers.control,
            alt: ev.modifiers.alt,
            meta: ev.modifiers.meta,
            repeat: ev.repeat,
          });
      })
      .on_mouse_click(move |ev| {
        let _ = mouse_click_tx.send(
          ende_runtime::ops::desktop::DesktopEvent::MouseClick {
            window_id: ev.window_id,
            state: match ev.state {
              laufey::MouseButtonState::Pressed => "pressed".to_string(),
              laufey::MouseButtonState::Released => "released".to_string(),
            },
            button: match ev.button {
              laufey::MouseButton::Left => 0,
              laufey::MouseButton::Middle => 1,
              laufey::MouseButton::Right => 2,
              laufey::MouseButton::Back => 3,
              laufey::MouseButton::Forward => 4,
              laufey::MouseButton::Other(n) => n,
            },
            client_x: ev.x,
            client_y: ev.y,
            shift: ev.modifiers.shift,
            control: ev.modifiers.control,
            alt: ev.modifiers.alt,
            meta: ev.modifiers.meta,
            click_count: ev.click_count,
          },
        );
      })
      .on_mouse_move(move |ev| {
        let _ = mouse_move_tx.send(
          ende_runtime::ops::desktop::DesktopEvent::MouseMove {
            window_id: ev.window_id,
            client_x: ev.x,
            client_y: ev.y,
            shift: ev.modifiers.shift,
            control: ev.modifiers.control,
            alt: ev.modifiers.alt,
            meta: ev.modifiers.meta,
          },
        );
      })
      .on_wheel(move |ev| {
        let _ =
          wheel_tx.send(ende_runtime::ops::desktop::DesktopEvent::Wheel {
            window_id: ev.window_id,
            delta_x: ev.delta_x,
            delta_y: ev.delta_y,
            delta_mode: match ev.delta_mode {
              laufey::WheelDeltaMode::Pixel => 0,
              laufey::WheelDeltaMode::Line => 1,
              laufey::WheelDeltaMode::Page => 2,
            },
            client_x: ev.x,
            client_y: ev.y,
            shift: ev.modifiers.shift,
            control: ev.modifiers.control,
            alt: ev.modifiers.alt,
            meta: ev.modifiers.meta,
          });
      })
      .on_cursor_enter_leave(move |ev| {
        let _ = cursor_tx.send(
          ende_runtime::ops::desktop::DesktopEvent::CursorEnterLeave {
            window_id: ev.window_id,
            entered: ev.entered,
            client_x: ev.x,
            client_y: ev.y,
            shift: ev.modifiers.shift,
            control: ev.modifiers.control,
            alt: ev.modifiers.alt,
            meta: ev.modifiers.meta,
          },
        );
      })
      .on_focused(move |ev| {
        let _ = focus_tx.send(
          ende_runtime::ops::desktop::DesktopEvent::FocusChanged {
            window_id: ev.window_id,
            focused: ev.focused,
          },
        );
      })
      .on_resize(move |ev| {
        let _ = resize_tx.send(
          ende_runtime::ops::desktop::DesktopEvent::WindowResize {
            window_id: ev.window_id,
            width: ev.width,
            height: ev.height,
          },
        );
      })
      .on_move(move |ev| {
        let _ =
          move_tx.send(ende_runtime::ops::desktop::DesktopEvent::WindowMove {
            window_id: ev.window_id,
            x: ev.x,
            y: ev.y,
          });
      })
      .on_page_load(move |ev| {
        let first_load = !loaded.swap(true, Ordering::AcqRel);
        if show_on_first_load
          && first_load
          && !hidden_by_app.lock().unwrap().contains(&ev.window_id)
        {
          laufey::Window::from_id(ev.window_id).show();
        }
        if let Some(guard) = &lost_scheme_guard
          && guard.lost_scheme_on_load(ev.window_id)
        {
          log::error!(
            "[desktop] the webview couldn't register the {}:// scheme, so the app's page can't load; another program may use the same WebView2 user data folder (WEBVIEW2_USER_DATA_FOLDER) with other settings",
            scheme_bridge::APP_SCHEME
          );
          laufey::Window::from_id(ev.window_id)
            .navigate(&scheme_bridge::lost_scheme_page());
        }
        let _ = page_load_tx.send(
          ende_runtime::ops::desktop::DesktopEvent::PageLoad {
            window_id: ev.window_id,
          },
        );
      })
      .on_close_requested(move |ev| {
        // Since laufey 0.7.0 a registered close-requested handler *defers*
        // the close: the window stays open until `Window::close()` is called.
        // JS decides: DESKTOP_JS dispatches a cancelable "close" event and
        // closes the window unless a listener calls `preventDefault()`. If
        // JS doesn't answer in time, `close_requests` closes it anyway.
        let window_id = ev.window_id;
        if closed_windows.lock().unwrap().contains(&window_id)
          || !close_requests.begin(window_id)
        {
          return;
        }
        let sent = close_tx.send(
          ende_runtime::ops::desktop::DesktopEvent::CloseRequested {
            window_id,
          },
        );
        if !sent {
          // The runtime is gone; nothing will answer.
          close_requests.answer(window_id);
          close_native_window(
            &closed_windows,
            &open_windows_on_close,
            window_id,
          );
        }
      })
  }

  /// Create the bootstrap window for the app. Unlike windows constructed from
  /// JS via `BrowserWindow`, this one is created *hidden* and revealed only once
  /// its first navigation has finished loading (wired here via `on_page_load`).
  ///
  /// The runtime navigates this window to the app's URL (`app://<host>/` or
  /// `http://127.0.0.1:PORT`, see [`RendererTransport`]) only after the
  /// app's server is listening, so creating it visible
  /// up front would leave the user staring at an empty webview — which paints
  /// solid black on Wayland, where the compositor presents the pre-load frame
  /// verbatim. Deferring the reveal to load-finished means the window's first
  /// visible frame already has content. See
  /// https://github.com/denoland/deno/issues/35530.
  ///
  /// Its size and creation-time style come from ende.json
  /// `desktop.initialWindow`. With `showOnFirstLoad: false` it isn't revealed
  /// at all: the app calls `show()`.
  fn create_initial_window(
    &self,
    options: ende_lib::standalone::binary::InitialWindowConfig,
    loaded: Arc<std::sync::atomic::AtomicBool>,
  ) -> u32 {
    let window = laufey::Window::new_with_options(
      i32::from(options.width),
      i32::from(options.height),
      laufey::WindowOptions {
        frameless: options.frameless,
        no_activate: options.no_activate,
        transparent_titlebar: options.transparent_titlebar,
        hidden: true,
        transparent: options.transparent,
      },
    );
    let window =
      self.setup_window_events(window, options.show_on_first_load, loaded);
    let id = window.id();
    if let Some(guard) = &self.lost_scheme_guard {
      guard.watch(id);
    }
    if !options.show_on_first_load {
      // `desktop.initialWindow.showOnFirstLoad: false`: the app shows it. Keep
      // the navigate fallback from showing it too, and hide it explicitly
      // because laufey 0.7.0's CEF backend ignores `hidden`.
      self.hidden_by_app.lock().unwrap().insert(id);
      window.hide();
    }

    self.open_windows.lock().unwrap().insert(id);
    id
  }
}

impl endert::desktop::DesktopApi for WefDesktopApi {
  fn create_window(
    &self,
    width: i32,
    height: i32,
    frameless: bool,
    no_activate: bool,
    transparent_titlebar: bool,
    transparent: bool,
    hidden: bool,
  ) -> u32 {
    let window = laufey::Window::new_with_options(
      width,
      height,
      laufey::WindowOptions {
        frameless,
        no_activate,
        transparent_titlebar,
        hidden,
        transparent,
      },
    );
    let window = self.setup_window_events(window, false, Default::default());
    let id = window.id();
    self.open_windows.lock().unwrap().insert(id);
    // A window created with `{ visible: false }` must stay hidden: record it so
    // the deferred first-paint reveal in `setup_window_events` does not show it.
    if hidden {
      self.hidden_by_app.lock().unwrap().insert(id);
      // laufey 0.7.0's CEF backend ignores the `hidden` option and shows the
      // window anyway; an explicit hide still works there.
      window.hide();
    }
    id
  }

  fn close_window(&self, window_id: u32) {
    self.close_requests.answer(window_id);
    close_native_window(&self.closed_windows, &self.open_windows, window_id);
  }

  fn answer_close_request(&self, window_id: u32) {
    self.close_requests.answer(window_id);
  }

  fn set_surface_attached(&self, window_id: u32) {
    self.surface_windows.lock().unwrap().insert(window_id);
  }

  fn is_closed(&self, window_id: u32) -> bool {
    self.closed_windows.lock().unwrap().contains(&window_id)
  }

  fn set_title(&self, window_id: u32, title: &str) {
    laufey::Window::from_id(window_id).set_title(title);
  }

  fn get_window_size(&self, window_id: u32) -> (i32, i32) {
    laufey::Window::from_id(window_id).get_size()
  }

  fn set_window_size(&self, window_id: u32, width: i32, height: i32) {
    laufey::Window::from_id(window_id).set_size(width, height);
  }

  fn get_window_position(&self, window_id: u32) -> (i32, i32) {
    laufey::Window::from_id(window_id).get_position()
  }

  fn set_window_position(&self, window_id: u32, x: i32, y: i32) {
    laufey::Window::from_id(window_id).set_position(x, y);
  }

  fn is_resizable(&self, window_id: u32) -> bool {
    laufey::Window::from_id(window_id).get_resizable()
  }

  fn set_resizable(&self, window_id: u32, resizable: bool) {
    laufey::Window::from_id(window_id).set_resizable(resizable);
  }

  fn is_always_on_top(&self, window_id: u32) -> bool {
    laufey::Window::from_id(window_id).get_always_on_top()
  }

  fn set_always_on_top(&self, window_id: u32, always_on_top: bool) {
    laufey::Window::from_id(window_id).set_always_on_top(always_on_top);
  }

  fn get_window_opacity(&self, window_id: u32) -> f64 {
    laufey::Window::from_id(window_id).get_opacity()
  }

  fn set_window_opacity(&self, window_id: u32, opacity: f64) {
    laufey::Window::from_id(window_id).set_opacity(opacity);
  }

  fn is_visible(&self, window_id: u32) -> bool {
    laufey::Window::from_id(window_id).get_visible()
  }

  fn show(&self, window_id: u32) {
    self.hidden_by_app.lock().unwrap().remove(&window_id);
    laufey::Window::from_id(window_id).show();
  }

  fn hide(&self, window_id: u32) {
    self.hidden_by_app.lock().unwrap().insert(window_id);
    laufey::Window::from_id(window_id).hide();
  }

  fn focus(&self, window_id: u32) {
    laufey::Window::from_id(window_id).focus();
  }

  fn open_devtools(&self, window_id: u32, renderer: bool, ende: bool) {
    if let Ok(mux) = env::var("ENDE_DESKTOP_MUX_WS") {
      // Reuse an existing DevTools window when one is already open, so
      // repeated `openDevtools()` calls don't pile up windows.
      if let Some(id) = *self.devtools_window.lock().unwrap()
        && !self.closed_windows.lock().unwrap().contains(&id)
      {
        laufey::Window::from_id(id).focus();
        return;
      }

      let (endpoint, frontend) = match (renderer, ende) {
        (true, true) => ("/unified", "inspector.html"),
        (true, false) => ("/cef", "inspector.html"),
        (false, true) => ("/ende", "js_app.html"),
        (false, false) => unreachable!(),
      };
      let url = format!("http://{mux}/devtools/{frontend}?ws={mux}{endpoint}");
      log::info!(
        "[desktop] openDevtools(renderer={renderer}, ende={ende}) → {url}"
      );
      let window = laufey::Window::new(1200, 800);
      window.set_title("Ende Desktop DevTools");
      window.navigate(&url);
      let window = self.setup_window_events(window, false, Default::default());
      let id = window.id();
      // Track for HMR reload + the singleton check above.
      self.open_windows.lock().unwrap().insert(id);
      *self.devtools_window.lock().unwrap() = Some(id);
      return;
    }
    laufey::Window::from_id(window_id).open_devtools();
  }

  fn execute_js(
    &self,
    window_id: u32,
    script: &str,
    callback: Box<
      dyn FnOnce(
          Result<
            ende_runtime::ops::desktop::DesktopValue,
            ende_runtime::ops::desktop::DesktopValue,
          >,
        ) + Send
        + 'static,
    >,
  ) {
    laufey::Window::from_id(window_id).execute_js(
      script,
      Some(move |result: Result<laufey::Value, laufey::Value>| {
        // A value too deep to convert is reported to the caller as a
        // rejection rather than taken as a successful result.
        callback(match result {
          Ok(val) => laufey_value_to_desktop_value(val)
            .map_err(ende_runtime::ops::desktop::DesktopValue::String),
          Err(err) => Err(
            laufey_value_to_desktop_value(err)
              .unwrap_or_else(ende_runtime::ops::desktop::DesktopValue::String),
          ),
        });
      }),
    );
  }

  fn print_to_pdf(
    &self,
    window_id: u32,
    callback: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send + 'static>,
  ) {
    // No path: laufey would write it unchecked on the UI thread.
    laufey::Window::from_id(window_id).print_to_pdf(None, callback);
  }

  fn supports_print_to_pdf(&self) -> bool {
    BACKEND_API
      .get()
      .is_some_and(|api| api.print_to_pdf.is_some())
  }

  fn bind(&self, window_id: u32, name: &str) {
    let tx = self.event_tx.clone();
    let responses = self.pending_responses.clone();
    let name_owned = name.to_string();
    laufey::Window::from_id(window_id).add_binding_async(
      name,
      move |mut js_call| {
        let tx = tx.clone();
        let responses = responses.clone();
        let name = name_owned.clone();
        async move {
          // `mem::take` rather than `.iter().cloned()`: `js_call.resolve`
          // below consumes `js_call`, so the args can't simply be moved out
          // of the field, and cloning would deep-copy every argument —
          // binary payloads included — on the exact path this transport is
          // meant to make cheap for large buffers (#36498).
          let args = match std::mem::take(&mut js_call.args)
            .into_iter()
            .map(laufey_value_to_desktop_value)
            .collect::<Result<Vec<_>, _>>()
          {
            Ok(args) => args,
            Err(err) => {
              // Too deeply nested to convert without risking the runtime
              // thread's stack. Reject this call; the app keeps running.
              js_call.reject(laufey::Value::String(err));
              return;
            }
          };
          let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
          let call_id =
            ende_runtime::ops::desktop::register_bind_call(&responses, resp_tx);
          let event = ende_runtime::ops::desktop::DesktopEvent::BindCall {
            // Attribute the call to the window the binding was registered on,
            // not to `js_call.window_id`. The backend's per-call renderer id
            // can drift from the id `bind()` recorded the callback under (seen
            // on CEF/Windows when a larger module graph delays startup; see
            // denoland/deno#35647), which would make the runtime-side lookup
            // in `windowBindCallbacks` miss and reject an otherwise-registered
            // call. The registration id always matches that map's key.
            window_id,
            name,
            args,
            call_id,
          };
          if !tx.send(event) {
            js_call.reject(laufey::Value::String(
              "event channel closed".to_string(),
            ));
            return;
          }
          match resp_rx.await {
            Ok(Ok(result)) => {
              js_call.resolve(desktop_value_to_laufey_value(result));
            }
            Ok(Err(error)) => {
              js_call.reject(laufey::Value::String(error));
            }
            Err(_) => {
              js_call.reject(laufey::Value::String(
                "bind response channel dropped".to_string(),
              ));
            }
          }
        }
      },
    );
  }

  fn unbind(&self, window_id: u32, name: &str) {
    laufey::Window::from_id(window_id).unbind(name);
  }

  fn navigate(&self, window_id: u32, url: &str) {
    if let Some(guard) = &self.lost_scheme_guard {
      guard.navigated_by_app(window_id);
    }
    laufey::Window::from_id(window_id).navigate(url);
  }

  fn quit(&self) {
    laufey::quit();
  }

  fn set_application_menu(
    &self,
    window_id: u32,
    menu: Vec<endert::desktop::MenuItem>,
  ) {
    let menu = menu
      .into_iter()
      .map(desktop_menu_item_to_laufey_menu_item)
      .collect::<Vec<_>>();
    let tx = self.event_tx.clone();
    laufey::Window::from_id(window_id).set_menu(&menu, move |id: &str| {
      let _ = tx.send(ende_runtime::ops::desktop::DesktopEvent::AppMenuClick {
        window_id,
        id: id.to_string(),
      });
    });
  }

  fn show_context_menu(
    &self,
    window_id: u32,
    x: i32,
    y: i32,
    menu: Vec<endert::desktop::MenuItem>,
  ) {
    let menu = menu
      .into_iter()
      .map(desktop_menu_item_to_laufey_menu_item)
      .collect::<Vec<_>>();
    let tx = self.event_tx.clone();
    laufey::Window::from_id(window_id).show_context_menu(
      x,
      y,
      &menu,
      move |id: &str| {
        let _ =
          tx.send(ende_runtime::ops::desktop::DesktopEvent::ContextMenuClick {
            window_id,
            id: id.to_string(),
          });
      },
    );
  }

  fn get_raw_window_handle(
    &self,
    window_id: u32,
  ) -> Result<
    (
      raw_window_handle::RawWindowHandle,
      raw_window_handle::RawDisplayHandle,
    ),
    deno_error::JsErrorBox,
  > {
    let window = laufey::Window::from_id(window_id);
    let handle_type = window.get_window_handle_type();
    let raw_win = window.get_window_handle();
    let raw_display = window.get_display_handle();

    let null_window = || {
      deno_error::JsErrorBox::generic(no_native_window_error(
        self.backend.as_deref(),
      ))
    };
    let null_display = || {
      deno_error::JsErrorBox::generic("Laufey returned a null display handle")
    };

    match handle_type {
      laufey::LAUFEY_WINDOW_HANDLE_APPKIT => {
        use raw_window_handle::*;
        let win = RawWindowHandle::AppKit(AppKitWindowHandle::new(
          std::ptr::NonNull::new(raw_win).ok_or_else(null_window)?,
        ));
        let display = RawDisplayHandle::AppKit(AppKitDisplayHandle::new());
        Ok((win, display))
      }
      laufey::LAUFEY_WINDOW_HANDLE_WIN32 => {
        use raw_window_handle::*;
        let mut handle = Win32WindowHandle::new(
          std::num::NonZeroIsize::new(raw_win as isize)
            .ok_or_else(null_window)?,
        );
        handle.hinstance = std::num::NonZeroIsize::new(raw_display as isize);
        let win = RawWindowHandle::Win32(handle);
        let display = RawDisplayHandle::Windows(WindowsDisplayHandle::new());
        Ok((win, display))
      }
      laufey::LAUFEY_WINDOW_HANDLE_X11 => {
        use raw_window_handle::*;
        let win = RawWindowHandle::Xlib(XlibWindowHandle::new(raw_win as _));
        let display = RawDisplayHandle::Xlib(XlibDisplayHandle::new(
          std::ptr::NonNull::new(raw_display),
          0,
        ));
        Ok((win, display))
      }
      laufey::LAUFEY_WINDOW_HANDLE_WAYLAND => {
        use raw_window_handle::*;
        let win = RawWindowHandle::Wayland(WaylandWindowHandle::new(
          std::ptr::NonNull::new(raw_win).ok_or_else(null_window)?,
        ));
        let display = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
          std::ptr::NonNull::new(raw_display).ok_or_else(null_display)?,
        ));
        Ok((win, display))
      }
      laufey::LAUFEY_WINDOW_HANDLE_UNKNOWN => Err(null_window()),
      other => Err(deno_error::JsErrorBox::generic(format!(
        "unknown Laufey window handle type: {other}",
      ))),
    }
  }

  fn run_on_ui_thread(&self, f: Box<dyn FnOnce() + Send>) {
    run_on_ui_thread(BACKEND_API.get().copied(), f);
  }

  fn alert(&self, title: &str, message: &str) {
    let _dialog = self.close_requests.dialog();
    laufey::alert(title, message);
  }

  fn confirm(&self, title: &str, message: &str) -> bool {
    let _dialog = self.close_requests.dialog();
    laufey::confirm(title, message)
  }

  fn prompt(
    &self,
    title: &str,
    message: &str,
    default_value: &str,
  ) -> Option<String> {
    let _dialog = self.close_requests.dialog();
    laufey::prompt(title, message, default_value)
  }

  fn read_clipboard_text(&self) -> Option<String> {
    laufey::read_clipboard_text()
  }

  fn write_clipboard_text(&self, text: &str) {
    laufey::write_clipboard_text(text);
  }

  fn set_dock_badge(&self, text: &str) {
    laufey::set_dock_badge(if text.is_empty() { None } else { Some(text) });
  }

  fn bounce_dock(&self, critical: bool) {
    laufey::bounce_dock(if critical {
      laufey::DockBounceType::Critical
    } else {
      laufey::DockBounceType::Informational
    });
  }

  fn set_dock_menu(&self, menu: Option<Vec<endert::desktop::MenuItem>>) {
    match menu {
      Some(menu) => {
        let menu = menu
          .into_iter()
          .map(desktop_menu_item_to_laufey_menu_item)
          .collect::<Vec<_>>();
        let tx = self.event_tx.clone();
        laufey::set_dock_menu(&menu, move |id: &str| {
          let _ =
            tx.send(ende_runtime::ops::desktop::DesktopEvent::DockMenuClick {
              id: id.to_string(),
            });
        });
      }
      None => laufey::clear_dock_menu(),
    }
  }

  fn set_dock_visible(&self, visible: bool) {
    laufey::set_dock_visible(visible);
  }

  fn create_tray(&self) -> u32 {
    let tray = laufey::TrayIcon::new();
    let tray_id = tray.id();
    if tray_id == 0 {
      return 0;
    }
    let click_tx = self.event_tx.clone();
    let tray = tray.on_click(move || {
      let _ = click_tx
        .send(ende_runtime::ops::desktop::DesktopEvent::TrayClick { tray_id });
    });
    let dblclick_tx = self.event_tx.clone();
    tray.set_double_click_handler(move || {
      let _ = dblclick_tx.send(
        ende_runtime::ops::desktop::DesktopEvent::TrayDoubleClick { tray_id },
      );
    });
    self.trays.lock().unwrap().insert(tray_id, tray);
    tray_id
  }

  fn destroy_tray(&self, tray_id: u32) {
    self.trays.lock().unwrap().remove(&tray_id);
  }

  fn set_tray_icon(&self, tray_id: u32, png_bytes: &[u8]) {
    if let Some(tray) = self.trays.lock().unwrap().get(&tray_id) {
      tray.set_icon(png_bytes);
    }
  }

  fn set_tray_icon_dark(&self, tray_id: u32, png_bytes: Option<&[u8]>) {
    if let Some(tray) = self.trays.lock().unwrap().get(&tray_id) {
      tray.set_icon_dark(png_bytes.unwrap_or(&[]));
    }
  }

  fn set_tray_tooltip(&self, tray_id: u32, text: Option<&str>) {
    if let Some(tray) = self.trays.lock().unwrap().get(&tray_id) {
      tray.set_tooltip(text);
    }
  }

  fn set_tray_menu(
    &self,
    tray_id: u32,
    menu: Option<Vec<endert::desktop::MenuItem>>,
  ) {
    let trays = self.trays.lock().unwrap();
    let Some(tray) = trays.get(&tray_id) else {
      return;
    };
    match menu {
      Some(menu) => {
        let menu = menu
          .into_iter()
          .map(desktop_menu_item_to_laufey_menu_item)
          .collect::<Vec<_>>();
        let tx = self.event_tx.clone();
        tray.set_menu(&menu, move |id: &str| {
          let _ =
            tx.send(ende_runtime::ops::desktop::DesktopEvent::TrayMenuClick {
              tray_id,
              id: id.to_string(),
            });
        });
      }
      None => tray.clear_menu(),
    }
  }

  fn get_tray_bounds(&self, tray_id: u32) -> Option<(i32, i32, i32, i32)> {
    let trays = self.trays.lock().unwrap();
    trays.get(&tray_id)?.get_bounds()
  }

  fn show_notification(
    &self,
    title: &str,
    body: Option<&str>,
    icon: Option<&[u8]>,
    tag: Option<&str>,
    silent: Option<bool>,
    require_interaction: Option<bool>,
  ) -> u32 {
    if !self.notifications_supported {
      // The JS side fires `error` on the notification.
      return 0;
    }
    let mut builder = laufey::Notification::new(title);
    if let Some(body) = body {
      builder = builder.body(body);
    }
    if let Some(icon) = icon {
      builder = builder.icon(icon.to_vec());
    }
    if let Some(tag) = tag {
      builder = builder.tag(tag);
    }
    if let Some(silent) = silent {
      builder = builder.silent(silent);
    }
    if let Some(require) = require_interaction {
      builder = builder.require_interaction(require);
    }

    // The laufey handler closure receives only the event; it needs the
    // notification id to route the event through the desktop channel.
    // We can't know the id until `on_event` returns, so we capture it
    // through a shared slot populated immediately after.
    let id_slot: Arc<std::sync::OnceLock<u32>> =
      Arc::new(std::sync::OnceLock::new());
    let id_for_handler = id_slot.clone();
    let tx = self.event_tx.clone();
    let notifications = self.notifications.clone();

    let handle = builder.on_event(move |event| {
      let Some(&nid) = id_for_handler.get() else {
        return;
      };
      use laufey::NotificationEvent;
      let desktop_event = match event {
        NotificationEvent::Shown => {
          ende_runtime::ops::desktop::DesktopEvent::NotificationShow {
            notification_id: nid,
          }
        }
        NotificationEvent::Clicked => {
          ende_runtime::ops::desktop::DesktopEvent::NotificationClick {
            notification_id: nid,
          }
        }
        NotificationEvent::Closed => {
          // Reported by whoever untracks it, see close_tracked_notification.
          if !untrack_notification(&notifications, nid) {
            return;
          }
          ende_runtime::ops::desktop::DesktopEvent::NotificationClose {
            notification_id: nid,
          }
        }
        // The Web Notification API has no "action" event in window context;
        // surface action button clicks as a click event for compatibility.
        NotificationEvent::Action(_) => {
          ende_runtime::ops::desktop::DesktopEvent::NotificationClick {
            notification_id: nid,
          }
        }
      };
      let _ = tx.send(desktop_event);
    });

    let id = handle.id();
    if id == 0 {
      // Backend doesn't support notifications. Emit a synthetic error
      // event so the user can observe the failure.
      let _ = self.event_tx.send(
        ende_runtime::ops::desktop::DesktopEvent::NotificationError {
          notification_id: 0,
        },
      );
      return 0;
    }
    let _ = id_slot.set(id);
    self.notifications.lock().unwrap().insert(id, handle);
    id
  }

  fn close_notification(&self, notification_id: u32) {
    close_tracked_notification(
      &self.notifications,
      notification_id,
      laufey::NotificationHandle::close,
      || {
        let _ = self.event_tx.send(
          ende_runtime::ops::desktop::DesktopEvent::NotificationClose {
            notification_id,
          },
        );
      },
    );
  }

  fn notifications_supported(&self) -> bool {
    self.notifications_supported
  }

  fn request_notification_permission(
    &self,
    cb: Box<
      dyn FnOnce(ende_runtime::ops::desktop::PermissionState) + Send + 'static,
    >,
  ) {
    if !self.notifications_supported {
      return cb(ende_runtime::ops::desktop::PermissionState::Denied);
    }
    laufey::request_permission(
      laufey::PermissionKind::Notifications,
      move |status| cb(map_permission_status(status)),
    );
  }

  fn query_notification_permission(
    &self,
    cb: Box<
      dyn FnOnce(ende_runtime::ops::desktop::PermissionState) + Send + 'static,
    >,
  ) {
    if !self.notifications_supported {
      return cb(ende_runtime::ops::desktop::PermissionState::Denied);
    }
    laufey::query_permission(
      laufey::PermissionKind::Notifications,
      move |status| cb(map_permission_status(status)),
    );
  }
}

fn map_permission_status(
  status: laufey::PermissionStatus,
) -> ende_runtime::ops::desktop::PermissionState {
  use ende_runtime::ops::desktop::PermissionState;
  match status {
    laufey::PermissionStatus::Granted => PermissionState::Granted,
    laufey::PermissionStatus::Denied => PermissionState::Denied,
    laufey::PermissionStatus::Prompt => PermissionState::Prompt,
    laufey::PermissionStatus::Unsupported => PermissionState::Unsupported,
  }
}

fn desktop_menu_item_to_laufey_menu_item(
  item: endert::desktop::MenuItem,
) -> laufey::MenuItem {
  match item {
    endert::desktop::MenuItem::Item {
      label,
      id,
      accelerator,
      enabled,
      checked,
      icon,
      tooltip,
    } => laufey::MenuItem::Item {
      label,
      id,
      accelerator,
      enabled,
      checked,
      icon,
      tooltip,
    },
    endert::desktop::MenuItem::Submenu { label, items } => {
      laufey::MenuItem::Submenu {
        label,
        items: items
          .into_iter()
          .map(desktop_menu_item_to_laufey_menu_item)
          .collect(),
      }
    }
    endert::desktop::MenuItem::Separator => laufey::MenuItem::Separator,
    endert::desktop::MenuItem::Role { role } => laufey::MenuItem::Role { role },
  }
}

#[allow(dead_code, reason = "kept alongside v8_to_laufey_value for symmetry")]
fn laufey_value_to_v8<'a>(
  scope: &v8::PinScope<'a, '_>,
  val: laufey::Value,
) -> v8::Local<'a, v8::Value> {
  match val {
    laufey::Value::Null => v8::null(scope).into(),
    laufey::Value::Bool(bool) => v8::Boolean::new(scope, bool).into(),
    laufey::Value::Int(int) => v8::Integer::new(scope, int).into(),
    laufey::Value::Double(double) => v8::Number::new(scope, double).into(),
    laufey::Value::String(str) => v8::String::new(scope, &str).unwrap().into(),
    laufey::Value::List(list) => {
      let elements = list
        .into_iter()
        .map(|v| laufey_value_to_v8(scope, v))
        .collect::<Vec<_>>();
      v8::Array::new_with_elements(scope, &elements).into()
    }
    laufey::Value::Dict(dict) => {
      let mut names = Vec::with_capacity(dict.len());
      let mut values = Vec::with_capacity(dict.len());

      for (k, v) in dict {
        names.push(v8::String::new(scope, &k).unwrap().into());
        values.push(laufey_value_to_v8(scope, v));
      }

      let prototype = v8::null(scope).into();
      v8::Object::with_prototype_and_properties(
        scope, prototype, &names, &values,
      )
      .into()
    }
    laufey::Value::Binary(bin) => {
      let len = bin.len();
      let backing_store = v8::ArrayBuffer::new_backing_store_from_vec(bin);
      let backing_store = backing_store.make_shared();
      let ab = v8::ArrayBuffer::with_backing_store(scope, &backing_store);
      let uint8_array = v8::Uint8Array::new(scope, ab, 0, len).unwrap();
      uint8_array.into()
    }
  }
}

#[allow(dead_code, reason = "kept alongside laufey_value_to_v8 for symmetry")]
fn v8_to_laufey_value<'a>(
  scope: &v8::PinScope<'a, '_>,
  val: v8::Local<'a, v8::Value>,
) -> laufey::Value {
  if val.is_null_or_undefined() {
    laufey::Value::Null
  } else if val.is_boolean() {
    laufey::Value::Bool(val.boolean_value(scope))
  } else if val.is_int32() {
    laufey::Value::Int(val.int32_value(scope).unwrap_or(0))
  } else if val.is_number() {
    laufey::Value::Double(val.number_value(scope).unwrap_or(0.0))
  } else if val.is_string() {
    let s = val.to_rust_string_lossy(scope);
    laufey::Value::String(s)
  } else if val.is_array_buffer_view() {
    let view: v8::Local<v8::ArrayBufferView> = val.try_into().unwrap();
    let len = view.byte_length();
    let mut buf = vec![0u8; len];
    view.copy_contents(&mut buf);
    laufey::Value::Binary(buf)
  } else if val.is_array() {
    let arr: v8::Local<v8::Array> = val.try_into().unwrap();
    let len = arr.length();
    let mut list = Vec::with_capacity(len as usize);
    for i in 0..len {
      if let Some(elem) = arr.get_index(scope, i) {
        list.push(v8_to_laufey_value(scope, elem));
      }
    }
    laufey::Value::List(list)
  } else if val.is_object() {
    let obj: v8::Local<v8::Object> = val.try_into().unwrap();
    let mut map = std::collections::HashMap::new();
    if let Some(names) =
      obj.get_own_property_names(scope, v8::GetPropertyNamesArgs::default())
    {
      for i in 0..names.length() {
        if let Some(key) = names.get_index(scope, i) {
          let key_str = key.to_rust_string_lossy(scope);
          if let Some(value) = obj.get(scope, key) {
            map.insert(key_str, v8_to_laufey_value(scope, value));
          }
        }
      }
    }
    laufey::Value::Dict(map)
  } else {
    // Fallback: coerce to string
    laufey::Value::String(val.to_rust_string_lossy(scope))
  }
}

/// Get the filesystem path of this dylib using `dladdr`.
#[cfg(unix)]
fn get_dylib_path() -> Option<PathBuf> {
  #[repr(C)]
  struct DlInfo {
    dli_fname: *const std::ffi::c_char,
    dli_fbase: *mut std::ffi::c_void,
    dli_sname: *const std::ffi::c_char,
    dli_saddr: *mut std::ffi::c_void,
  }
  unsafe extern "C" {
    fn dladdr(
      addr: *const std::ffi::c_void,
      info: *mut DlInfo,
    ) -> std::ffi::c_int;
  }
  // SAFETY: `dladdr` is given a valid function pointer and a stack-allocated
  // `DlInfo`; on success we only read `dli_fname` after null-checking it.
  unsafe {
    let mut info: DlInfo = std::mem::zeroed();
    let addr = get_dylib_path as *const std::ffi::c_void;
    if dladdr(addr, &mut info) != 0 && !info.dli_fname.is_null() {
      let c_str = std::ffi::CStr::from_ptr(info.dli_fname);
      Some(PathBuf::from(c_str.to_string_lossy().into_owned()))
    } else {
      None
    }
  }
}

/// Replaces `dylib_path` with `src`, falling back to a copy when a direct
/// rename fails.
///
/// The fallback copies `src` to a temp file *beside the dylib*
/// (`<dylib>.<tmp_suffix>`) and renames that into place, so the final step is
/// a same-filesystem rename, which is the thing that makes it atomic: the
/// dylib is never observed half-written, however the copy goes.
///
/// The temp file lives inside the bundle for the duration, which is fine on
/// this path specifically — we're replacing the dylib, so the bundle's code
/// signature is being invalidated regardless.
#[cfg(unix)]
fn replace_dylib_with(src: &Path, dylib_path: &Path, tmp_suffix: &str) -> bool {
  if std::fs::rename(src, dylib_path).is_ok() {
    return true;
  }
  let ext = dylib_path.extension().unwrap_or_default().to_string_lossy();
  let tmp_path = dylib_path.with_extension(format!("{ext}.{tmp_suffix}"));
  let staged = std::fs::copy(src, &tmp_path).is_ok()
    && std::fs::rename(&tmp_path, dylib_path).is_ok();
  if !staged {
    let _ = std::fs::remove_file(&tmp_path);
  }
  staged
}

/// Set on the host that [`restart_host`] starts, to [`REEXEC_AFTER_SWAP`] or
/// [`REEXEC_AFTER_ROLLBACK`]. That process removes it at startup, so the app
/// and its child processes never see it. The name says "re-exec", as the docs
/// here do, but a macOS app bundle is restarted through LaunchServices
/// instead ([`restart_plan`]). The name can't change: the old version's code
/// sets it and the new version's code reads it.
#[cfg(unix)]
const UPDATE_REEXEC_ENV: &str = "ENDE_DESKTOP_UPDATE_REEXEC";
#[cfg(unix)]
const REEXEC_AFTER_SWAP: &str = "swapped";
#[cfg(unix)]
const REEXEC_AFTER_ROLLBACK: &str = "rolled-back";

/// What [`apply_pending_update`] made of this launch.
///
/// The update files live beside the dylib. `<dylib>.update` is staged by
/// `op_desktop_apply_patch`. `<dylib>.backup` is the dylib an update replaced.
/// `<dylib>.update-trial` is written when the new code first starts.
/// `<dylib>.update-ok` confirms the new code (`UpdateTrial` in ende_runtime).
/// Each launch checks them in this order:
///
/// 1. `.update` exists: swap it in, keeping the old dylib as `.backup`
///    ([`Swapped`]). `runtime_main` then re-execs the host, so the new code
///    runs in this same launch. If the swap fails, `.update` stays for the next
///    launch ([`Normal`]). A re-exec'd process never swaps, so it never re-execs
///    either.
/// 2. `.backup` and `.update-ok`: the update was confirmed. Delete `.backup`,
///    `.update-ok` and `.update-trial` ([`Normal`]).
/// 3. `.backup` without `.update-ok` or `.update-trial`: this is the new code's
///    first launch, its trial. Lock `.backup` ([`UpdateTrialLock`], held
///    until the process ends) and write `.update-trial` ([`Trial`]). Normally
///    this is the re-exec'd process; it's the next launch if the re-exec failed
///    or the process died between the swap and the exec.
/// 4. `.backup` and `.update-trial` without `.update-ok`, and `.backup`
///    locked: the trial launch is still running, so this is a second instance.
///    Leave the files to the trial ([`Normal`]).
/// 5. `.backup` and `.update-trial` without `.update-ok`, and `.backup` not
///    locked: the trial launch ended without confirming, so it crashed.
///    Restore `.backup`
///    ([`RolledBack`]). The code running is still the broken update's, so
///    `runtime_main` re-execs the host here too, and the restored code
///    reports the rollback. If the restore fails, keep the files so the next
///    launch retries ([`Normal`]).
/// 6. Anything else: nothing to do ([`Normal`]).
///
/// Only a [`Trial`] launch writes `.update-ok`, so an old process that keeps
/// running after a failed re-exec can't confirm an update it never ran. It
/// writes it once the app's main module has evaluated (top-level await
/// included) and the event loop has run one more turn, unless an uncaught
/// error has started the desktop error exit by then; or when the app exits
/// with code 0 before that. So an update whose main module throws, whose
/// top-level await rejects, or that leaves a startup rejection unhandled, is
/// rolled back. A crash later in the trial launch isn't. A re-exec'd process
/// never re-execs, whatever it finds.
///
/// [`Swapped`]: UpdateLaunch::Swapped
/// [`Normal`]: UpdateLaunch::Normal
/// [`Trial`]: UpdateLaunch::Trial
/// [`RolledBack`]: UpdateLaunch::RolledBack
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateLaunch {
  Normal,
  Swapped,
  Trial,
  RolledBack,
}

/// Held by an update's trial launch for as long as it runs: an exclusive
/// lock on `<dylib>.backup`. The kernel drops it when the process ends,
/// however it ends.
#[cfg(unix)]
struct UpdateTrialLock(
  #[allow(dead_code, reason = "held, not read")] std::fs::File,
);

/// Takes the trial lock, or logs why it couldn't (then a second launch
/// during the trial rolls the update back, as before the lock existed).
#[cfg(unix)]
#[allow(clippy::print_stderr, reason = "runs before logging is initialized")]
fn lock_update_trial(backup_path: &Path) -> Option<UpdateTrialLock> {
  let locked = std::fs::File::open(backup_path).and_then(|file| {
    file.try_lock()?;
    Ok(file)
  });
  match locked {
    Ok(file) => Some(UpdateTrialLock(file)),
    Err(e) => {
      eprintln!("[desktop] could not lock the update trial: {e}");
      None
    }
  }
}

/// Whether another process holds the trial lock: the trial launch is still
/// running. `false` when the lock can't be checked, so a crashed trial is
/// still rolled back.
#[cfg(unix)]
fn update_trial_is_running(backup_path: &Path) -> bool {
  std::fs::File::open(backup_path).is_ok_and(|file| {
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
  })
}

/// Manages pending updates and rollback on startup. See [`UpdateLaunch`] for
/// the states. `reexeced` is true in the process [`restart_host`] started. A
/// [`UpdateLaunch::Trial`] comes with the [`UpdateTrialLock`] to hold.
// Only invoked from the unix `runtime_main` startup path; gated to match its
// sole call site so it isn't flagged as dead code on other platforms.
#[cfg(unix)]
#[allow(clippy::print_stderr, reason = "runs before logging is initialized")]
fn apply_pending_update(
  dylib_path: &Path,
  reexeced: bool,
) -> (UpdateLaunch, Option<UpdateTrialLock>) {
  let ext = dylib_path.extension().unwrap_or_default().to_string_lossy();
  let update_path = dylib_path.with_extension(format!("{}.update", ext));
  let backup_path = dylib_path.with_extension(format!("{}.backup", ext));
  let sentinel_path = dylib_path.with_extension(format!("{}.update-ok", ext));
  let trial_path = dylib_path.with_extension(format!("{}.update-trial", ext));

  if update_path.exists() && reexeced {
    // The swap before the re-exec consumed `.update`, so this one was staged
    // since, or couldn't be deleted. Swapping again here could loop.
    eprintln!(
      "[desktop] another staged update is waiting; it applies on the next launch"
    );
  } else if update_path.exists() {
    // New update pending — apply it.
    // Remove stale state so we can detect if *this* update fails.
    let _ = std::fs::remove_file(&sentinel_path);
    let _ = std::fs::remove_file(&trial_path);
    let _ = std::fs::remove_file(&backup_path);

    // Hard-link (or copy) the live dylib aside as a backup *without*
    // unlinking the original. If the subsequent swap-in of the update
    // fails (perms, disk full, EXDEV), the running dylib is still in place
    // — previously a rename-then-rename pair could leave the app with no
    // dylib at all on rename #2 failure.
    let backup_ok = std::fs::hard_link(dylib_path, &backup_path).is_ok()
      || std::fs::copy(dylib_path, &backup_path).is_ok();
    if !backup_ok {
      eprintln!("[desktop] could not stage backup, skipping update");
      return (UpdateLaunch::Normal, None);
    }

    if replace_dylib_with(&update_path, dylib_path, "update.tmp") {
      // A successful direct rename already consumed `.update`; after a staged
      // copy it's still there, so drop it either way.
      let _ = std::fs::remove_file(&update_path);
      return (UpdateLaunch::Swapped, None);
    }
    // Couldn't apply the update by rename or copy. Leave `.update` in
    // place so the next launch retries, and delete the unused `.backup`
    // — otherwise the next launch would take the old dylib for an
    // update on trial.
    let _ = std::fs::remove_file(&backup_path);
    eprintln!(
      "[desktop] failed to apply staged update; will retry on next launch"
    );
    return (UpdateLaunch::Normal, None);
  }

  if !backup_path.exists() {
    return (UpdateLaunch::Normal, None);
  }

  if sentinel_path.exists() {
    // Previous update booted fine — clean up. The app is past any update it
    // rolled back before, so forget those too.
    let _ = std::fs::remove_file(&backup_path);
    let _ = std::fs::remove_file(&sentinel_path);
    let _ = std::fs::remove_file(&trial_path);
    let _ = std::fs::remove_file(
      ende_runtime::ops::desktop::rejected_updates_path(dylib_path),
    );
    return (UpdateLaunch::Normal, None);
  }

  if !trial_path.exists() {
    // The new code's first launch. It takes the trial lock before recording
    // the trial, so a launch that finds the record while this one runs finds
    // the lock taken too. If it can't record the trial, a crash here isn't
    // rolled back, but neither could a rollback be written.
    let lock = lock_update_trial(&backup_path);
    if let Err(e) = std::fs::write(&trial_path, b"") {
      eprintln!("[desktop] could not mark the update trial launch: {e}");
    }
    return (UpdateLaunch::Trial, lock);
  }

  if update_trial_is_running(&backup_path) {
    // Another instance is the trial launch, still running: it may yet
    // confirm the update. This one runs the update without confirming it.
    eprintln!(
      "[desktop] another launch is still trying the update; not rolling back"
    );
    return (UpdateLaunch::Normal, None);
  }

  // The trial launch didn't write the sentinel → it crashed. Rollback.
  eprintln!("[desktop] Last update failed to start, rolling back...");
  if let Err(e) = ende_runtime::ops::desktop::reject_update(dylib_path) {
    eprintln!(
      "[desktop] could not record the failed update, so it may be installed again: {e}"
    );
  }
  if !replace_dylib_with(&backup_path, dylib_path, "rollback.tmp") {
    // Report the truth rather than dispatching a "rolled back" event for a
    // rollback that didn't happen: the broken dylib is still in place, so
    // this launch will fail the same way. Keep `.backup` so the next launch
    // retries instead of treating the failed update as the new baseline.
    eprintln!("[desktop] rollback failed; keeping backup for next launch");
    return (UpdateLaunch::Normal, None);
  }
  // After a copy, `.backup` is still there and would read as a new trial.
  let _ = std::fs::remove_file(&backup_path);
  let _ = std::fs::remove_file(&trial_path);
  (UpdateLaunch::RolledBack, None)
}

/// Decides what `runtime_main` does after [`apply_pending_update`]: the
/// [`UPDATE_REEXEC_ENV`] value to re-exec with, if any, and what this launch
/// is. `reexec_reason` is the value this process was started with.
///
/// [`UpdateLaunch::Swapped`] and [`UpdateLaunch::RolledBack`] both leave this
/// process running code that's no longer on disk, so both re-exec. A re-exec'd
/// process never re-execs. After a rollback, the restored code reports it.
#[cfg(unix)]
fn after_update_check(
  launch: UpdateLaunch,
  reexec_reason: Option<&std::ffi::OsStr>,
) -> (Option<&'static str>, UpdateLaunch) {
  match reexec_reason {
    None => match launch {
      UpdateLaunch::Swapped => (Some(REEXEC_AFTER_SWAP), launch),
      UpdateLaunch::RolledBack => (Some(REEXEC_AFTER_ROLLBACK), launch),
      UpdateLaunch::Normal | UpdateLaunch::Trial => (None, launch),
    },
    Some(reason) if reason == REEXEC_AFTER_ROLLBACK => {
      (None, UpdateLaunch::RolledBack)
    }
    Some(_) => (None, launch),
  }
}

/// How [`restart_host`] starts the new run of the host.
#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum Restart {
  /// `execve` the host executable in this process ([`exec_host`]).
  Exec,
  /// Run `/usr/bin/open` with these arguments once this process has exited
  /// ([`open_after_exit`]).
  #[cfg(target_os = "macos")]
  LaunchServices(Vec<std::ffi::OsString>),
}

/// Picks how to restart the host executable `exe`, run with `args` (argv
/// without argv\[0]) and the environment `vars`, so the new run gets
/// [`UPDATE_REEXEC_ENV`] set to `reason`.
///
/// On macOS an app bundle's host goes through LaunchServices: `open -n
/// <bundle> --args <args>`, with every variable passed by `--env`.
/// LaunchServices registers the process it launched, and after an `execve`
/// the new image's `NSStatusItem`s (`Ende.Tray`) never reach the menu bar,
/// whether LaunchServices or a shell started the first one. `-n` because the
/// old process may still be registered when `open` runs, and `open` would
/// then only activate it. For a `--compress` app this is the extracted
/// bundle, the one whose dylib was updated; its thin launcher would only open
/// it again. Anything else, and every other unix, `execve`s.
#[cfg(unix)]
fn restart_plan(
  exe: &Path,
  args: &[std::ffi::OsString],
  vars: &[(std::ffi::OsString, std::ffi::OsString)],
  reason: &str,
) -> Restart {
  #[cfg(target_os = "macos")]
  if let Some(bundle) = app_bundle_of(exe) {
    use std::ffi::OsString;

    let mut open_args = vec![OsString::from("-n")];
    let reexec = (OsString::from(UPDATE_REEXEC_ENV), OsString::from(reason));
    for (key, value) in vars.iter().chain([&reexec]) {
      let mut var = key.clone();
      var.push("=");
      var.push(value);
      open_args.extend([OsString::from("--env"), var]);
    }
    open_args.push(bundle.as_os_str().to_owned());
    if !args.is_empty() {
      open_args.push("--args".into());
      open_args.extend_from_slice(args);
    }
    return Restart::LaunchServices(open_args);
  }
  let _ = (exe, args, vars, reason);
  Restart::Exec
}

/// The app bundle `exe` is the executable of: `<bundle>.app` when `exe` is
/// `<bundle>.app/Contents/MacOS/<name>` and `Contents/Info.plist` exists.
#[cfg(target_os = "macos")]
fn app_bundle_of(exe: &Path) -> Option<&Path> {
  let macos = exe.parent()?;
  let contents = macos.parent()?;
  let bundle = contents.parent()?;
  let is_bundle = macos.file_name()? == "MacOS"
    && contents.file_name()? == "Contents"
    && bundle.extension()? == "app"
    && contents.join("Info.plist").is_file();
  is_bundle.then_some(bundle)
}

/// Starts a new run of the host executable, so the dylib now on disk runs in
/// this same launch, with the same arguments and environment, plus
/// [`UPDATE_REEXEC_ENV`] set to `reason`. See [`restart_plan`] for how. Only
/// returns on failure, with the process as it was.
#[cfg(unix)]
fn restart_host(reason: &str) -> std::io::Error {
  let exe = match env::current_exe() {
    Ok(exe) => exe,
    Err(e) => return e,
  };
  let argv: Vec<_> = env::args_os().collect();
  let vars: Vec<_> = env::vars_os().collect();
  match restart_plan(&exe, argv.get(1..).unwrap_or_default(), &vars, reason) {
    Restart::Exec => exec_host(&exe, &argv, &vars, reason),
    #[cfg(target_os = "macos")]
    Restart::LaunchServices(open_args) => open_after_exit(&open_args),
  }
}

/// Replaces this process with a new run of the host executable `exe`, with
/// `argv` and `vars` plus [`UPDATE_REEXEC_ENV`] set to `reason`. Only returns
/// on failure, with the process as it was.
///
/// This runs before any of the app's code, but the backend is up: its UI
/// thread is in its event loop, and CEF has started its helper processes.
/// `execve` ends every thread, and the helpers lose their IPC channels and
/// exit. Descriptors without `FD_CLOEXEC` would stay open in the new image,
/// which starts its own backend and knows nothing of them (CEF's IPC sockets,
/// a profile lock), so every descriptor above stderr is marked close-on-exec
/// first, and restored if the exec fails. That also closes any extra
/// descriptor a launcher passed in, which a launch from Finder or a desktop
/// file doesn't do.
///
/// It calls `execve` itself because `Command::exec` resets SIGPIPE in this
/// process before it execs, and swaps the global `environ` while the backend's
/// threads may read it.
#[cfg(unix)]
fn exec_host(
  exe: &Path,
  argv: &[std::ffi::OsString],
  vars: &[(std::ffi::OsString, std::ffi::OsString)],
  reason: &str,
) -> std::io::Error {
  use std::ffi::CString;
  use std::os::unix::ffi::OsStrExt;

  fn c_string(bytes: Vec<u8>) -> std::io::Result<CString> {
    CString::new(bytes).map_err(std::io::Error::other)
  }
  let build = || -> std::io::Result<(CString, Vec<CString>, Vec<CString>)> {
    let exe = c_string(exe.as_os_str().as_bytes().to_vec())?;
    let argv = argv
      .iter()
      .map(|arg| c_string(arg.as_bytes().to_vec()))
      .collect::<std::io::Result<Vec<_>>>()?;
    let mut envp = Vec::new();
    for (key, value) in vars {
      let mut entry = key.as_bytes().to_vec();
      entry.push(b'=');
      entry.extend_from_slice(value.as_bytes());
      envp.push(c_string(entry)?);
    }
    envp.push(c_string(
      format!("{UPDATE_REEXEC_ENV}={reason}").into_bytes(),
    )?);
    Ok((exe, argv, envp))
  };
  let (exe, argv, envp) = match build() {
    Ok(parts) => parts,
    Err(e) => return e,
  };
  let mut argv_ptrs: Vec<*const libc::c_char> =
    argv.iter().map(|a| a.as_ptr()).collect();
  argv_ptrs.push(std::ptr::null());
  let mut envp_ptrs: Vec<*const libc::c_char> =
    envp.iter().map(|e| e.as_ptr()).collect();
  envp_ptrs.push(std::ptr::null());

  let marked = mark_fds_cloexec_above_stderr();
  // SAFETY: `exe` and every pointer in `argv_ptrs`/`envp_ptrs` point into
  // CStrings that outlive the call, and both arrays end in a null pointer.
  unsafe { libc::execve(exe.as_ptr(), argv_ptrs.as_ptr(), envp_ptrs.as_ptr()) };
  let error = std::io::Error::last_os_error();
  clear_cloexec(&marked);
  error
}

/// Exits this process, and has `/usr/bin/open` run with `open_args` once it
/// has. Only returns on failure, with the process as it was.
///
/// Like [`exec_host`], the old run ends before the new one starts, so the two
/// never share the app's data (CEF's profile, WebKit's storage). A waiter
/// ([`spawn_after_exit`]) runs `open` when this process is gone. If `open`
/// fails, the waiter says so on this process's stderr, and the app runs the
/// update or the restored version on its next launch. Descriptors above stderr
/// are marked close-on-exec first, as in [`exec_host`], so the waiter and
/// `open` hold none of the backend's files.
#[cfg(target_os = "macos")]
fn open_after_exit(open_args: &[std::ffi::OsString]) -> std::io::Error {
  let marked = mark_fds_cloexec_above_stderr();
  match spawn_after_exit(Path::new("/usr/bin/open"), open_args) {
    Ok(_alive) => {
      // SAFETY: `_exit` ends the process at once, as `execve` would have:
      // no destructors or atexit handlers run while the backend's threads
      // are still going. Exiting closes `_alive`'s write end.
      unsafe { libc::_exit(0) }
    }
    Err(e) => {
      clear_cloexec(&marked);
      e
    }
  }
}

/// Spawns a shell that runs `program` with `args` once every copy of the
/// returned pipe end is closed. Only this process holds it (pipes are made
/// close-on-exec), so that's when this process has exited. The shell's stdin
/// is the pipe's read end, and `read` returns at end of file.
#[cfg(target_os = "macos")]
fn spawn_after_exit(
  program: &Path,
  args: &[std::ffi::OsString],
) -> std::io::Result<std::io::PipeWriter> {
  const WAIT_THEN_RUN: &str =
    r#"read -r _; "$@" || echo "[desktop] could not restart the app" >&2"#;
  let (reader, writer) = std::io::pipe()?;
  std::process::Command::new("/bin/sh")
    .arg("-c")
    .arg(WAIT_THEN_RUN)
    .arg("sh")
    .arg(program)
    .args(args)
    .stdin(reader)
    .spawn()?;
  Ok(writer)
}

/// Undoes [`mark_fds_cloexec_above_stderr`].
#[cfg(unix)]
fn clear_cloexec(fds: &[libc::c_int]) {
  for &fd in fds {
    // SAFETY: plain fcntl calls on a descriptor number; one closed since
    // fails with EBADF and is left alone.
    unsafe {
      let flags = libc::fcntl(fd, libc::F_GETFD);
      if flags >= 0 {
        libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
      }
    }
  }
}

/// Marks every open descriptor above stderr close-on-exec, and returns the
/// ones that weren't, for [`exec_host`] (and `open_after_exit` on macOS) to
/// restore.
#[cfg(unix)]
fn mark_fds_cloexec_above_stderr() -> Vec<libc::c_int> {
  #[cfg(target_os = "linux")]
  const FD_DIR: &str = "/proc/self/fd";
  #[cfg(not(target_os = "linux"))]
  const FD_DIR: &str = "/dev/fd";
  // Collect first: the directory handle is itself an open descriptor (with
  // FD_CLOEXEC, and closed by the time the loop below runs).
  let fds: Vec<libc::c_int> = match std::fs::read_dir(FD_DIR) {
    Ok(entries) => entries
      .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
      .filter(|fd| *fd > libc::STDERR_FILENO)
      .collect(),
    Err(_) => return Vec::new(),
  };
  let mut marked = Vec::new();
  for fd in fds {
    // SAFETY: plain fcntl calls on a descriptor number; one that isn't open
    // fails with EBADF and is skipped.
    unsafe {
      let flags = libc::fcntl(fd, libc::F_GETFD);
      if flags >= 0
        && flags & libc::FD_CLOEXEC == 0
        && libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) == 0
      {
        marked.push(fd);
      }
    }
  }
  marked
}

/// The backend's C API table, for what laufey 0.7.0's Rust API doesn't wrap
/// (`post_ui_task`, see [`run_on_ui_thread`]). Set by `laufey_runtime_init`.
static BACKEND_API: OnceLock<&'static laufey::LaufeyBackendApi> =
  OnceLock::new();

// The three Laufey C ABI exports. Ende defines them itself instead of using
// `laufey::main!`, which hands the API pointer to laufey without keeping it.

/// Called by the backend before `laufey_runtime_start`.
///
/// # Safety
///
/// `api` must be null or point to a `LaufeyBackendApi` that lives for the rest
/// of the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn laufey_runtime_init(
  api: *const laufey::LaufeyBackendApi,
) -> std::ffi::c_int {
  // SAFETY: forwarded from our caller, same contract.
  let rc = unsafe { laufey::init_api(api) };
  if rc == 0 {
    // SAFETY: `init_api` accepted it, so it's non-null, and our caller
    // guarantees it lives for the rest of the process.
    let _ = BACKEND_API.set(unsafe { &*api });
  }
  rc
}

/// Called by the backend on its runtime-loader thread; runs the app.
#[unsafe(no_mangle)]
pub extern "C" fn laufey_runtime_start() -> std::ffi::c_int {
  runtime_main();
  0
}

/// Called by the backend when the app quits.
#[unsafe(no_mangle)]
pub extern "C" fn laufey_runtime_shutdown() {
  cookie_jar::flush_on_exit();
  laufey::shutdown();
}

/// Run `f` on the backend's UI thread and wait until it has run.
///
/// laufey 0.7.0 has no Rust wrapper for the C API's `post_ui_task`; the
/// unreleased littledivy/laufey#79 adds `laufey::run_on_ui_thread`. So this
/// posts through the API table that `laufey_runtime_init` keeps. If a laufey
/// release ships `run_on_ui_thread`, switch to it.
///
/// Only macOS hops, because AppKit is main-thread-only. `f` runs inline
/// elsewhere, on the main thread itself, and without a `post_ui_task`.
///
/// Waits for as long as the UI thread takes, e.g. while a modal dialog is up.
/// If the backend drops the task because its event loop has ended, this never
/// returns, but then the process is exiting.
fn run_on_ui_thread(
  api: Option<&laufey::LaufeyBackendApi>,
  f: Box<dyn FnOnce() + Send>,
) {
  #[cfg(target_os = "macos")]
  {
    unsafe extern "C" {
      fn pthread_main_np() -> std::ffi::c_int;
    }
    // SAFETY: libSystem function with no preconditions.
    let on_main_thread = unsafe { pthread_main_np() } != 0;
    if let Some(api) = api
      && let Some(post_ui_task) = api.post_ui_task
      && !on_main_thread
    {
      type Task = Box<dyn FnOnce() + Send>;
      unsafe extern "C" fn run_task(data: *mut std::ffi::c_void) {
        // SAFETY: `data` is the `Box<Task>` leaked below, and the backend
        // runs each posted task once.
        let task = unsafe { Box::from_raw(data.cast::<Task>()) };
        // Don't unwind into the backend's C frames. A panic drops `done_tx`,
        // which ends the wait below.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
      }
      let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
      let task: Box<Task> = Box::new(Box::new(move || {
        f();
        let _ = done_tx.send(());
      }));
      // SAFETY: `post_ui_task` comes from the backend's API table with its
      // `backend_data`. `run_task` takes ownership of the task.
      unsafe {
        post_ui_task(
          api.backend_data,
          Some(run_task),
          Box::into_raw(task).cast(),
        );
      }
      let _ = done_rx.recv();
      return;
    }
  }
  #[cfg(not(target_os = "macos"))]
  let _ = api;
  f();
}

/// Variables only `ende desktop`'s development runs set: `--hmr`, and
/// `--inspect` (also `--inspect-brk` and `--inspect-wait`).
#[cfg(any(windows, test))]
const DEV_RUN_ENV_VARS: &[&str] =
  &["ENDE_DESKTOP_HMR", "ENDE_DESKTOP_INSPECT_INTERNAL_PORT"];

/// Whether `ende desktop` started this process to develop the app, judged by
/// which variables are set.
#[cfg(any(windows, test))]
fn is_dev_run(is_set: impl Fn(&str) -> bool) -> bool {
  DEV_RUN_ENV_VARS.iter().any(|name| is_set(name))
}

/// Attaches to the parent's console when `ende desktop` runs the app in
/// development mode (`--hmr` or `--inspect`), so `console.log` output reaches
/// the terminal.
///
/// The backend is a GUI-subsystem executable. Windows gives such a process no
/// console, and no standard handles even when the parent's are inherited, so
/// its output went nowhere (#36501). Only development runs set
/// [`DEV_RUN_ENV_VARS`], so an app launched normally, e.g. from Explorer, keeps
/// running without a console.
#[cfg(windows)]
fn attach_parent_console_in_dev_mode() {
  use windows_sys::Win32::Foundation::GENERIC_READ;
  use windows_sys::Win32::Foundation::GENERIC_WRITE;
  use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
  use windows_sys::Win32::Storage::FileSystem::CreateFileW;
  use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
  use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE;
  use windows_sys::Win32::Storage::FileSystem::OPEN_EXISTING;
  use windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS;
  use windows_sys::Win32::System::Console::AttachConsole;
  use windows_sys::Win32::System::Console::GetConsoleWindow;
  use windows_sys::Win32::System::Console::GetStdHandle;
  use windows_sys::Win32::System::Console::STD_ERROR_HANDLE;
  use windows_sys::Win32::System::Console::STD_OUTPUT_HANDLE;
  use windows_sys::Win32::System::Console::SetStdHandle;

  if !is_dev_run(|name| env::var_os(name).is_some()) {
    return;
  }
  // SAFETY: `CreateFileW` gets a NUL-terminated UTF-16 string that outlives
  // the call; the other functions take no pointers. Failures are reported
  // through return values, which are checked.
  unsafe {
    if !GetConsoleWindow().is_null()
      || AttachConsole(ATTACH_PARENT_PROCESS) == 0
    {
      return;
    }
    let conout: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
    for kind in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
      // Keep handles the parent passed explicitly, e.g. pipes or files.
      let current = GetStdHandle(kind);
      if !current.is_null() && current != INVALID_HANDLE_VALUE {
        continue;
      }
      let handle = CreateFileW(
        conout.as_ptr(),
        GENERIC_READ | GENERIC_WRITE,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        std::ptr::null(),
        OPEN_EXISTING,
        0,
        std::ptr::null_mut(),
      );
      if handle != INVALID_HANDLE_VALUE {
        SetStdHandle(kind, handle);
      }
    }
  }
}

/// The variable WebView2 reads its user data folder from.
#[cfg(any(windows, test))]
const WEBVIEW2_USER_DATA_FOLDER: &str = "WEBVIEW2_USER_DATA_FOLDER";

/// The folder to default `WEBVIEW2_USER_DATA_FOLDER` to, or `None` to leave it
/// alone.
///
/// Without it, WebView2 keeps its data next to the exe, which a normal user
/// can't write under Program Files: an MSI-installed app gets E_ACCESSDENIED
/// and a blank window (#36768). The default is
/// `%LOCALAPPDATA%\<identifier>\WebView2`: per user and not roaming (it's
/// mostly cache), keyed by the app identifier, which is stable across renames
/// and unique across vendors, and in a subfolder so the identifier's folder
/// can hold other app data. WebView2 adds `EBWebView` inside it.
///
/// `None` when the user set the variable, in the environment or in the app's
/// embedded env file, when `LOCALAPPDATA` is unset, or when the identifier
/// isn't usable as a folder name.
#[cfg(any(windows, test))]
fn default_webview2_user_data_folder<'a>(
  current: Option<&std::ffi::OsStr>,
  mut env_file_keys: impl Iterator<Item = &'a str>,
  local_app_data: Option<&std::ffi::OsStr>,
  identifier: Option<&str>,
) -> Option<PathBuf> {
  let user_set = current.is_some_and(|v| !v.is_empty())
    || env_file_keys.any(|k| k.eq_ignore_ascii_case(WEBVIEW2_USER_DATA_FOLDER));
  if user_set {
    return None;
  }
  let local_app_data = local_app_data.filter(|v| !v.is_empty())?;
  let identifier = identifier.filter(|id| is_usable_folder_name(id))?;
  Some(
    PathBuf::from(local_app_data)
      .join(identifier)
      .join("WebView2"),
  )
}

/// Whether a reverse-DNS app identifier works as a single Windows folder
/// name: `[A-Za-z0-9.-]`, no leading or trailing dot (Windows drops trailing
/// dots), and no reserved device name before the first dot (`nul.app` opens
/// the NUL device).
#[cfg(any(windows, test))]
fn is_usable_folder_name(id: &str) -> bool {
  if id.is_empty()
    || id.len() > 155
    || id.starts_with('.')
    || id.ends_with('.')
    || !id
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
  {
    return false;
  }
  let stem = id.split('.').next().unwrap_or(id).to_ascii_lowercase();
  let numbered = stem.len() == 4
    && (stem.starts_with("com") || stem.starts_with("lpt"))
    && stem.as_bytes()[3].is_ascii_digit();
  !(numbered || matches!(stem.as_str(), "con" | "prn" | "aux" | "nul"))
}

/// Defaults `WEBVIEW2_USER_DATA_FOLDER` to the app's own folder, see
/// [`default_webview2_user_data_folder`]. Runs before the first window, and
/// so before the backend creates a WebView2 environment.
#[cfg(windows)]
fn set_default_webview2_user_data_folder(
  metadata: &ende_lib::standalone::binary::Metadata,
) {
  let current = env::var_os(WEBVIEW2_USER_DATA_FOLDER);
  let local_app_data = env::var_os("LOCALAPPDATA");
  let Some(folder) = default_webview2_user_data_folder(
    current.as_deref(),
    metadata.env_vars_from_env_file.keys().map(String::as_str),
    local_app_data.as_deref(),
    metadata.app_identifier.as_deref(),
  ) else {
    return;
  };
  // WebView2 would create it as well. Creating it here means a failure keeps
  // WebView2's own default instead of a window that can't start.
  if let Err(err) = std::fs::create_dir_all(&folder) {
    log::warn!(
      "[desktop] could not create the WebView2 user data folder {}: {err}",
      folder.display()
    );
    return;
  }
  // SAFETY: Rust documents `set_var` as always safe on Windows.
  unsafe { env::set_var(WEBVIEW2_USER_DATA_FOLDER, &folder) };
  // WebView2 reads it at each environment creation, so it stays set, but a
  // WebView2 app this app starts must not share the folder.
  ende_runtime::ende_os::keep_env_var_from_child_processes(
    WEBVIEW2_USER_DATA_FOLDER,
  );
}

/// The host of the `app://<host>` origin the renderer loads the app from,
/// or `None` to keep the random loopback port. Only the webview backend on
/// macOS and Windows gets `app://`, and only with a baked host:
///
/// - Linux webview: laufey 0.7.0's WebKitGTK host doesn't forward request
///   bodies (`WebKitURISchemeRequest` doesn't expose them), so every POST
///   would reach the app empty.
/// - CEF: its profile is a per-process temporary directory, so storage
///   wouldn't persist anyway, and Chromium's Local Network Access likely
///   blocks an `app://` page's WebSocket to the loopback relay.
/// - raw: no web renderer and no scheme handler.
/// - No backend or host in the metadata: built before they were baked, or
///   no identifier derives from the app's name.
fn app_scheme_host<'a>(
  backend: Option<&str>,
  os: &str,
  host: Option<&'a str>,
) -> Option<&'a str> {
  match (backend, os) {
    (Some("webview"), "macos" | "windows") => host,
    _ => None,
  }
}

/// Whether the navigate fallback shows the initial window, 10 s after
/// navigating it: only if its first load never finished (that load reveals
/// it, and showing it again seems to replay the open animation on the macOS
/// webview), and never a window the app hid.
fn navigate_fallback_shows(loaded: bool, hidden_by_app: bool) -> bool {
  !loaded && !hidden_by_app
}

/// Whether `new Notification()` can work on this backend and OS. Not on the
/// raw backend on macOS: laufey 0.7.0's notify_rust -> mac_notification_sys
/// runs an NSAppleScript to find the bundle identifier that never returns,
/// which hangs the JS thread for good. Until laufey fixes it, Ende refuses
/// notifications there instead (`Notification.permission` is "denied").
fn notifications_supported(backend: Option<&str>, os: &str) -> bool {
  !matches!((backend, os), (Some("raw"), "macos"))
}

/// The message of the error `getNativeWindow()` throws when a window has no
/// native handle on `backend` (`None`: built before the backend was baked in,
/// so the default webview one). laufey 0.7.0's webview windows report no
/// handle type, and its CEF windows a null one (seen on macOS); only raw
/// backend windows have one.
fn no_native_window_error(backend: Option<&str>) -> String {
  match backend {
    Some("raw") => "Laufey returned no native handle for this window".into(),
    backend => format!(
      "getNativeWindow() needs the raw backend: windows of the {} backend \
       have no native handle. Set \"desktop\": {{ \"backend\": \"raw\" }} in \
       ende.json or build with `ende desktop --backend raw`.",
      backend.unwrap_or("webview"),
    ),
  }
}

/// The `app://<host>` bridge's cookie jar, persisted under the app
/// identifier in the platform's data folder (see [`cookie_jar::cookie_file`])
/// and written when the app exits. In memory only when there's no usable
/// folder.
fn open_cookie_jar(
  host: &str,
  identifier: Option<&str>,
) -> Arc<cookie_jar::CookieJar> {
  let file = endert::binary::get_data_local_dir()
    .zip(identifier)
    .and_then(|(dir, id)| cookie_jar::cookie_file(&dir, id));
  if file.is_none() {
    log::warn!(
      "[desktop] no data folder for the app's cookies; they last until the app quits"
    );
  }
  let jar = cookie_jar::CookieJar::open(host, file);
  cookie_jar::flush_at_exit(&jar);
  jar
}

/// How the renderer reaches the app's server.
enum RendererTransport {
  /// `app://<host>/`: the app serves on the memory listener `<host>`, and
  /// [`scheme_bridge`] forwards the webview's requests to it. The page's
  /// WebSockets go through the loopback relay bound here, if it could be.
  AppScheme {
    origin: scheme_bridge::AppOrigin,
    ws_relay: Option<std::net::TcpListener>,
  },
  /// `http://127.0.0.1:<port>/`: the app serves on that loopback port.
  Loopback { port: u16 },
}

impl RendererTransport {
  /// The transport [`app_scheme_host`] picks for this app on this OS. `None`
  /// (logged) when no loopback port could be allocated.
  fn choose(metadata: &ende_lib::standalone::binary::Metadata) -> Option<Self> {
    if let Some(host) = app_scheme_host(
      metadata.desktop_backend.as_deref(),
      env::consts::OS,
      metadata.app_origin_host.as_deref(),
    ) {
      let ws_relay = match scheme_bridge::bind_ws_loopback_listener() {
        Ok(listener) => Some(listener),
        Err(e) => {
          log::error!(
            "[desktop] failed to bind the WebSocket relay: {e}; the page can't open WebSockets to the app"
          );
          None
        }
      };
      return Some(Self::AppScheme {
        origin: scheme_bridge::AppOrigin::new(host),
        ws_relay,
      });
    }
    match allocate_random_port() {
      Ok(port) => Some(Self::Loopback { port }),
      Err(e) => {
        log::error!("[desktop] failed to allocate serve port: {}", e);
        None
      }
    }
  }

  /// The address the app's first `Ende.serve` binds.
  fn serve_address(&self) -> String {
    match self {
      Self::AppScheme { origin, .. } => format!("memory:{}", origin.host()),
      Self::Loopback { port } => format!("tcp:127.0.0.1:{port}"),
    }
  }

  /// Publish the serve address as `ENDE_SERVE_ADDRESS` (and the deprecated
  /// `DENO_SERVE_ADDRESS`, so user code that reads it keeps working) and the
  /// relay URL as `ENDE_DESKTOP_WS_URL`, to the runtime only.
  fn publish_env(&self) {
    let serve_address = self.serve_address();
    for key in ["ENDE_SERVE_ADDRESS", "DENO_SERVE_ADDRESS"] {
      ende_runtime::ende_os::set_process_local_env_var(key, &serve_address);
    }
    if let Self::AppScheme {
      ws_relay: Some(listener),
      ..
    } = self
      && let Ok(addr) = listener.local_addr()
      && let Some(url) = scheme_bridge::ws_relay_url(addr)
    {
      ende_runtime::ende_os::set_process_local_env_var(
        scheme_bridge::WS_URL_ENV,
        url,
      );
    }
  }

  /// The URL the initial window loads.
  fn url(&self) -> String {
    match self {
      Self::AppScheme { origin, .. } => origin.root_url(),
      Self::Loopback { port } => format!("http://127.0.0.1:{port}"),
    }
  }

  /// Whether the app's server answers `GET /` with a 2xx or 3xx.
  async fn server_ready(&self) -> bool {
    match self {
      Self::AppScheme { origin, .. } => {
        match ende_runtime::ende_net::memory::connect_memory(origin.host()) {
          Ok(stream) => answers_get_root(stream, origin.host()).await,
          Err(_) => false,
        }
      }
      Self::Loopback { port } => {
        match tokio::net::TcpStream::connect(("127.0.0.1", *port)).await {
          Ok(stream) => {
            answers_get_root(stream, &format!("127.0.0.1:{port}")).await
          }
          Err(_) => false,
        }
      }
    }
  }
}

/// Whether a server answers `GET /` on `stream` with a 2xx or 3xx. A full
/// request, not just a connect: frameworks like Vite accept connections
/// before they're ready to serve.
async fn answers_get_root(
  mut stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
  host: &str,
) -> bool {
  use tokio::io::AsyncReadExt;
  use tokio::io::AsyncWriteExt;

  let req =
    format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
  if stream.write_all(req.as_bytes()).await.is_err() {
    return false;
  }
  let mut buf = vec![0u8; 256];
  let Ok(n) = stream.read(&mut buf).await else {
    return false;
  };
  let response = String::from_utf8_lossy(&buf[..n]);
  ["HTTP/1.1 2", "HTTP/1.1 3", "HTTP/1.0 2", "HTTP/1.0 3"]
    .iter()
    .any(|prefix| response.starts_with(prefix))
}

fn runtime_main() {
  #[cfg(windows)]
  attach_parent_console_in_dev_mode();

  // Apply any pending update before anything else.
  #[cfg(unix)]
  #[allow(clippy::print_stderr, reason = "runs before logging is initialized")]
  // A trial launch holds its lock until the process ends.
  let (update_launch, _update_trial_lock) = {
    let reexec_reason = env::var_os(UPDATE_REEXEC_ENV);
    if reexec_reason.is_some() {
      // SAFETY: none of our threads exist yet; see the setenv comment below.
      unsafe { env::remove_var(UPDATE_REEXEC_ENV) };
    }
    let (launch, trial_lock) = match std::panic::catch_unwind(|| {
      if let Some(ref dylib_path) = get_dylib_path() {
        eprintln!("[desktop] dylib path: {:?}", dylib_path);
        apply_pending_update(dylib_path, reexec_reason.is_some())
      } else {
        eprintln!("[desktop] could not determine dylib path");
        (UpdateLaunch::Normal, None)
      }
    }) {
      Ok(v) => v,
      Err(e) => {
        eprintln!("[desktop] update check panicked: {:?}", e);
        (UpdateLaunch::Normal, None)
      }
    };
    let (restart, launch) =
      after_update_check(launch, reexec_reason.as_deref());
    if let Some(reason) = restart {
      let target = if reason == REEXEC_AFTER_SWAP {
        "the applied update"
      } else {
        "the restored version"
      };
      eprintln!("[desktop] restarting into {target}");
      let error = restart_host(reason);
      // Still this code. After a swap, the new dylib gets its trial launch
      // next time (state 3 in `UpdateLaunch`).
      eprintln!(
        "[desktop] warning: could not restart into {target} ({error}); it runs on the next launch"
      );
    }
    (launch, trial_lock)
  };
  #[cfg(unix)]
  let update_rolled_back = update_launch == UpdateLaunch::RolledBack;
  #[cfg(unix)]
  let update_trial = update_launch == UpdateLaunch::Trial;
  #[cfg(not(unix))]
  let (update_rolled_back, update_trial) = (false, false);

  // Set up panic hook for desktop error reporting.  The error reporting
  // URL is only known after binary metadata is parsed (in run_desktop),
  // so the hook reads from a global that gets set later.
  {
    let orig_hook = std::panic::take_hook();
    #[allow(clippy::print_stderr, reason = "panic hook")]
    std::panic::set_hook(Box::new(move |panic_info| {
      use ende_runtime::ops::desktop::error_report_config;
      use ende_runtime::ops::desktop::send_error_report;

      if let Some((url, app_version)) = error_report_config() {
        let message = if let Some(s) =
          panic_info.payload().downcast_ref::<&str>()
        {
          (*s).to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
          s.clone()
        } else {
          "Ende runtime panicked".to_string()
        };

        let location = panic_info
          .location()
          .map(|l| format!("at {}:{}:{}", l.file(), l.line(), l.column()));

        let body = ende_core::serde_json::json!({
          "version": 1,
          "message": message,
          "stack": location,
          "appVersion": app_version,
          "platform": env::consts::OS,
          "arch": env::consts::ARCH,
        });
        send_error_report(url, &body.to_string());
      }

      eprintln!(
        "\n============================================================"
      );
      eprintln!("Ende has panicked. This is a bug in Ende. Please report this");
      eprintln!("at https://github.com/theende/ende/issues/new.");
      eprintln!();
      eprintln!("Platform: {} {}", env::consts::OS, env::consts::ARCH);
      eprintln!();

      orig_hook(panic_info);
      ende_runtime::exit(1);
    }));
  }

  endert::init_logging(None, None);

  ende_runtime::ende_permissions::mark_desktop();

  rustls::crypto::aws_lc_rs::default_provider()
    .install_default()
    .unwrap();

  laufey::set_js_namespace("bindings");

  // Read the embedded standalone section before the tokio runtime starts.
  let args: Vec<_> = env::args_os().collect();
  let data = match endert::binary::extract_standalone_with_finder(
    Cow::Owned(args),
    find_section_in_dylib,
  ) {
    Ok(data) => data,
    Err(e) => {
      log::error!("[desktop] failed to read standalone section: {:?}", e);
      return;
    }
  };
  #[cfg(windows)]
  set_default_webview2_user_data_folder(&data.metadata);

  // How the renderer reaches the app, and the serve address (plus the
  // WebSocket relay URL) published for the app's code. They go into the
  // runtime's process-local environment (`ende_os::set_process_local_env_var`),
  // not the process environment: they name this process's own server, so a
  // child process the app starts must not inherit them (a child `ende` would
  // try to serve on the same address), and the relay URL holds its secret.
  let Some(transport) = RendererTransport::choose(&data.metadata) else {
    return;
  };
  transport.publish_env();

  // Everything above must stay on this (still effectively single-threaded)
  // loader thread: `set_default_webview2_user_data_folder` calls `setenv`,
  // which is only safe before the runtime spins up its IO thread (and,
  // optionally, the inspector server thread). The init calls above
  // (init_logging, mark_desktop, rustls install_default,
  // set_js_namespace) don't spawn threads. The runtime itself moves to a
  // dedicated thread with a real stack; the loader thread just parks in
  // `join` (inside `run_on_runtime_thread`) until the app exits.
  run_on_runtime_thread(move || {
    let rt = ende_runtime::tokio_util::create_basic_runtime();

    rt.block_on(async {
      log::debug!("[desktop] run_desktop starting");
      match run_desktop(update_rolled_back, update_trial, transport, data).await
      {
        Ok(()) => log::debug!("[desktop] run_desktop completed OK"),
        Err(error) => {
          // A failure raised while the app's main module was still loading or
          // first evaluating is tagged `DesktopStartupError` (see
          // `endert::run`). It carries its own pre-formatted message.
          let startup =
            error.downcast_ref::<endert::run::DesktopStartupError>();
          let error_string = match startup {
            Some(startup) => startup.message.clone(),
            None => match js_error_downcast_ref(&error) {
              Some(js_error) => format_js_error(js_error, None),
              None => format!("{:?}", error),
            },
          };
          let message = error_string.trim_start_matches("error: ");
          log::error!("{}: {}", colors::red_bold("error"), message);
          let is_startup = startup.is_some();
          // A `DesktopStartupError` wraps the formatted message rather than the
          // original `JsError`, so only consult `js_error_downcast_ref` for the
          // untagged (post-load) case.
          let is_js_error =
            !is_startup && js_error_downcast_ref(&error).is_some();
          if should_show_native_error_dialog(is_startup, is_js_error) {
            laufey::alert("Application Error", message);
          }
        }
      }
    });
  });
}

/// Decide whether the desktop shell should pop a native error dialog for a
/// failure that propagated out of the runtime.
///
/// - Startup failures (tagged `DesktopStartupError`; see its docs) are
///   surfaced here because the app's own `error` / `unhandledrejection`
///   listeners never ran to report them.
/// - Non-JS crashes are surfaced too.
/// - A post-load JS error is left to the app's own listeners, which already
///   show a dialog; alerting again here would just duplicate it.
fn should_show_native_error_dialog(
  is_startup: bool,
  is_js_error: bool,
) -> bool {
  is_startup || !is_js_error
}

/// Stack size for the thread the Ende runtime runs on. Laufey invokes
/// `laufey_runtime_start` on its RuntimeLoader thread, which gets the
/// platform-default stack for secondary threads — only 512KB on macOS.
/// That is far too small for the runtime: synchronous module parsing (swc)
/// alone recurses past it on real-world module graphs (e.g. a SvelteKit
/// dev server's graph) and kills the process with SIGBUS. 8MB matches the
/// headroom the CLI gets from the OS main thread.
const RUNTIME_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Run `f` to completion on a dedicated thread with a stack large enough
/// for the Ende runtime (see `RUNTIME_THREAD_STACK_SIZE`), keeping the
/// calling (loader) thread parked until it finishes.
fn run_on_runtime_thread<F: FnOnce() + Send + 'static>(f: F) {
  let thread = std::thread::Builder::new()
    .name("ende-desktop-runtime".to_string())
    .stack_size(RUNTIME_THREAD_STACK_SIZE)
    .spawn(f)
    .expect("failed to spawn desktop runtime thread");
  if thread.join().is_err() {
    // The runtime thread panicked. The panic hook normally exits the
    // process itself; this is a backstop in case it returned.
    ende_runtime::exit(1);
  }
}

/// Convert a laufey::Value to a DesktopValue for direct V8 conversion.
fn laufey_value_to_desktop_value(
  v: laufey::Value,
) -> Result<ende_runtime::ops::desktop::DesktopValue, String> {
  laufey_value_to_desktop_value_at(v, 0)
}

/// Depth-bounded body of [`laufey_value_to_desktop_value`].
///
/// The value comes from the renderer — binding arguments, or an `execute_js`
/// result — so its nesting is whatever the page sent. This conversion recurses
/// once per level, and `DesktopValue::to_v8` recurses over the result again,
/// so an unbounded value would walk the runtime thread off the end of its
/// stack. A `laufey::Value` can't be cyclic (the backend would have had to
/// resolve the cycle to build it), but depth alone is enough.
///
/// Bounded by the same `MAX_DEPTH` the `DesktopValue` deserializer uses, so
/// both directions agree on what is too deep.
fn laufey_value_to_desktop_value_at(
  v: laufey::Value,
  depth: usize,
) -> Result<ende_runtime::ops::desktop::DesktopValue, String> {
  use ende_runtime::ops::desktop::DesktopValue;
  Ok(match v {
    laufey::Value::Null => DesktopValue::Null,
    laufey::Value::Bool(b) => DesktopValue::Bool(b),
    laufey::Value::Int(i) => DesktopValue::Int(i),
    laufey::Value::Double(d) => DesktopValue::Double(d),
    laufey::Value::String(s) => DesktopValue::String(s),
    laufey::Value::List(l) => {
      let depth = nested_depth(depth)?;
      DesktopValue::List(
        l.into_iter()
          .map(|v| laufey_value_to_desktop_value_at(v, depth))
          .collect::<Result<Vec<_>, _>>()?,
      )
    }
    laufey::Value::Dict(d) => {
      let depth = nested_depth(depth)?;
      DesktopValue::Dict(
        d.into_iter()
          .map(|(k, v)| {
            laufey_value_to_desktop_value_at(v, depth).map(|v| (k, v))
          })
          .collect::<Result<Vec<_>, _>>()?,
      )
    }
    laufey::Value::Binary(b) => DesktopValue::Binary(b),
  })
}

fn nested_depth(depth: usize) -> Result<usize, String> {
  use ende_runtime::ops::desktop::MAX_DEPTH;
  match depth.checked_add(1).filter(|d| *d <= MAX_DEPTH) {
    Some(d) => Ok(d),
    None => Err(format!(
      "binding value nested deeper than {MAX_DEPTH} levels"
    )),
  }
}

/// Convert a DesktopValue back to a laufey::Value for delivery to the
/// renderer. The inverse of `laufey_value_to_desktop_value`; `Binary` maps to
/// `laufey::Value::Binary` so binding results carrying byte data arrive in
/// the webview as a `Uint8Array` (denoland/deno#36498).
fn desktop_value_to_laufey_value(
  v: ende_runtime::ops::desktop::DesktopValue,
) -> laufey::Value {
  use ende_runtime::ops::desktop::DesktopValue;
  match v {
    DesktopValue::Null => laufey::Value::Null,
    DesktopValue::Bool(b) => laufey::Value::Bool(b),
    DesktopValue::Int(i) => laufey::Value::Int(i),
    DesktopValue::Double(d) => laufey::Value::Double(d),
    DesktopValue::String(s) => laufey::Value::String(s),
    DesktopValue::List(l) => laufey::Value::List(
      l.into_iter().map(desktop_value_to_laufey_value).collect(),
    ),
    DesktopValue::Dict(d) => laufey::Value::Dict(
      d.into_iter()
        .map(|(k, v)| (k, desktop_value_to_laufey_value(v)))
        .collect(),
    ),
    DesktopValue::Binary(b) => laufey::Value::Binary(b),
  }
}

/// Find the embedded data section in this dylib (not the main executable).
fn find_section_in_dylib() -> Result<&'static [u8], AnyError> {
  match libsui::find_section_in_current_image("d3n0l4nd")
    .context("Failed reading standalone binary section from dylib.")
  {
    Ok(Some(data)) => Ok(data),
    Ok(None) => {
      bail!("Could not find standalone binary section in dylib.")
    }
    Err(err) => Err(err),
  }
}

async fn run_desktop(
  update_rolled_back: bool,
  update_trial: bool,
  mut transport: RendererTransport,
  data: endert::binary::StandaloneData,
) -> Result<(), AnyError> {
  // Make the error reporting URL available to the panic hook.
  if let Some(ref url) = data.metadata.error_reporting_url {
    ende_runtime::ops::desktop::set_error_report_config(
      url.clone(),
      data.metadata.app_version.clone(),
    );
  }

  let sys = endert::file_system::EndeRtSys::new(data.vfs.clone());

  ende_runtime::ende_telemetry::init(
    &sys,
    otel_runtime_config(),
    data.metadata.otel_config.clone(),
  )?;
  endert::init_logging(
    data.metadata.log_level,
    Some(data.metadata.otel_config.clone()),
  );
  endert::load_env_vars(&data.metadata.env_vars_from_env_file);

  // Wire up the Ende-side inspector when launched under
  // `ende desktop --inspect[-brk|-wait]`. The parent process binds the
  // user-visible port and runs a multiplexer that fronts both this
  // inspector and the CEF renderer's debug port; we just listen on the
  // internal port that the parent allocated for us.
  // Surface a malformed value loudly: previously a typoed port silently
  // disabled the inspector and the user wondered why DevTools showed
  // nothing. Bail rather than no-op so the failure is visible.
  let inspect_internal_port = match env::var(
    "ENDE_DESKTOP_INSPECT_INTERNAL_PORT",
  ) {
    Ok(s) => match s.parse::<std::net::SocketAddr>() {
      Ok(addr) => Some(addr),
      Err(e) => {
        bail!(
          "ENDE_DESKTOP_INSPECT_INTERNAL_PORT={s:?} is not a valid SocketAddr: {e}"
        );
      }
    },
    Err(_) => None,
  };
  let inspect_brk = env::var("ENDE_DESKTOP_INSPECT_BRK").is_ok();
  let inspect_wait = env::var("ENDE_DESKTOP_INSPECT_WAIT").is_ok();
  if let Some(addr) = inspect_internal_port {
    ende_runtime::ende_inspector_server::create_inspector_server(
      addr,
      "ende-desktop",
      // Don't print the ws:// URL ourselves — DevTools attaches via the
      // parent mux's user-visible port.
      ende_runtime::ende_inspector_server::InspectPublishUid {
        console: false,
        http: true,
      },
    )?;
    log::debug!("[desktop] inspector server bound on {addr}");
  }

  // ENDE_SERVE_ADDRESS (and ENDE_DESKTOP_WS_URL) are published by
  // `runtime_main` from `transport`, before any of the app's code runs.
  let (serve_port, serve_host) = match &transport {
    RendererTransport::AppScheme { .. } => (None, None),
    RendererTransport::Loopback { port } => {
      (Some(*port), Some("127.0.0.1".to_string()))
    }
  };
  let mut bridge = None;
  if let RendererTransport::AppScheme { origin, ws_relay } = &mut transport {
    // WKWebView keeps no cookies for the scheme, so the bridge keeps them.
    let cookies = cfg!(target_os = "macos").then(|| {
      open_cookie_jar(origin.host(), data.metadata.app_identifier.as_deref())
    });
    // Before the first window is created (in `op_state_init` below): WebKit
    // reads a web view's scheme handlers when the web view is created.
    bridge = Some(scheme_bridge::register(origin.clone(), cookies.clone()));
    if let Some(listener) = ws_relay.take() {
      scheme_bridge::spawn_ws_loopback_proxy(listener, origin.clone(), cookies);
    }
  }

  // laufey 0.7.0's WebView2 host can drop the `app` scheme for a window
  // (see `LostSchemeGuard`); show why instead of a blank window.
  let lost_scheme_guard = bridge
    .clone()
    .filter(|_| cfg!(windows))
    .map(|bridge| Arc::new(scheme_bridge::LostSchemeGuard::new(bridge)));

  // Enable HMR if ENDE_DESKTOP_HMR is set to a directory path
  // (set by `ende compile --desktop --hmr`).
  let hmr_watch_dir = env::var("ENDE_DESKTOP_HMR").ok().map(PathBuf::from);

  // Shared initial window ID for navigate_fut and HMR reload.
  let initial_window_id = Arc::new(AtomicU32::new(0));
  let initial_window_id_for_navigate = initial_window_id.clone();
  // Set when the initial window's first load has finished, for the navigate
  // fallback.
  let initial_window_loaded =
    Arc::new(std::sync::atomic::AtomicBool::new(false));
  let initial_window_loaded_for_api = initial_window_loaded.clone();

  // Track every live window so HMR can refresh secondary windows too,
  // not just the initial one. The same Arc is handed to WefDesktopApi
  // below; create_window / close_window keep it in sync.
  let open_windows: Arc<Mutex<HashSet<u32>>> =
    Arc::new(Mutex::new(HashSet::new()));
  let open_windows_for_api = open_windows.clone();
  let open_windows_for_hmr = open_windows.clone();

  // Windows the app hid via `hide` / `{ visible: false }`. Shared with the
  // WefDesktopApi below and the navigate fallback so neither the deferred
  // first-paint reveal nor the fallback show override an app-hidden window.
  let hidden_by_app: Arc<Mutex<HashSet<u32>>> =
    Arc::new(Mutex::new(HashSet::new()));
  let hidden_by_app_for_api = hidden_by_app.clone();
  let hidden_by_app_for_navigate = hidden_by_app.clone();

  let hmr_on_reload: Option<endert::hmr::HmrReloadCallback> =
    if hmr_watch_dir.is_some() {
      Some(Box::new(move || {
        let ids: Vec<u32> = open_windows_for_hmr
          .lock()
          .unwrap()
          .iter()
          .copied()
          .collect();
        for id in ids {
          laufey::Window::from_id(id)
            .execute_js::<fn(Result<laufey::Value, laufey::Value>)>(
              "location.reload()",
              None,
            );
        }
      }))
    } else {
      None
    };

  // Desktop extension: provides Ende.desktop.* APIs + auto-update
  #[cfg(unix)]
  let auto_update_state = get_dylib_path().map(|p| {
    endert::desktop::AutoUpdateState {
      update_trial: update_trial.then(|| endert::desktop::UpdateTrial::new(&p)),
      dylib_path: p,
      app_version: data.metadata.app_version.clone(),
      rolled_back: update_rolled_back, // from runtime_main's startup check
    }
  });
  #[cfg(not(unix))]
  let auto_update_state: Option<endert::desktop::AutoUpdateState> = {
    // Auto-update rollback is only wired up on unix; consume the flags so the
    // parameters aren't flagged as unused on other platforms.
    let _ = (update_rolled_back, update_trial);
    None
  };

  let auto_update_version = auto_update_state
    .as_ref()
    .and_then(|s| s.app_version.clone());
  let auto_update_rolled_back =
    auto_update_state.as_ref().is_some_and(|s| s.rolled_back);

  // App name (ende.json `desktop.app.name`, baked in at compile time) used as
  // the default window title. Moved into the op_state_init closure below.
  let app_name = data.metadata.app_name.clone();
  let initial_window = data.metadata.initial_window;
  let notifications_supported = notifications_supported(
    data.metadata.desktop_backend.as_deref(),
    env::consts::OS,
  );
  let backend = data.metadata.desktop_backend.clone();

  let run_opts = RunOptions {
    auto_serve: true,
    serve_port,
    serve_host,
    hmr_watch_dir,
    hmr_on_reload,
    op_state_init: Some(Box::new(move |state| {
      let (event_tx, event_rx) =
        endert::desktop::create_desktop_event_channel();
      let pending_responses = endert::desktop::PendingBindResponses::new();
      let closed_windows = Arc::new(Mutex::new(HashSet::new()));
      let surface_windows = Arc::new(Mutex::new(HashSet::new()));
      let close_requests = {
        let closed_windows = closed_windows.clone();
        let open_windows = open_windows_for_api.clone();
        let surface_windows = surface_windows.clone();
        let hidden_by_app = hidden_by_app_for_api.clone();
        CloseRequests::new(CLOSE_ANSWER_TIMEOUT, move |window_id| {
          let window = laufey::Window::from_id(window_id);
          match time_out_close(
            &closed_windows,
            &open_windows,
            &surface_windows,
            &hidden_by_app,
            window_id,
          ) {
            TimedOutClose::Destroy => window.close(),
            TimedOutClose::Hide => window.hide(),
          }
        })
      };
      let api = WefDesktopApi {
        event_tx: event_tx.clone(),
        pending_responses: pending_responses.clone(),
        closed_windows,
        surface_windows,
        close_requests,
        open_windows: open_windows_for_api.clone(),
        hidden_by_app: hidden_by_app_for_api.clone(),
        trays: Arc::new(Mutex::new(HashMap::new())),
        notifications: Arc::new(Mutex::new(HashMap::new())),
        notifications_supported,
        backend: backend.clone(),
        devtools_window: Mutex::new(None),
        lost_scheme_guard,
      };

      // Forward macOS dock-reopen callbacks (clicking the dock icon while
      // no windows are visible) into the shared event channel so JS can
      // observe them as `Ende.dock` "reopen" events.
      {
        let reopen_tx = event_tx.clone();
        laufey::on_dock_reopen(move |has_visible_windows| {
          let _ = reopen_tx.send(
            ende_runtime::ops::desktop::DesktopEvent::DockReopen {
              has_visible_windows,
            },
          );
        });
      }

      // Create the initial window (hidden) and wire up event handlers. It is
      // revealed from its `on_page_load` handler once content has painted, so
      // the user never sees the empty pre-navigation frame (issue #35530).
      let window_id = api
        .create_initial_window(initial_window, initial_window_loaded_for_api);
      initial_window_id.store(window_id, Ordering::Release);

      // Title the initial window with the app name up front, so an app that
      // never constructs its own `BrowserWindow` still shows "MyApp" rather
      // than the backend's internal default (`laufey_webview`). The
      // `BrowserWindow` constructor applies the same default to any further
      // untitled windows via `DesktopAppName` in op state.
      if let Some(name) = app_name.as_deref().filter(|n| !n.is_empty()) {
        api.set_title(window_id, name);
      }

      endert::desktop::init_desktop_state(
        state,
        Box::new(api),
        auto_update_state,
      );
      state.put(event_rx);
      state.put(event_tx);
      state.put(pending_responses);
      state.put(endert::desktop::InitialWindowId(std::sync::Mutex::new(
        Some(window_id),
      )));
      state.put(ende_runtime::ops::desktop::InitialWindowSize {
        width: i32::from(initial_window.width),
        height: i32::from(initial_window.height),
      });
      if let Some(name) = app_name.filter(|n| !n.is_empty()) {
        state.put(ende_runtime::ops::desktop::DesktopAppName(name));
      }
    })),
    auto_update_version,
    auto_update_rolled_back,
    error_reporting_url: data.metadata.error_reporting_url.clone(),
    release_base_url: data.metadata.release_base_url.clone(),
    inspect_brk,
    inspect_wait,
    is_inspecting: inspect_internal_port.is_some(),
  };

  // Run the Ende runtime and Laufey event loop concurrently.
  // We spawn the runtime first, wait for the server to be ready,
  // then navigate the webview.
  let url = transport.url();
  log::debug!("[desktop] starting runtime and laufey event loop");
  let run_fut =
    endert::run::run_with_options(Arc::new(sys.clone()), sys, data, run_opts);
  let laufey_fut = laufey::run();

  // Wait for the server to be ready, then navigate the initial window.
  let wait_for_debugger = inspect_brk || inspect_wait;
  let mux_addr = env::var("ENDE_DESKTOP_MUX_WS").ok();
  let navigate_fut = async move {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    // When --inspect-wait or --inspect-brk: block until a DevTools
    // client has connected to the mux. This prevents the renderer
    // from racing ahead while the developer is still opening DevTools.
    if wait_for_debugger && let Some(ref mux) = mux_addr {
      log::info!("[desktop] Waiting for debugger to attach on ws://{mux} …");
      loop {
        if let Ok(mut stream) =
          tokio::net::TcpStream::connect(mux.as_str()).await
        {
          let req = format!(
            "GET /debugger-attached HTTP/1.1\r\nHost: {mux}\r\nConnection: close\r\n\r\n",
          );
          if stream.write_all(req.as_bytes()).await.is_ok() {
            let mut buf = vec![0u8; 128];
            if let Ok(n) = stream.read(&mut buf).await {
              let resp = String::from_utf8_lossy(&buf[..n]);
              if resp.starts_with("HTTP/1.1 200")
                || resp.starts_with("HTTP/1.0 200")
              {
                log::info!("[desktop] Debugger attached");
                break;
              }
            }
          }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
      }
    }

    let id = initial_window_id_for_navigate.load(Ordering::Acquire);
    let mut server_ready = false;
    for i in 0..60 {
      if transport.server_ready().await {
        log::debug!(
          "[desktop] Server ready after {} attempts, navigating to {}",
          i + 1,
          &url
        );
        server_ready = true;
        break;
      }
      tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    if !server_ready {
      log::warn!("Server not ready after 15s, navigating anyway");
    }
    laufey::Window::from_id(id).navigate(&url);

    // The window was created hidden and is normally revealed from its
    // `on_page_load` handler the moment content paints. If that load never
    // finishes (e.g. the initial navigation errors), fall back to showing it
    // anyway so the user isn't left with no window at all; see
    // `navigate_fallback_shows`.
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    if navigate_fallback_shows(
      initial_window_loaded.load(Ordering::Acquire),
      hidden_by_app_for_navigate.lock().unwrap().contains(&id),
    ) {
      laufey::Window::from_id(id).show();
    }
    if let Some(bridge) = &bridge
      && !bridge.received_a_request()
    {
      log::error!(
        "[desktop] the webview made no request to {url} in 10 s; if the window stays blank, the webview couldn't register the {}:// scheme",
        scheme_bridge::APP_SCHEME
      );
    }
  };

  // Hold the JoinHandle so we can abort it when the runtime / Laufey
  // event loop ends — otherwise the navigate poll keeps trying for up
  // to 15s after window close, writing warnings to a stderr that may
  // already be torn down.
  let navigate_handle = tokio::spawn(navigate_fut);

  tokio::select! {
    result = run_fut => {
      match result {
        Ok(exit_code) => {
          log::debug!("[desktop] Ende runtime exited with code {}", exit_code);
        }
        Err(err) => {
          log::error!("[desktop] Ende runtime error: {:?}", err);
          navigate_handle.abort();
          return Err(err);
        }
      }
    }
    _ = laufey_fut => {
      log::debug!("[desktop] Laufey event loop ended (window closed)");
    }
  }
  navigate_handle.abort();

  Ok(())
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;
  use std::collections::HashSet;
  use std::sync::Arc;
  use std::sync::Mutex;
  use std::time::Duration;
  use std::time::Instant;

  use ende_runtime::ops::desktop::PermissionState;

  use super::CloseRequests;
  use super::close_tracked_notification;
  use super::desktop_menu_item_to_laufey_menu_item;
  use super::desktop_value_to_laufey_value;
  use super::laufey_value_to_desktop_value;
  use super::map_permission_status;
  use super::run_on_ui_thread;
  use super::should_show_native_error_dialog;
  use super::untrack_notification;

  #[test]
  fn dev_runs_are_told_by_their_variables() {
    use super::is_dev_run;
    assert!(!is_dev_run(|_| false));
    assert!(is_dev_run(|name| name == "ENDE_DESKTOP_HMR"));
    // `ende desktop --inspect` (and --inspect-brk / --inspect-wait) without
    // --hmr.
    assert!(is_dev_run(
      |name| name == "ENDE_DESKTOP_INSPECT_INTERNAL_PORT"
    ));
  }

  // --- default_webview2_user_data_folder ---

  mod webview2_user_data_folder {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::super::default_webview2_user_data_folder;
    use super::super::is_usable_folder_name;

    fn default_for(identifier: &str) -> Option<PathBuf> {
      default_webview2_user_data_folder(
        None,
        std::iter::empty(),
        Some(OsStr::new("LocalAppData")),
        Some(identifier),
      )
    }

    #[test]
    fn defaults_to_a_folder_per_identifier() {
      assert_eq!(
        default_for("com.example.app"),
        Some(
          PathBuf::from("LocalAppData")
            .join("com.example.app")
            .join("WebView2")
        )
      );
    }

    #[test]
    fn keeps_a_value_the_user_set() {
      let local = Some(OsStr::new("LocalAppData"));
      let id = Some("com.example.app");
      assert_eq!(
        default_webview2_user_data_folder(
          Some(OsStr::new("D:\\wv2")),
          std::iter::empty(),
          local,
          id,
        ),
        None
      );
      // The app's embedded env file sets it after this runs, and
      // `load_env_vars` never overrides a variable that is already set.
      // Windows variable names ignore case.
      assert_eq!(
        default_webview2_user_data_folder(
          None,
          ["OTHER", "webview2_user_data_folder"].into_iter(),
          local,
          id,
        ),
        None
      );
      // An empty value counts as unset.
      assert!(
        default_webview2_user_data_folder(
          Some(OsStr::new("")),
          std::iter::empty(),
          local,
          id,
        )
        .is_some()
      );
    }

    #[test]
    fn needs_local_app_data_and_an_identifier() {
      let id = Some("com.example.app");
      assert_eq!(
        default_webview2_user_data_folder(None, std::iter::empty(), None, id),
        None
      );
      assert_eq!(
        default_webview2_user_data_folder(
          None,
          std::iter::empty(),
          Some(OsStr::new("")),
          id,
        ),
        None
      );
      assert_eq!(
        default_webview2_user_data_folder(
          None,
          std::iter::empty(),
          Some(OsStr::new("LocalAppData")),
          None,
        ),
        None
      );
    }

    #[test]
    fn rejects_identifiers_that_are_not_one_folder_name() {
      for id in [
        "sh.ende.desktop.my-app",
        "com.Example.App2",
        "console.app",
        "com10.app",
        "lpt.app",
      ] {
        assert!(is_usable_folder_name(id), "{id:?} should be usable");
      }
      let long = "a".repeat(156);
      for id in [
        "",
        ".hidden",
        "trailing.",
        "..",
        "a/b",
        "a\\b",
        "a:b",
        "a b",
        "a_b",
        "caf\u{e9}.app",
        "CON.app",
        "nul",
        "aux.example",
        "com1.example",
        "LPT9.x",
        long.as_str(),
      ] {
        assert!(!is_usable_folder_name(id), "{id:?} should be rejected");
        assert_eq!(default_for(id), None);
      }
    }
  }

  // --- should_show_native_error_dialog ---
  //
  // See `should_show_native_error_dialog` / `DesktopStartupError` for the
  // rationale. In short: startup failures must be surfaced (the app never got
  // to report them), while post-load JS errors are left to the app's own
  // listeners so we don't show a duplicate dialog.

  #[test]
  fn startup_js_error_is_surfaced() {
    // A failed import / link error / top-level throw during the load phase
    // is tagged as a startup error and never reaches the app's listeners,
    // so the shell must show the dialog itself.
    assert!(should_show_native_error_dialog(true, false));
  }

  #[test]
  fn post_load_js_error_is_left_to_the_app() {
    // An uncaught error after the app loaded is reported by the app's own
    // error/unhandledrejection listeners: a second dialog here would just
    // duplicate it.
    assert!(!should_show_native_error_dialog(false, true));
  }

  #[test]
  fn non_js_crash_is_surfaced() {
    // A non-JS runtime crash has no JS listener to report it.
    assert!(should_show_native_error_dialog(false, false));
  }

  // --- app_scheme_host / RendererTransport ---

  #[test]
  fn app_scheme_is_only_for_the_webview_on_macos_and_windows() {
    use super::app_scheme_host;
    let host = Some("com.example.app");
    assert_eq!(app_scheme_host(Some("webview"), "macos", host), host);
    assert_eq!(app_scheme_host(Some("webview"), "windows", host), host);
    // No host: the loopback port.
    assert_eq!(app_scheme_host(Some("webview"), "macos", None), None);
    assert_eq!(app_scheme_host(Some("webview"), "windows", None), None);
    // The Linux webview drops request bodies; CEF and raw keep loopback;
    // so does an app whose metadata names no backend.
    for (backend, os) in [
      (Some("webview"), "linux"),
      (Some("cef"), "macos"),
      (Some("cef"), "windows"),
      (Some("cef"), "linux"),
      (Some("raw"), "macos"),
      (Some("raw"), "windows"),
      (Some("raw"), "linux"),
      (Some("WebView"), "macos"),
      (None, "macos"),
      (None, "windows"),
    ] {
      assert_eq!(app_scheme_host(backend, os, host), None, "{backend:?} {os}");
    }
  }

  #[test]
  fn navigate_fallback_shows_only_a_window_that_never_loaded() {
    use super::navigate_fallback_shows;
    assert!(navigate_fallback_shows(false, false));
    // Its first load revealed it (or the app hid it before): a second show()
    // replays the open animation.
    assert!(!navigate_fallback_shows(true, false));
    assert!(!navigate_fallback_shows(false, true));
    assert!(!navigate_fallback_shows(true, true));
  }

  #[test]
  fn native_window_error_points_other_backends_to_raw() {
    use super::no_native_window_error;
    for backend in ["webview", "cef"] {
      let message = no_native_window_error(Some(backend));
      assert!(
        message.contains(&format!("the {backend} backend")),
        "{message}"
      );
      assert!(message.contains(r#""backend": "raw""#), "{message}");
      assert!(message.contains("--backend raw"), "{message}");
    }
    // An app built before the backend was baked in ran the default one.
    assert!(no_native_window_error(None).contains("the webview backend"));
    assert_eq!(
      no_native_window_error(Some("raw")),
      "Laufey returned no native handle for this window"
    );
  }

  #[test]
  fn raw_backend_on_macos_has_no_notifications() {
    use super::notifications_supported;
    // laufey 0.7.0's raw backend hangs the JS thread for good in
    // `new Notification()` on macOS.
    assert!(!notifications_supported(Some("raw"), "macos"));
    for (backend, os) in [
      (Some("raw"), "windows"),
      (Some("raw"), "linux"),
      (Some("webview"), "macos"),
      (Some("cef"), "macos"),
      (None, "macos"),
    ] {
      assert!(notifications_supported(backend, os), "{backend:?} {os}");
    }
  }

  #[test]
  fn renderer_transport_addresses() {
    use super::RendererTransport;
    let app = RendererTransport::AppScheme {
      origin: super::scheme_bridge::AppOrigin::new("com.example.app"),
      ws_relay: None,
    };
    assert_eq!(app.serve_address(), "memory:com.example.app");
    assert_eq!(app.url(), "app://com.example.app/");
    let loopback = RendererTransport::Loopback { port: 4321 };
    assert_eq!(loopback.serve_address(), "tcp:127.0.0.1:4321");
    assert_eq!(loopback.url(), "http://127.0.0.1:4321");
  }

  #[tokio::test]
  async fn answers_get_root_wants_a_2xx_or_3xx() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    for (answer, ready) in [
      (&b"HTTP/1.1 200 OK\r\n\r\n"[..], true),
      (b"HTTP/1.1 302 Found\r\n\r\n", true),
      (b"HTTP/1.0 204 No Content\r\n\r\n", true),
      (b"HTTP/1.1 404 Not Found\r\n\r\n", false),
      (b"HTTP/1.1 503 Service Unavailable\r\n\r\n", false),
      (b"", false),
    ] {
      let (client, mut server) = tokio::io::duplex(1024);
      let app = tokio::spawn(async move {
        let mut buf = vec![0u8; 1024];
        let n = server.read(&mut buf).await.unwrap();
        server.write_all(answer).await.unwrap();
        String::from_utf8(buf[..n].to_vec()).unwrap()
      });
      assert_eq!(
        super::answers_get_root(client, "com.example.app").await,
        ready
      );
      assert_eq!(
        app.await.unwrap(),
        "GET / HTTP/1.1\r\nHost: com.example.app\r\nConnection: close\r\n\r\n"
      );
    }
  }

  #[tokio::test]
  async fn app_scheme_readiness_asks_the_memory_listener() {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    let transport = super::RendererTransport::AppScheme {
      origin: super::scheme_bridge::AppOrigin::new("readiness.example"),
      ws_relay: None,
    };
    // Nothing listens yet.
    assert!(!transport.server_ready().await);
    let listener =
      ende_runtime::ende_net::memory::listen_memory("readiness.example")
        .unwrap();
    tokio::spawn(async move {
      let (mut stream, _) = listener.accept().await.unwrap();
      let mut buf = vec![0u8; 1024];
      let _ = stream.read(&mut buf).await.unwrap();
      stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
    });
    assert!(transport.server_ready().await);
  }

  // --- map_permission_status ---

  #[test]
  fn permission_status_maps_one_to_one() {
    // Every laufey PermissionStatus must round-trip to the matching
    // Ende PermissionState. Off-by-one swaps here would surface in JS
    // as "granted shows up as denied" — silent and very confusing.
    assert!(matches!(
      map_permission_status(laufey::PermissionStatus::Granted),
      PermissionState::Granted
    ));
    assert!(matches!(
      map_permission_status(laufey::PermissionStatus::Denied),
      PermissionState::Denied
    ));
    assert!(matches!(
      map_permission_status(laufey::PermissionStatus::Prompt),
      PermissionState::Prompt
    ));
    assert!(matches!(
      map_permission_status(laufey::PermissionStatus::Unsupported),
      PermissionState::Unsupported
    ));
  }

  // --- desktop_menu_item_to_laufey_menu_item ---

  #[test]
  fn menu_item_conversion_preserves_fields() {
    let item = endert::desktop::MenuItem::Item {
      label: "Save".into(),
      id: Some("file.save".into()),
      accelerator: Some("CmdOrCtrl+S".into()),
      enabled: true,
      checked: true,
      icon: Some(vec![0x89, b'P', b'N', b'G']),
      tooltip: Some("Save the current file".into()),
    };
    match desktop_menu_item_to_laufey_menu_item(item) {
      laufey::MenuItem::Item {
        label,
        id,
        accelerator,
        enabled,
        checked,
        icon,
        tooltip,
      } => {
        assert_eq!(label, "Save");
        assert_eq!(id.as_deref(), Some("file.save"));
        assert_eq!(accelerator.as_deref(), Some("CmdOrCtrl+S"));
        assert!(enabled);
        assert!(checked);
        assert_eq!(icon.as_deref(), Some(&[0x89, b'P', b'N', b'G'][..]));
        assert_eq!(tooltip.as_deref(), Some("Save the current file"));
      }
      _ => panic!("expected Item"),
    }
  }

  #[test]
  fn menu_item_conversion_recurses_into_submenus() {
    let item = endert::desktop::MenuItem::Submenu {
      label: "File".into(),
      items: vec![
        endert::desktop::MenuItem::Item {
          label: "Open".into(),
          id: Some("open".into()),
          accelerator: None,
          enabled: false,
          checked: false,
          icon: None,
          tooltip: None,
        },
        endert::desktop::MenuItem::Separator,
        endert::desktop::MenuItem::Role {
          role: "quit".into(),
        },
      ],
    };
    let converted = desktop_menu_item_to_laufey_menu_item(item);
    let laufey::MenuItem::Submenu { items, .. } = converted else {
      panic!("expected Submenu");
    };
    assert_eq!(items.len(), 3);
    // First child is an Item with enabled=false preserved.
    match &items[0] {
      laufey::MenuItem::Item { enabled, label, .. } => {
        assert_eq!(label, "Open");
        assert!(!enabled, "enabled=false must propagate through recursion");
      }
      _ => panic!("first child should be Item"),
    }
    assert!(matches!(items[1], laufey::MenuItem::Separator));
    match &items[2] {
      laufey::MenuItem::Role { role } => assert_eq!(role, "quit"),
      _ => panic!("third child should be Role"),
    }
  }

  // --- laufey_value_to_desktop_value / laufey_value_to_json / json_to_laufey_value ---

  #[test]
  fn laufey_value_to_desktop_value_covers_every_variant() {
    use ende_runtime::ops::desktop::DesktopValue;
    let conv = |v| laufey_value_to_desktop_value(v).unwrap();
    assert!(matches!(conv(laufey::Value::Null), DesktopValue::Null));
    assert!(matches!(
      conv(laufey::Value::Bool(true)),
      DesktopValue::Bool(true)
    ));
    assert!(matches!(conv(laufey::Value::Int(7)), DesktopValue::Int(7)));
    assert!(matches!(
      conv(laufey::Value::Double(1.5)),
      DesktopValue::Double(d) if d == 1.5
    ));
    match conv(laufey::Value::String("hi".into())) {
      DesktopValue::String(s) => assert_eq!(s, "hi"),
      _ => panic!(),
    }
    match conv(laufey::Value::Binary(vec![1, 2, 3])) {
      DesktopValue::Binary(b) => assert_eq!(b, vec![1, 2, 3]),
      _ => panic!(),
    }
  }

  #[test]
  fn laufey_value_to_desktop_value_recurses() {
    use ende_runtime::ops::desktop::DesktopValue;
    let v = laufey::Value::List(vec![
      laufey::Value::Int(1),
      laufey::Value::List(vec![laufey::Value::String("nested".into())]),
    ]);
    let dv = laufey_value_to_desktop_value(v).unwrap();
    let DesktopValue::List(outer) = dv else {
      panic!("outer must be List")
    };
    assert_eq!(outer.len(), 2);
    let DesktopValue::List(inner) = &outer[1] else {
      panic!("second element must be a nested List")
    };
    match &inner[0] {
      DesktopValue::String(s) => assert_eq!(s, "nested"),
      _ => panic!("inner element should be String"),
    }
  }

  #[test]
  fn laufey_value_to_desktop_value_bounds_renderer_depth() {
    use ende_runtime::ops::desktop::MAX_DEPTH;
    // Binding arguments come from the renderer, so their nesting is whatever
    // the page sent. This conversion recurses per level and `to_v8` recurses
    // over the result again, so an unbounded value would walk the runtime
    // thread off its stack. The deserializer's MAX_DEPTH doesn't cover this
    // direction — it never sees these values.
    fn nest(depth: usize) -> laufey::Value {
      let mut v = laufey::Value::Int(1);
      for _ in 0..depth {
        v = laufey::Value::List(vec![v]);
      }
      v
    }

    let err = laufey_value_to_desktop_value(nest(MAX_DEPTH + 1)).unwrap_err();
    assert!(err.contains("nested deeper than"), "got: {err}");

    // Objects nest through a separate arm, so they need their own guard.
    let mut v = laufey::Value::Int(1);
    for _ in 0..MAX_DEPTH + 1 {
      let mut dict = std::collections::HashMap::new();
      dict.insert("a".to_string(), v);
      v = laufey::Value::Dict(dict);
    }
    let err = laufey_value_to_desktop_value(v).unwrap_err();
    assert!(err.contains("nested deeper than"), "got: {err}");

    // At the limit it still converts, so the guard isn't just rejecting
    // anything structured.
    laufey_value_to_desktop_value(nest(MAX_DEPTH))
      .expect("nesting within the limit must still convert");
  }

  #[test]
  fn laufey_desktop_value_roundtrip() {
    use std::collections::HashMap;
    let mut dict = HashMap::new();
    dict.insert("name".to_string(), laufey::Value::String("ada".into()));
    dict.insert("count".to_string(), laufey::Value::Int(42));
    dict.insert("ok".to_string(), laufey::Value::Bool(true));
    dict.insert("bin".to_string(), laufey::Value::Binary(vec![1, 2, 255]));
    let v = laufey::Value::Dict(dict);

    // laufey → DesktopValue → laufey must preserve every variant —
    // including Binary, which the old serde_json transport dropped
    // (denoland/deno#36498).
    let back =
      desktop_value_to_laufey_value(laufey_value_to_desktop_value(v).unwrap());
    let laufey::Value::Dict(d) = back else {
      panic!("round-trip must yield Dict")
    };
    match d.get("name") {
      Some(laufey::Value::String(s)) => assert_eq!(s, "ada"),
      _ => panic!("name must round-trip as String"),
    }
    match d.get("count") {
      Some(laufey::Value::Int(42)) => {}
      _ => panic!("count must round-trip as Int(42)"),
    }
    match d.get("ok") {
      Some(laufey::Value::Bool(true)) => {}
      _ => panic!("ok must round-trip as Bool(true)"),
    }
    match d.get("bin") {
      Some(laufey::Value::Binary(b)) => assert_eq!(b, &vec![1, 2, 255]),
      _ => panic!("bin must round-trip as Binary"),
    }
  }

  // --- apply_pending_update ---
  //
  // State machine: which of `<dylib>.update`, `.backup`, `.update-trial` and
  // the `.update-ok` sentinel exist decides what apply_pending_update does
  // (see `UpdateLaunch`). Getting it wrong is serious: booting a half-applied
  // dylib, adopting an update that never ran, or boot-looping on rollback.
  // Tests below exercise each branch and the launch sequences, using a
  // tempdir as the live dylib location.

  // apply_pending_update is unix-only (see its #[cfg(unix)] gate), so its
  // tests and helpers live in a unix-only submodule.
  #[cfg(unix)]
  mod pending_update_tests {
    use super::super::UpdateLaunch;
    use super::super::apply_pending_update;

    fn touch(path: &std::path::Path, content: &str) {
      std::fs::write(path, content).unwrap();
    }

    fn read(path: &std::path::Path) -> String {
      std::fs::read_to_string(path).unwrap()
    }

    struct Paths {
      dylib: std::path::PathBuf,
      update: std::path::PathBuf,
      backup: std::path::PathBuf,
      trial: std::path::PathBuf,
      sentinel: std::path::PathBuf,
      rejected: std::path::PathBuf,
    }

    fn paths(tmp: &std::path::Path) -> Paths {
      Paths {
        dylib: tmp.join("app.dylib"),
        update: tmp.join("app.dylib.update"),
        backup: tmp.join("app.dylib.backup"),
        trial: tmp.join("app.dylib.update-trial"),
        sentinel: tmp.join("app.dylib.update-ok"),
        rejected: tmp.join("app.dylib.update-rejected"),
      }
    }

    /// A normal launch (not re-exec'd) that has ended.
    fn launch(p: &Paths) -> UpdateLaunch {
      apply_pending_update(&p.dylib, false).0
    }

    /// The launch `restart_host` starts, once it has ended.
    fn reexeced_launch(p: &Paths) -> UpdateLaunch {
      apply_pending_update(&p.dylib, true).0
    }

    /// What `op_desktop_confirm_update` does in a trial launch.
    fn confirm(p: &Paths) {
      touch(&p.sentinel, "ok");
    }

    #[test]
    fn pending_update_no_files_is_noop() {
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "live");
      assert_eq!(launch(&p), UpdateLaunch::Normal);
      assert_eq!(read(&p.dylib), "live", "dylib must be untouched");
      assert!(!p.trial.exists());
    }

    #[test]
    fn pending_update_swaps_in_new_dylib_when_update_exists() {
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "old");
      touch(&p.update, "new");

      assert_eq!(launch(&p), UpdateLaunch::Swapped);
      assert_eq!(
        read(&p.dylib),
        "new",
        "dylib must now hold the update contents"
      );
      assert!(p.backup.exists(), "live dylib must be preserved as .backup");
      assert_eq!(
        read(&p.backup),
        "old",
        "backup must be the *previous* dylib"
      );
      // Nothing marks the update as tried or confirmed yet.
      assert!(!p.sentinel.exists());
      assert!(!p.trial.exists());
      // The staged update file is consumed.
      assert!(!p.update.exists());
    }

    #[test]
    fn pending_update_clears_stale_state_before_swap() {
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "old");
      touch(&p.update, "new");
      touch(&p.sentinel, "ok"); // sentinel left over from a previous launch
      touch(&p.trial, ""); // and a trial marker
      touch(&p.backup, "very-old"); // and a backup, also stale
      assert_eq!(launch(&p), UpdateLaunch::Swapped);
      // After applying a new update: backup is the just-replaced 'old',
      // sentinel and trial marker were cleared. The previous backup
      // ("very-old") must not persist, or rollback would resurrect a
      // two-version-old dylib.
      assert_eq!(read(&p.backup), "old");
      assert!(!p.sentinel.exists());
      assert!(!p.trial.exists());
    }

    #[test]
    fn pending_update_reexeced_launch_never_swaps() {
      // The guard against a re-exec loop: even if an `.update` is there, the
      // re-exec'd process runs the dylib it was started for.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "new");
      touch(&p.backup, "old");
      touch(&p.update, "newer");
      assert_eq!(reexeced_launch(&p), UpdateLaunch::Trial);
      assert_eq!(read(&p.dylib), "new");
      assert_eq!(read(&p.backup), "old");
      assert_eq!(read(&p.update), "newer", "left for the next launch");
    }

    #[test]
    fn pending_update_first_launch_after_swap_is_the_trial() {
      // .backup without sentinel or trial marker = the swapped-in dylib
      // hasn't run yet. This launch runs it, and records that it did.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "new");
      touch(&p.backup, "old");
      assert_eq!(launch(&p), UpdateLaunch::Trial);
      assert!(p.trial.exists(), "the trial launch must be recorded");
      assert_eq!(read(&p.dylib), "new", "the new dylib runs");
      assert_eq!(read(&p.backup), "old", "backup kept for a rollback");
    }

    #[test]
    fn pending_update_launch_during_the_trial_does_not_roll_back() {
      // Two instances started inside one trial window (a terminal, `open -n`
      // or Linux start a second one): the second must not take the running
      // trial for a crashed one and roll a good update back.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "new");
      touch(&p.backup, "old");
      let (trial, lock) = apply_pending_update(&p.dylib, true);
      assert_eq!(trial, UpdateLaunch::Trial);
      assert_eq!(launch(&p), UpdateLaunch::Normal);
      assert_eq!(read(&p.dylib), "new", "the update stays");
      assert_eq!(read(&p.backup), "old");
      assert!(p.trial.exists(), "the trial is still on");
      assert!(lock.is_some(), "the trial launch holds the trial lock");
      // Once the trial launch has ended unconfirmed, it rolls back.
      drop(lock);
      assert_eq!(launch(&p), UpdateLaunch::RolledBack);
      assert_eq!(read(&p.dylib), "old");
    }

    #[test]
    fn pending_update_rolls_back_when_trial_ended_unconfirmed() {
      // .backup + trial marker + no sentinel = the trial launch crashed
      // before writing the sentinel. Rollback to backup.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "new-but-broken");
      touch(&p.backup, "old-but-known-good");
      touch(&p.trial, "");
      assert_eq!(launch(&p), UpdateLaunch::RolledBack);
      assert_eq!(
        read(&p.dylib),
        "old-but-known-good",
        "rollback must restore the backup contents over the live dylib"
      );
      assert!(!p.backup.exists());
      assert!(!p.trial.exists());
      assert_eq!(launch(&p), UpdateLaunch::Normal, "rolls back only once");
    }

    #[test]
    fn pending_update_rollback_rejects_the_failed_update() {
      // Remembered so the restored app doesn't stage the same update again
      // (see `stage_patch` in ende_runtime).
      const BROKEN_SHA256: &str =
        "f70cc4e9f6ceeb3df298b14b352258508f724fc8ef320008b5c246076333c075";
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "new-but-broken");
      touch(&p.backup, "old-but-known-good");
      touch(&p.trial, "");
      assert_eq!(launch(&p), UpdateLaunch::RolledBack);
      assert_eq!(
        read(&p.rejected).lines().collect::<Vec<_>>(),
        [BROKEN_SHA256]
      );
      // A later update the app confirms moves it past the rejected one.
      touch(&p.update, "newer");
      assert_eq!(launch(&p), UpdateLaunch::Swapped);
      assert!(p.rejected.exists(), "kept while the new update is on trial");
      assert_eq!(reexeced_launch(&p), UpdateLaunch::Trial);
      confirm(&p);
      assert_eq!(launch(&p), UpdateLaunch::Normal);
      assert!(!p.rejected.exists());
    }

    #[test]
    fn pending_update_cleans_up_after_successful_boot() {
      // .backup + .update-ok sentinel = previous update was applied AND
      // booted at least once. Clean up; we don't need to keep the backup
      // around forever.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "new");
      touch(&p.backup, "old");
      touch(&p.trial, "");
      touch(&p.sentinel, "ok");
      assert_eq!(launch(&p), UpdateLaunch::Normal);
      assert!(
        !p.backup.exists(),
        "successful-boot path must delete .backup"
      );
      assert!(
        !p.sentinel.exists(),
        "successful-boot path must delete sentinel"
      );
      assert!(!p.trial.exists());
      assert_eq!(read(&p.dylib), "new", "dylib untouched on cleanup path");
    }

    #[test]
    fn pending_update_reports_no_rollback_when_rollback_fails() {
      // A rollback that can't write must not claim it happened: the caller
      // dispatches a "rolled back" event off the return value, and the broken
      // dylib is still in place. The backup has to survive so the next launch
      // retries rather than adopting the failed update as the new baseline.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "new-but-broken");
      touch(&p.backup, "old-but-known-good");
      touch(&p.trial, "");

      // Make the dylib's directory unwritable so both the rename and the
      // staged copy fail. Skipped when that doesn't actually deny writes —
      // root ignores these bits, and this test would silently invert.
      let original = std::fs::metadata(tmp.path()).unwrap().permissions();
      let mut readonly = original.clone();
      std::os::unix::fs::PermissionsExt::set_mode(&mut readonly, 0o500);
      std::fs::set_permissions(tmp.path(), readonly).unwrap();
      let writes_denied =
        std::fs::write(tmp.path().join("probe"), b"x").is_err();

      let result = if writes_denied {
        Some(launch(&p))
      } else {
        None
      };

      // Restore before asserting, or a failure leaves an undeletable tempdir.
      std::fs::set_permissions(tmp.path(), original).unwrap();

      if let Some(result) = result {
        assert_eq!(
          result,
          UpdateLaunch::Normal,
          "a rollback that could not be written must not report success"
        );
        assert!(p.backup.exists(), "failed rollback must keep .backup");
        assert!(p.trial.exists(), "the next launch must retry the rollback");
        assert_eq!(
          read(&p.dylib),
          "new-but-broken",
          "dylib is untouched when the rollback cannot be applied"
        );
      }
    }

    #[test]
    fn pending_update_with_only_sentinel_is_noop() {
      // Sentinel without backup or update — orphaned. Don't roll back
      // (there's nothing to roll back to) and don't touch the dylib.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "live");
      touch(&p.sentinel, "ok");
      assert_eq!(launch(&p), UpdateLaunch::Normal);
      assert_eq!(read(&p.dylib), "live");
    }

    #[test]
    fn pending_update_confirmed_trial_is_kept() {
      // Swap, re-exec into the trial, confirm; the next launch keeps it.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "v1");
      touch(&p.update, "v2");
      assert_eq!(launch(&p), UpdateLaunch::Swapped);
      assert_eq!(reexeced_launch(&p), UpdateLaunch::Trial);
      confirm(&p);
      assert_eq!(launch(&p), UpdateLaunch::Normal);
      assert_eq!(read(&p.dylib), "v2");
      for leftover in [&p.update, &p.backup, &p.trial, &p.sentinel] {
        assert!(!leftover.exists(), "{} left behind", leftover.display());
      }
    }

    #[test]
    fn pending_update_crashed_trial_rolls_back_on_the_next_launch() {
      // Swap, re-exec into the trial, crash before confirming. The launch
      // after that runs the old dylib again.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "v1");
      touch(&p.update, "v2-crashes");
      assert_eq!(launch(&p), UpdateLaunch::Swapped);
      assert_eq!(reexeced_launch(&p), UpdateLaunch::Trial);
      assert_eq!(launch(&p), UpdateLaunch::RolledBack);
      assert_eq!(read(&p.dylib), "v1");
      assert_eq!(launch(&p), UpdateLaunch::Normal);
      assert_eq!(read(&p.dylib), "v1");
    }

    #[test]
    fn pending_update_failed_reexec_defers_the_trial() {
      // The re-exec failed, so the old code ran on after the swap and didn't
      // confirm. The next launch is the new dylib's trial, not a rollback.
      let tmp = tempfile::tempdir().unwrap();
      let p = paths(tmp.path());
      touch(&p.dylib, "v1");
      touch(&p.update, "v2");
      assert_eq!(launch(&p), UpdateLaunch::Swapped);
      assert_eq!(launch(&p), UpdateLaunch::Trial);
      assert_eq!(read(&p.dylib), "v2");
    }
  }

  #[cfg(unix)]
  #[test]
  fn reexec_happens_once_after_a_swap_or_a_rollback() {
    use std::ffi::OsStr;

    use super::REEXEC_AFTER_ROLLBACK;
    use super::REEXEC_AFTER_SWAP;
    use super::UpdateLaunch::*;
    use super::after_update_check;

    let swapped = Some(OsStr::new(REEXEC_AFTER_SWAP));
    let rolled_back = Some(OsStr::new(REEXEC_AFTER_ROLLBACK));
    // A normal launch re-execs after changing the dylib on disk.
    assert_eq!(
      after_update_check(Swapped, None),
      (Some(REEXEC_AFTER_SWAP), Swapped)
    );
    assert_eq!(
      after_update_check(RolledBack, None),
      (Some(REEXEC_AFTER_ROLLBACK), RolledBack)
    );
    assert_eq!(after_update_check(Normal, None), (None, Normal));
    assert_eq!(after_update_check(Trial, None), (None, Trial));
    // The re-exec'd process doesn't, whatever it finds.
    assert_eq!(after_update_check(Trial, swapped), (None, Trial));
    assert_eq!(after_update_check(Swapped, swapped), (None, Swapped));
    assert_eq!(after_update_check(RolledBack, swapped), (None, RolledBack));
    assert_eq!(
      after_update_check(RolledBack, rolled_back),
      (None, RolledBack)
    );
    // The restored code reports the rollback that restarted it.
    assert_eq!(after_update_check(Normal, rolled_back), (None, RolledBack));
  }

  /// A host executable laid out as `<dir>/<name>.app/Contents/MacOS/<exe>`,
  /// with an Info.plist if `plist`.
  #[cfg(unix)]
  fn fake_app_bundle(dir: &std::path::Path, plist: bool) -> std::path::PathBuf {
    let macos = dir.join("My App.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    if plist {
      std::fs::write(macos.parent().unwrap().join("Info.plist"), "").unwrap();
    }
    let exe = macos.join("laufey_webview");
    std::fs::write(&exe, "").unwrap();
    exe
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn restart_of_an_app_bundle_goes_through_launch_services() {
    use std::ffi::OsString;

    use super::Restart;
    use super::restart_plan;

    let tmp = tempfile::tempdir().unwrap();
    let exe = fake_app_bundle(tmp.path(), true);
    let bundle = exe.parent().unwrap().parent().unwrap().parent().unwrap();
    let args = [OsString::from("x y"), OsString::from("--flag=1")];
    let vars = [
      (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
      (OsString::from("PASS_ME"), OsString::from("a=b c")),
    ];
    let expected: Vec<OsString> = [
      "-n",
      "--env",
      "PATH=/usr/bin:/bin",
      "--env",
      "PASS_ME=a=b c",
      "--env",
      "ENDE_DESKTOP_UPDATE_REEXEC=swapped",
    ]
    .iter()
    .map(OsString::from)
    .chain([bundle.as_os_str().to_owned()])
    .chain(["--args", "x y", "--flag=1"].iter().map(OsString::from))
    .collect();
    assert_eq!(
      restart_plan(&exe, &args, &vars, "swapped"),
      Restart::LaunchServices(expected)
    );

    // No arguments: no `--args`.
    let Restart::LaunchServices(open_args) =
      restart_plan(&exe, &[], &[], "rolled-back")
    else {
      panic!("not through LaunchServices");
    };
    assert_eq!(
      open_args,
      [
        OsString::from("-n"),
        "--env".into(),
        "ENDE_DESKTOP_UPDATE_REEXEC=rolled-back".into(),
        bundle.as_os_str().to_owned(),
      ]
    );
  }

  #[cfg(target_os = "macos")]
  #[test]
  fn restart_runs_open_only_once_the_old_process_is_gone() {
    use std::ffi::OsString;

    use super::spawn_after_exit;

    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("ran");
    let alive = spawn_after_exit(
      std::path::Path::new("/usr/bin/touch"),
      &[OsString::from(&marker)],
    )
    .unwrap();
    // Still "running": the waiter holds off.
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(!marker.exists(), "ran before the old process exited");
    // Exiting closes the write end; dropping it here does the same.
    drop(alive);
    let deadline =
      std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !marker.exists() && std::time::Instant::now() < deadline {
      std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(marker.exists(), "never ran after the old process exited");
  }

  #[cfg(unix)]
  #[test]
  fn restart_outside_an_app_bundle_execs() {
    use super::Restart;
    use super::restart_plan;

    let tmp = tempfile::tempdir().unwrap();
    // Not in a bundle at all.
    let exe = tmp.path().join("laufey_webview");
    std::fs::write(&exe, "").unwrap();
    assert_eq!(restart_plan(&exe, &[], &[], "swapped"), Restart::Exec);
    // A bundle's layout without an Info.plist isn't an app LaunchServices
    // can open.
    let exe = fake_app_bundle(&tmp.path().join("noplist"), false);
    assert_eq!(restart_plan(&exe, &[], &[], "swapped"), Restart::Exec);
  }

  #[cfg(all(unix, not(target_os = "macos")))]
  #[test]
  fn restart_of_an_app_bundle_layout_execs_off_macos() {
    use super::Restart;
    use super::restart_plan;

    let tmp = tempfile::tempdir().unwrap();
    let exe = fake_app_bundle(tmp.path(), true);
    assert_eq!(restart_plan(&exe, &[], &[], "swapped"), Restart::Exec);
  }

  #[cfg(unix)]
  #[test]
  fn reexec_marks_inheritable_fds_cloexec() {
    use super::clear_cloexec;
    use super::mark_fds_cloexec_above_stderr;
    // SAFETY: plain fcntl/dup/close calls on descriptors this test owns.
    unsafe {
      let fd = libc::dup(libc::STDERR_FILENO);
      assert!(fd > libc::STDERR_FILENO);
      assert_eq!(libc::fcntl(fd, libc::F_GETFD) & libc::FD_CLOEXEC, 0);
      let stderr_flags = libc::fcntl(libc::STDERR_FILENO, libc::F_GETFD);

      let marked = mark_fds_cloexec_above_stderr();
      // Other tests run in parallel and may hold descriptors of their own,
      // so only check ours.
      let flags_marked = libc::fcntl(fd, libc::F_GETFD);
      clear_cloexec(&marked);
      let flags_restored = libc::fcntl(fd, libc::F_GETFD);
      libc::close(fd);

      assert!(marked.contains(&fd));
      assert_ne!(flags_marked & libc::FD_CLOEXEC, 0);
      assert_eq!(flags_restored & libc::FD_CLOEXEC, 0, "restored on failure");
      assert_eq!(
        libc::fcntl(libc::STDERR_FILENO, libc::F_GETFD),
        stderr_flags,
        "stdio stays inheritable"
      );
    }
  }

  #[test]
  fn desktop_value_to_laufey_handles_nested_arrays_and_objects() {
    use ende_runtime::ops::desktop::DesktopValue;
    let dv = DesktopValue::Dict(vec![
      (
        "list".into(),
        DesktopValue::List(vec![
          DesktopValue::Int(1),
          DesktopValue::Int(2),
          DesktopValue::Int(3),
        ]),
      ),
      (
        "nested".into(),
        DesktopValue::Dict(vec![(
          "key".into(),
          DesktopValue::String("value".into()),
        )]),
      ),
      ("null".into(), DesktopValue::Null),
    ]);
    let v = desktop_value_to_laufey_value(dv);
    let laufey::Value::Dict(d) = v else {
      panic!("expected Dict")
    };
    assert!(matches!(d.get("null"), Some(laufey::Value::Null)));
    match d.get("list") {
      Some(laufey::Value::List(items)) => assert_eq!(items.len(), 3),
      _ => panic!("list must convert to List"),
    }
    match d.get("nested") {
      Some(laufey::Value::Dict(inner)) => match inner.get("key") {
        Some(laufey::Value::String(s)) => assert_eq!(s, "value"),
        _ => panic!("nested.key must be String"),
      },
      _ => panic!("nested must convert to Dict"),
    }
  }

  // --- run_on_ui_thread ---

  /// An API table whose `post_ui_task` is `post_ui_task`, everything else
  /// unset.
  fn fake_api(
    post_ui_task: Option<
      unsafe extern "C" fn(
        *mut std::ffi::c_void,
        Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
        *mut std::ffi::c_void,
      ),
    >,
  ) -> laufey::LaufeyBackendApi {
    // SAFETY: a C struct of integers, pointers and `Option<fn>`s, for which
    // all-zero bytes are valid (0, null, `None`).
    let mut api: laufey::LaufeyBackendApi = unsafe { std::mem::zeroed() };
    api.post_ui_task = post_ui_task;
    api
  }

  /// Like the backends: runs the task later, on another thread.
  unsafe extern "C" fn post_to_other_thread(
    _backend_data: *mut std::ffi::c_void,
    task: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
    data: *mut std::ffi::c_void,
  ) {
    let task = task.unwrap();
    let data = data as usize;
    std::thread::spawn(move || {
      std::thread::sleep(std::time::Duration::from_millis(50));
      // SAFETY: the task and data `run_on_ui_thread` posted, run once.
      unsafe { task(data as *mut std::ffi::c_void) };
    });
  }

  fn thread_that_runs(api: Option<&laufey::LaufeyBackendApi>) -> bool {
    let caller = std::thread::current().id();
    let (tx, rx) = std::sync::mpsc::channel();
    run_on_ui_thread(
      api,
      Box::new(move || tx.send(std::thread::current().id()).unwrap()),
    );
    // `try_recv`: the task must have run by the time the call returns.
    rx.try_recv().unwrap() == caller
  }

  // Test threads aren't the process main thread, so on macOS these exercise
  // the hop.
  #[cfg(target_os = "macos")]
  #[test]
  fn run_on_ui_thread_posts_and_waits_on_macos() {
    let api = fake_api(Some(post_to_other_thread));
    assert!(!thread_that_runs(Some(&api)), "expected a hop");
  }

  #[cfg(not(target_os = "macos"))]
  #[test]
  fn run_on_ui_thread_is_inline_off_macos() {
    let api = fake_api(Some(post_to_other_thread));
    assert!(thread_that_runs(Some(&api)));
  }

  #[test]
  fn run_on_ui_thread_is_inline_without_post_ui_task() {
    assert!(thread_that_runs(None));
    assert!(thread_that_runs(Some(&fake_api(None))));
  }

  // A panicking task must not unwind into the backend's C frames (that would
  // abort the process); the caller just stops waiting.
  #[cfg(target_os = "macos")]
  #[test]
  fn run_on_ui_thread_survives_a_panicking_task() {
    let api = fake_api(Some(post_to_other_thread));
    run_on_ui_thread(Some(&api), Box::new(|| panic!("task panicked")));
  }

  // --- close_tracked_notification (#36950) ---

  #[test]
  fn notification_close_runs_with_the_map_unlocked() {
    // The backend can deliver the notification's Closed event before its
    // close returns, and that handler locks the same map: closing under the
    // lock deadlocks.
    let notifications = Mutex::new(HashMap::from([(1u32, "handle")]));
    let mut closed = None;
    close_tracked_notification(
      &notifications,
      1,
      |handle| {
        assert!(
          notifications.try_lock().is_ok(),
          "the notifications map is still locked during close"
        );
        closed = Some(*handle);
      },
      || {},
    );
    assert_eq!(closed, Some("handle"));
    assert!(notifications.lock().unwrap().is_empty());
  }

  #[test]
  fn notification_close_ignores_unknown_ids() {
    let notifications = Mutex::new(HashMap::<u32, ()>::new());
    close_tracked_notification(
      &notifications,
      9,
      |_| panic!("closed"),
      || panic!("reported a close"),
    );
  }

  #[test]
  fn notification_close_reports_one_close() {
    // laufey 0.7.0 drops the notification's event handler in `close`, so the
    // backend's Closed never arrives: Ende reports the close itself.
    let notifications = Mutex::new(HashMap::from([(1u32, "handle")]));
    let mut closes = 0;
    close_tracked_notification(
      &notifications,
      1,
      |_| {
        // A backend that does deliver Closed during `close`: the handler
        // finds the notification untracked and doesn't report it again.
        assert!(!untrack_notification(&notifications, 1));
      },
      || closes += 1,
    );
    close_tracked_notification(
      &notifications,
      1,
      |_| panic!("closed twice"),
      || closes += 1,
    );
    assert_eq!(closes, 1);
  }

  // --- time_out_close ---

  #[test]
  fn a_window_hidden_by_the_close_timeout_counts_as_closed() {
    use super::TimedOutClose;
    use super::time_out_close;
    let closed = Mutex::new(HashSet::new());
    let open = Mutex::new(HashSet::from([1, 2]));
    let surfaces = Mutex::new(HashSet::from([2]));
    let hidden = Mutex::new(HashSet::new());
    assert_eq!(
      time_out_close(&closed, &open, &surfaces, &hidden, 1),
      TimedOutClose::Destroy
    );
    // A WebGPU window is only hidden, but it's closed as far as the app is
    // concerned: `isClosed()` is true, so the `close` event JS dispatches
    // when it catches up can't be canceled, and a late listener can't keep
    // the window around hidden.
    assert_eq!(
      time_out_close(&closed, &open, &surfaces, &hidden, 2),
      TimedOutClose::Hide
    );
    assert_eq!(*closed.lock().unwrap(), HashSet::from([1, 2]));
    assert!(open.lock().unwrap().is_empty());
    assert_eq!(*hidden.lock().unwrap(), HashSet::from([2]));
  }

  // --- CloseRequests (denoland/deno#36973) ---

  type Fired = Arc<Mutex<Vec<(u32, Instant)>>>;

  fn close_requests(timeout_ms: u64) -> (Arc<CloseRequests>, Fired) {
    let fired: Fired = Arc::new(Mutex::new(Vec::new()));
    let sink = fired.clone();
    let requests =
      CloseRequests::new(Duration::from_millis(timeout_ms), move |window_id| {
        sink.lock().unwrap().push((window_id, Instant::now()))
      });
    (requests, fired)
  }

  /// Wait up to 5 s for the first timeout.
  fn first_timeout(fired: &Fired) -> Option<(u32, Instant)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
      if let Some(&first) = fired.lock().unwrap().first() {
        return Some(first);
      }
      std::thread::sleep(Duration::from_millis(10));
    }
    None
  }

  #[test]
  fn close_request_without_answer_times_out() {
    let (requests, fired) = close_requests(100);
    let start = Instant::now();
    assert!(requests.begin(7));
    let (window_id, at) = first_timeout(&fired).expect("no timeout");
    assert_eq!(window_id, 7);
    assert!(at - start >= Duration::from_millis(100));
  }

  #[test]
  fn answered_close_request_does_not_time_out() {
    let (requests, fired) = close_requests(100);
    assert!(requests.begin(7));
    requests.answer(7);
    std::thread::sleep(Duration::from_millis(400));
    assert!(fired.lock().unwrap().is_empty());
  }

  #[test]
  fn close_request_is_sent_once_until_answered() {
    let (requests, _fired) = close_requests(60_000);
    assert!(requests.begin(7));
    assert!(!requests.begin(7), "a second click while pending");
    assert!(requests.begin(8), "other windows are independent");
    requests.answer(7);
    assert!(
      requests.begin(7),
      "a click after the answer is a new request"
    );
  }

  #[test]
  fn dialog_time_does_not_count_toward_the_timeout() {
    let (requests, fired) = close_requests(500);
    assert!(requests.begin(7));
    std::thread::sleep(Duration::from_millis(100));
    let dialog = requests.dialog();
    // A request that arrives while the dialog is up waits too.
    assert!(requests.begin(8));
    std::thread::sleep(Duration::from_millis(1000));
    assert!(
      fired.lock().unwrap().is_empty(),
      "timed out inside a dialog"
    );
    let closed_at = Instant::now();
    drop(dialog);
    let (window_id, at) = first_timeout(&fired).expect("no timeout");
    assert_eq!(window_id, 7);
    // About 400 ms of window 7's budget was left when the dialog opened.
    assert!(at - closed_at >= Duration::from_millis(250));
  }

  #[test]
  fn nested_dialogs_pause_until_the_last_one_closes() {
    let (requests, fired) = close_requests(100);
    let outer = requests.dialog();
    let inner = requests.dialog();
    assert!(requests.begin(7));
    drop(inner);
    std::thread::sleep(Duration::from_millis(400));
    assert!(fired.lock().unwrap().is_empty());
    drop(outer);
    assert_eq!(first_timeout(&fired).map(|(id, _)| id), Some(7));
  }
}
