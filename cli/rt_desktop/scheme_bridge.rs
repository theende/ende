// Copyright 2018-2026 the Deno authors. MIT license.

//! Serves the webview from `app://<host>` by bridging its requests into the
//! app's in-process `Ende.serve` server.
//!
//! On the backends where [`crate::app_scheme_host`] allows it, the desktop
//! runtime serves the app over an in-process memory listener
//! (`ENDE_SERVE_ADDRESS=memory:<host>`) instead of a TCP loopback port. The
//! webview can't dial that listener, so a laufey scheme handler for `app`
//! (the only scheme laufey 0.7.0's hosts register) receives each request
//! here; the bridge opens a fresh memory connection, speaks HTTP/1.1 over it
//! with a hyper client and returns the response to the webview.
//!
//! The page so runs at a stable origin, `app://<host>`, where `<host>` is the
//! app identifier (lowercased), and origin-keyed storage (`localStorage`,
//! IndexedDB) stays in place across launches (denoland/deno#35444). The bridge
//! serves exactly that origin: a request for another host on the scheme is
//! answered with `421 Misdirected Request`.
//!
//! # WebSockets
//!
//! Webviews route `ws://` through their network stack, never through a scheme
//! handler, and laufey's scheme exchange is one-shot HTTP. So the runtime
//! also binds a narrow WebSocket relay on a random loopback port
//! ([`spawn_ws_loopback_proxy`]) that forwards upgrades into the memory
//! listener. Any local process, any user's, and any page in any browser on
//! the machine can reach that port, so the relay admits an upgrade only when
//!
//! * its request target starts with `/.ende-desktop-relay/<token>`
//!   ([`RELAY_PATH_PREFIX`]), where `<token>` is a 256-bit secret drawn once
//!   per launch ([`relay_token`]). The app learns it from [`WS_URL_ENV`],
//!   which only the runtime sees (child processes don't inherit it, and it is
//!   in no process environment or argv), and hands it to its page;
//! * and its `Origin` header is exactly `app://<host>`. A page elsewhere can't
//!   forge that header, so this also closes cross-site WebSocket hijacking and
//!   DNS rebinding (a rebound page still sends its own origin).
//!
//! The relay strips the prefix and token, so `…/<token>/ws?x=1` reaches the
//! app's server as `GET /ws?x=1`. It forwards only the client's request head
//! (never bytes pipelined after it), then reads the server's response head:
//! anything but `101 Switching Protocols` is replaced with the relay's own
//! `502` and the connection is closed, so the relay never carries a plain
//! HTTP response or a second request. A request that isn't a WebSocket
//! upgrade is answered `400`, and a client that doesn't send a complete head
//! within [`HEAD_READ_TIMEOUT`] is dropped. The relay is not a boundary
//! against native code running as the same user, which can read the app's
//! memory.
//!
//! # What the `Ende.serve` handler sees
//!
//! Every request from the bridge or the relay arrives on the memory
//! transport: `info.remoteAddr.transport === "memory"`, and `request.url` is
//! `http://<host>/…` (the bridge and the relay both send `Host: <host>`). Gate
//! desktop-only endpoints on the transport. On that transport:
//!
//! * a request carrying [`RELAY_MARKER_HEADER`] (`x-ende-desktop-relay: 1`)
//!   came through the loopback relay, so it is a WebSocket upgrade with the
//!   token and the app origin, but possibly from a native local process that
//!   knows the token. The relay removes every copy a client sent and adds
//!   exactly one; the bridge removes it from every request and response;
//! * a request without it was loaded by the webview. That doesn't prove the
//!   app's own page sent it: any document the webview shows can address the
//!   app origin (a cross-origin iframe, a remote page the window navigated
//!   to, a form another site posts to the app). A request whose `Origin` is
//!   present and not exactly the app origin, or whose `Sec-Fetch-Site` is
//!   anything but `same-origin` / `none`, carries exactly one
//!   [`CROSS_ORIGIN_MARKER_HEADER`] (`x-ende-desktop-cross-origin: 1`). It is
//!   still forwarded (an identity provider's `form_post` callback is a
//!   legitimate cross-site POST). The marker only helps where the engine
//!   passes those headers to the scheme handler, and WKWebView passes
//!   neither for fetches and navigations (macOS 27; only `Referer`), so an
//!   unmarked request doesn't prove anything about the document that made
//!   it.
//!
//! The bridge buffers a request body of at most [`MAX_BRIDGE_REQUEST_BODY`]
//! bytes (`413` beyond it).
//!
//! # Redirects
//!
//! WKWebView doesn't follow a redirect a scheme handler answers with: a
//! `fetch` gets the 3xx itself and a navigation stops at it, leaving the
//! window on the page it was on. So on macOS the bridge handles a `301`,
//! `302`, `303`, `307` or `308` whose `Location` resolves to the app origin
//! ([`redirect_action`]):
//!
//! * A navigation ([`is_navigation`]) whose redirect is a `GET` (any
//!   redirect of a `GET`; `301` / `302` / `303` of a form `POST`) gets a
//!   `200` page that runs `location.replace(<target>)`, so the window ends
//!   up at the target URL and relative links resolve against it. Each
//!   window gets [`MAX_REDIRECTS`] such redirects in a row; then a `502`.
//! * Anything else (`fetch`, XHR, subresources, a `307` / `308` of a form
//!   `POST`) is followed by the bridge, with the Fetch standard's method
//!   rules: `303`, and `301` / `302` after a `POST`, become a `GET` without
//!   the body; `307` / `308` resend the method and the buffered body. After
//!   [`MAX_REDIRECTS`] the request gets a `502`. The page can't tell:
//!   `response.url` stays the URL it asked for, `response.redirected` is
//!   false, and `redirect: "manual"` / `"error"` are followed too.
//!
//! A redirect elsewhere (another site, another host on the scheme) is passed
//! through: on macOS 27 a `fetch` resolves with the 3xx and a navigation
//! stops, neither leaves the app origin. WebView2 is untested, so Windows
//! passes every 3xx through.
//!
//! # laufey 0.7.0 limits
//!
//! * The hosts give no cancel notification (the crate passes no `on_cancel`),
//!   so the bridge only learns that the webview gave up when a body write
//!   fails. Until then a cancelled request keeps the app's handler running:
//!   a long poll runs until the app answers, and a stream that is idle
//!   between chunks stays open until its next chunk.
//! * WebView2 buffers the whole response before the page sees any of it, so
//!   server-sent events and long polls don't work on Windows.
//! * WKWebView forwards `HTTPBody`, not `HTTPBodyStream`, so a body the
//!   engine streams arrives empty (a `Blob` body does, on macOS 27; strings
//!   and `FormData` arrive).
//! * WKWebView keeps no cookies for the scheme: `Set-Cookie` is ignored and
//!   `document.cookie` stays empty (macOS 27).

use std::net::SocketAddr;
use std::time::Duration;

use ende_runtime::ende_net::memory::connect_memory;
use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::header::CONNECTION;
use hyper::header::HOST;
use hyper::header::LOCATION;
use hyper::header::PROXY_AUTHENTICATE;
use hyper::header::PROXY_AUTHORIZATION;
use hyper::header::TE;
use hyper::header::TRAILER;
use hyper::header::TRANSFER_ENCODING;
use hyper::header::UPGRADE;
use hyper_util::rt::TokioIo;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

/// The scheme the webview loads the app from. laufey 0.7.0 hard-codes it in
/// the macOS, Windows and CEF hosts.
pub const APP_SCHEME: &str = "app";

/// The variable through which the app learns the URL its page dials for a
/// WebSocket to its own server:
/// `ws://127.0.0.1:<port>/.ende-desktop-relay/<token>`, to which the page
/// appends its path (`…/<token>/ws?room=1`). Only the runtime sees it
/// (`ende_os::set_process_local_env_var`): the token is the relay's secret.
pub const WS_URL_ENV: &str = "ENDE_DESKTOP_WS_URL";

/// The request-target prefix the relay requires, followed by
/// [`relay_token`].
pub const RELAY_PATH_PREFIX: &str = "/.ende-desktop-relay/";

/// The header that marks a request from the WebSocket relay (see the module
/// docs). Lowercase; header names compare case-insensitively.
pub const RELAY_MARKER_HEADER: &str = "x-ende-desktop-relay";

/// The header the bridge adds to a request a document of another origin
/// made (see the module docs). Lowercase.
pub const CROSS_ORIGIN_MARKER_HEADER: &str = "x-ende-desktop-cross-origin";

/// The largest request body the bridge forwards to `Ende.serve`; a larger
/// one is answered `413 Payload Too Large`.
pub const MAX_BRIDGE_REQUEST_BODY: usize = 4 * 1024 * 1024;

/// The page origin `app://<host>`. `host` is the baked `app_origin_host`:
/// a lowercase domain name, checked at build time, that also names the
/// memory listener.
#[derive(Clone, Debug)]
pub struct AppOrigin {
  host: String,
}

impl AppOrigin {
  pub fn new(host: &str) -> Self {
    Self {
      host: host.to_string(),
    }
  }

  pub fn host(&self) -> &str {
    &self.host
  }

  /// The URL the initial window loads: `app://<host>/`.
  pub fn root_url(&self) -> String {
    format!("{self}/")
  }

  /// Whether an `Origin` header value is exactly this origin. Only spaces
  /// and tabs around it are ignored; case is significant (the browser sends
  /// the origin as it navigated to it, which is lowercase).
  fn matches_origin_header(&self, value: &str) -> bool {
    value
      .trim_matches([' ', '\t'])
      .strip_prefix(APP_SCHEME)
      .and_then(|rest| rest.strip_prefix("://"))
      == Some(self.host.as_str())
  }
}

impl std::fmt::Display for AppOrigin {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{APP_SCHEME}://{}", self.host)
  }
}

/// The URL the page dials (plus its own path) for the relay bound at `addr`:
/// the value of [`WS_URL_ENV`]. `None` when no relay token could be drawn
/// (the relay then refuses every upgrade).
pub fn ws_relay_url(addr: SocketAddr) -> Option<String> {
  Some(format!(
    "ws://127.0.0.1:{}{RELAY_PATH_PREFIX}{}",
    addr.port(),
    relay_token()?
  ))
}

/// This launch's relay secret: 32 random bytes as 64 lowercase hex digits,
/// drawn once. `None` if the system RNG failed, in which case the relay
/// admits nothing (never a guessable token).
pub fn relay_token() -> Option<&'static str> {
  static TOKEN: std::sync::OnceLock<Option<String>> =
    std::sync::OnceLock::new();
  TOKEN
    .get_or_init(|| {
      let mut bytes = [0u8; 32];
      let provider = rustls::crypto::aws_lc_rs::default_provider();
      provider.secure_random.fill(&mut bytes).ok()?;
      Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
    })
    .as_deref()
}

type BridgeError = Box<dyn std::error::Error + Send + Sync>;

/// One webview request's exchange: [`laufey::SchemeExchange`], or a fake in
/// tests.
trait Exchange {
  /// Bytes of the request body read (>0), 0 at its end, negative on an
  /// error.
  fn read_body(&self, buf: &mut [u8]) -> isize;
  /// Send the response status and headers, once.
  fn begin(&self, status: i32, headers: &[(String, String)]);
  /// Bytes of a response body chunk accepted, negative once the webview has
  /// gone away.
  fn write(&self, buf: &[u8]) -> isize;
}

impl Exchange for laufey::SchemeExchange {
  fn read_body(&self, buf: &mut [u8]) -> isize {
    laufey::SchemeExchange::read_body(self, buf)
  }

  fn begin(&self, status: i32, headers: &[(String, String)]) {
    laufey::SchemeExchange::begin(self, status, headers)
  }

  fn write(&self, buf: &[u8]) -> isize {
    laufey::SchemeExchange::write(self, buf)
  }
}

/// Register the `app` scheme handler on the current tokio runtime. Each
/// request is bridged on its own task, so the backend's thread is never
/// blocked. Call it from the runtime's tokio context, before the first
/// window is created.
///
/// Redirects to the app origin are handled by the bridge on macOS only
/// (see the module docs): WKWebView doesn't follow them. Whether WebView2
/// does is untested, so Windows passes every 3xx through as before.
pub fn register(origin: AppOrigin) {
  let handle = tokio::runtime::Handle::current();
  let bridge =
    std::sync::Arc::new(Bridge::new(origin, cfg!(target_os = "macos")));
  laufey::register_scheme_handler(APP_SCHEME, move |req| {
    log::trace!("[desktop] {} {}", req.method, req.url);
    let bridge = bridge.clone();
    handle.spawn(async move {
      let exchange = req.exchange;
      bridge
        .respond(
          req.window_id,
          &req.method,
          &req.url,
          &req.headers,
          &exchange,
        )
        .await;
      exchange.finish();
    });
  });
}

/// The most redirects one request follows, as in the Fetch standard. A
/// window's navigations get as many in a row.
const MAX_REDIRECTS: usize = 20;

/// The `app` scheme handler's state.
struct Bridge {
  origin: AppOrigin,
  /// Whether same-origin redirects are followed (fetches) or turned into a
  /// page that navigates to the target (navigations), instead of being
  /// passed to the webview.
  follow_redirects: bool,
  /// Per window: how many same-origin redirects its navigations got in a
  /// row, so a redirect loop stops like it would in a browser.
  navigation_redirects: std::sync::Mutex<std::collections::HashMap<u32, usize>>,
}

impl Bridge {
  fn new(origin: AppOrigin, follow_redirects: bool) -> Self {
    Self {
      origin,
      follow_redirects,
      navigation_redirects: Default::default(),
    }
  }

  /// Answer one webview request: bridged to the app, or a `502` page when
  /// the bridge failed before a response head went out.
  async fn respond(
    &self,
    window_id: u32,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    exchange: &impl Exchange,
  ) {
    let mut began = false;
    if let Err(e) = Box::pin(
      self.bridge(window_id, method, url, headers, exchange, &mut began),
    )
    .await
    {
      log::error!("[desktop] {APP_SCHEME}:// bridge error: {e}");
      if !began {
        begin_text(exchange, 502);
        let _ =
          exchange.write(format!("desktop transport error: {e}\n").as_bytes());
      }
    }
  }

  async fn bridge(
    &self,
    window_id: u32,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    exchange: &impl Exchange,
    began: &mut bool,
  ) -> Result<(), BridgeError> {
    let origin = &self.origin;
    // The bridge backs exactly one origin. Another host on the scheme is
    // another origin to the browser, and serving it would run the app at an
    // origin nobody chose.
    if !authority_matches(url, origin) {
      begin_text(exchange, 421);
      *began = true;
      let _ = exchange.write(
        format!("misdirected request: this app is served at {origin}\n")
          .as_bytes(),
      );
      return Ok(());
    }

    let body = match read_request_body(exchange, method) {
      Ok(body) => bytes::Bytes::from(body),
      Err(BodyError::TooLarge) => {
        begin_text(exchange, 413);
        *began = true;
        let _ = exchange.write(
          format!("request body larger than {MAX_BRIDGE_REQUEST_BODY} bytes\n")
            .as_bytes(),
        );
        return Ok(());
      }
      Err(BodyError::Unreadable) => {
        // Not forwarded as if it were complete: the app would act on a
        // truncated upload or form.
        begin_text(exchange, 400);
        *began = true;
        let _ = exchange.write(b"the request body could not be read\n");
        return Ok(());
      }
    };

    let navigation = is_navigation(headers);
    let mut request = BridgedRequest {
      method: method.to_string(),
      url: url.to_string(),
      headers,
      body,
      drop_body_headers: false,
    };
    let mut followed = 0;
    let response = loop {
      let response = request.send(origin).await?;
      if !self.follow_redirects {
        break response;
      }
      let location = response.headers().get(LOCATION).map(|value| {
        response_header_value(LOCATION.as_str(), value.as_bytes(), origin)
      });
      let action = redirect_action(
        response.status().as_u16(),
        location.as_deref(),
        &request.url,
        &request.method,
        navigation,
        origin,
      );
      match action {
        RedirectAction::PassThrough => break response,
        RedirectAction::Navigate(target) => {
          if !self.count_navigation_redirect(window_id) {
            return too_many_redirects(exchange, began, &request.url);
          }
          begin_navigation_page(exchange);
          *began = true;
          write_all(exchange, navigation_page(&target).as_bytes()).await;
          return Ok(());
        }
        RedirectAction::Follow { url, method } => {
          followed += 1;
          if followed > MAX_REDIRECTS {
            return too_many_redirects(exchange, began, &request.url);
          }
          // 301 / 302 after a POST and 303 become a GET without a body.
          if method != request.method {
            request.body = bytes::Bytes::new();
            request.drop_body_headers = true;
          }
          request.method = method;
          request.url = url;
        }
      }
    };
    if navigation {
      self.navigation_redirects.lock().unwrap().remove(&window_id);
    }

    let status = i32::from(response.status().as_u16());
    let mut resp_headers = Vec::with_capacity(response.headers().len());
    for (name, value) in response.headers() {
      if is_hop_by_hop_header(name.as_str()) || is_relay_marker(name.as_str()) {
        continue;
      }
      resp_headers.push((
        name.as_str().to_string(),
        response_header_value(name.as_str(), value.as_bytes(), origin),
      ));
    }
    exchange.begin(status, &resp_headers);
    *began = true;

    // The next frame is only pulled once the webview took this one. When a
    // write fails (the webview cancelled or went away), returning drops the
    // response and the connection, so the app's `request.signal` aborts and
    // its body stream is cancelled.
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
      let frame = frame?;
      if let Some(chunk) = frame.data_ref()
        && !write_all(exchange, chunk).await
      {
        break;
      }
    }
    Ok(())
  }

  /// Count a same-origin redirect of a navigation in `window_id`. False
  /// (and the count starts over) once there were more than
  /// [`MAX_REDIRECTS`] in a row: the window is in a redirect loop.
  fn count_navigation_redirect(&self, window_id: u32) -> bool {
    let mut counts = self.navigation_redirects.lock().unwrap();
    let count = counts.entry(window_id).or_default();
    *count += 1;
    if *count > MAX_REDIRECTS {
      counts.remove(&window_id);
      return false;
    }
    true
  }
}

/// A request the bridge sends to the app: the webview's, or the next hop of
/// a redirect it follows.
struct BridgedRequest<'a> {
  method: String,
  /// `app://<host>/…`
  url: String,
  /// The webview's headers.
  headers: &'a [(String, String)],
  body: bytes::Bytes,
  /// Leave out the headers that describe a body: the method was changed to
  /// `GET` and the body dropped.
  drop_body_headers: bool,
}

impl BridgedRequest<'_> {
  /// Send it over a fresh in-process connection to the app's listener,
  /// which is named after the host.
  async fn send(
    &self,
    origin: &AppOrigin,
  ) -> Result<hyper::Response<hyper::body::Incoming>, BridgeError> {
    let stream = connect_memory(origin.host())?;
    let (mut sender, conn) =
      hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
      let _ = conn.await;
    });

    let mut builder = hyper::Request::builder()
      .method(self.method.as_str())
      .uri(path_and_query(&self.url));
    for (name, value) in self.headers {
      if should_skip_request_header(name)
        || (self.drop_body_headers && is_request_body_header(name))
      {
        continue;
      }
      builder = builder.header(name.as_str(), value.as_str());
    }
    // `request.url` is `http://<host>/…` on the app's side.
    builder = builder.header(HOST, origin.host());
    if is_cross_origin_request(self.headers, origin) {
      builder = builder.header(CROSS_ORIGIN_MARKER_HEADER, "1");
    }
    let request = builder.body(Full::new(self.body.clone()))?;
    Ok(sender.send_request(request).await?)
  }
}

fn begin_text(exchange: &impl Exchange, status: i32) {
  exchange.begin(
    status,
    &[(
      "content-type".to_string(),
      "text/plain; charset=utf-8".to_string(),
    )],
  );
}

/// The `502` for a request that was redirected more than [`MAX_REDIRECTS`]
/// times, where a browser's `fetch` would fail with a network error.
fn too_many_redirects(
  exchange: &impl Exchange,
  began: &mut bool,
  url: &str,
) -> Result<(), BridgeError> {
  log::warn!("[desktop] too many redirects, the last to {url}");
  begin_text(exchange, 502);
  *began = true;
  let _ = exchange.write(b"too many redirects\n");
  Ok(())
}

/// What the bridge does with an app response (see the module docs).
#[derive(Debug, PartialEq, Eq)]
enum RedirectAction {
  /// Hand it to the webview as it is.
  PassThrough,
  /// A navigation redirected to this `app://<host>/…` URL: answer with a
  /// page that navigates there.
  Navigate(String),
  /// Send the request again, to this URL with this method.
  Follow { url: String, method: String },
}

/// The bridge's answer to a response with `status` and `location` (moved to
/// the app origin already) to a `method` request for `url`. Only a
/// redirect status with a `Location` that resolves to the app origin is
/// handled; anything else, a cross-origin redirect included, is passed
/// through.
///
/// The method changes as in the Fetch standard: `301` / `302` after a
/// `POST`, and `303` after anything but `GET` / `HEAD`, become a `GET`
/// (without the body); `307` / `308` keep the method and the body. A
/// navigation whose redirect is a `GET` becomes [`RedirectAction::Navigate`],
/// so the window ends up at the target URL. A `307` / `308` of a form
/// `POST` can't be replayed by a navigation and is followed here instead.
fn redirect_action(
  status: u16,
  location: Option<&str>,
  url: &str,
  method: &str,
  navigation: bool,
  origin: &AppOrigin,
) -> RedirectAction {
  if !matches!(status, 301 | 302 | 303 | 307 | 308) {
    return RedirectAction::PassThrough;
  }
  let Some(target) = location.and_then(|location| {
    ende_core::url::Url::parse(url).ok()?.join(location).ok()
  }) else {
    return RedirectAction::PassThrough;
  };
  let same_origin = target.scheme() == APP_SCHEME
    && target.host_str() == Some(origin.host())
    && target.port().is_none()
    && target.username().is_empty()
    && target.password().is_none();
  if !same_origin {
    return RedirectAction::PassThrough;
  }
  let to_get = match status {
    301 | 302 => method.eq_ignore_ascii_case("POST"),
    303 => {
      !method.eq_ignore_ascii_case("GET")
        && !method.eq_ignore_ascii_case("HEAD")
    }
    _ => false,
  };
  let method = if to_get { "GET" } else { method };
  if navigation && method.eq_ignore_ascii_case("GET") {
    return RedirectAction::Navigate(target.into());
  }
  RedirectAction::Follow {
    url: target.into(),
    method: method.to_string(),
  }
}

/// Whether the webview loads the request as a document (a navigation, in
/// a window or a frame) rather than for `fetch`, XHR or a subresource.
/// laufey 0.7.0 doesn't say, and WKWebView sends no `Sec-Fetch-*` to a
/// scheme handler, so without `Sec-Fetch-Mode` it is a request that asks
/// for HTML and carries `Upgrade-Insecure-Requests`, which engines send
/// with navigations only (a `fetch` that asks for HTML doesn't send it).
fn is_navigation(headers: &[(String, String)]) -> bool {
  let value = |name: &str| {
    headers
      .iter()
      .find(|(n, _)| n.trim().eq_ignore_ascii_case(name))
      .map(|(_, v)| v.trim_matches([' ', '\t']))
  };
  if let Some(mode) = value("sec-fetch-mode") {
    return mode.eq_ignore_ascii_case("navigate");
  }
  value("upgrade-insecure-requests") == Some("1")
    && value("accept")
      .is_some_and(|accept| accept.to_ascii_lowercase().contains("text/html"))
}

/// Fetch's request-body-header names, dropped with the body when a
/// redirect changes the method to `GET`.
fn is_request_body_header(name: &str) -> bool {
  let name = name.trim();
  [
    "content-type",
    "content-encoding",
    "content-language",
    "content-location",
  ]
  .iter()
  .any(|n| name.eq_ignore_ascii_case(n))
}

fn begin_navigation_page(exchange: &impl Exchange) {
  exchange.begin(
    200,
    &[
      (
        "content-type".to_string(),
        "text/html; charset=utf-8".to_string(),
      ),
      ("cache-control".to_string(), "no-store".to_string()),
    ],
  );
}

/// The page a redirected navigation gets instead of the redirect: it
/// replaces itself with `target` (so it leaves no history entry, like a
/// redirect). `target` is a serialized URL on the app origin.
///
/// It goes into a script as a JSON string, with `<`, `>`, `&`, U+2028 and
/// U+2029 escaped, so no `Location` can end the script or add markup. The
/// `<noscript>` refresh is for a webview without JavaScript; there the URL
/// is in a double-quoted `url=` (a `"` is percent-encoded first, so it
/// can't end that early) inside an HTML-escaped attribute.
fn navigation_page(target: &str) -> String {
  let json = serde_json::Value::from(target)
    .to_string()
    .replace('<', "\\u003c")
    .replace('>', "\\u003e")
    .replace('&', "\\u0026")
    .replace('\u{2028}', "\\u2028")
    .replace('\u{2029}', "\\u2029");
  let refresh = html_attribute_escape(&target.replace('"', "%22"));
  format!(
    "<!doctype html><meta charset=\"utf-8\"><title>Redirecting</title>\
     <script>location.replace({json});</script>\
     <noscript><meta http-equiv=\"refresh\" content=\"0; url=&quot;{refresh}&quot;\"></noscript>\n"
  )
}

fn html_attribute_escape(text: &str) -> String {
  let mut escaped = String::with_capacity(text.len());
  for c in text.chars() {
    match c {
      '&' => escaped.push_str("&amp;"),
      '"' => escaped.push_str("&quot;"),
      '\'' => escaped.push_str("&#39;"),
      '<' => escaped.push_str("&lt;"),
      '>' => escaped.push_str("&gt;"),
      c => escaped.push(c),
    }
  }
  escaped
}

#[derive(Debug, PartialEq, Eq)]
enum BodyError {
  /// Larger than [`MAX_BRIDGE_REQUEST_BODY`].
  TooLarge,
  /// The backend reported an error part way, or before any byte.
  Unreadable,
}

/// The whole request body. The hosts buffer it, so the reads are
/// non-blocking copies. A read error is never taken for the end of the
/// body. A `GET` / `HEAD` / `OPTIONS` whose first read fails has no body to
/// lose (a backend without body support fails every read) and gets an empty
/// one.
fn read_request_body(
  source: &impl Exchange,
  method: &str,
) -> Result<Vec<u8>, BodyError> {
  let mut body = Vec::new();
  let mut buf = [0u8; 16 * 1024];
  loop {
    let n = source.read_body(&mut buf);
    if n == 0 {
      return Ok(body);
    }
    if n < 0 {
      let bodiless = ["GET", "HEAD", "OPTIONS"]
        .iter()
        .any(|m| m.eq_ignore_ascii_case(method));
      return if body.is_empty() && bodiless {
        Ok(body)
      } else {
        Err(BodyError::Unreadable)
      };
    }
    let n = (n as usize).min(buf.len());
    if body.len().saturating_add(n) > MAX_BRIDGE_REQUEST_BODY {
      return Err(BodyError::TooLarge);
    }
    body.extend_from_slice(&buf[..n]);
  }
}

/// Hand all of `chunk` to the webview, retrying with a growing pause (at
/// most [`WRITE_RETRY_MAX`]) while it accepts nothing. A short write used to
/// drop the rest of the chunk. False once the webview has gone away.
async fn write_all(sink: &impl Exchange, mut chunk: &[u8]) -> bool {
  let mut pause = WRITE_RETRY_MIN;
  while !chunk.is_empty() {
    let n = sink.write(chunk);
    if n < 0 {
      return false;
    }
    let n = (n as usize).min(chunk.len());
    if n == 0 {
      tokio::time::sleep(pause).await;
      pause = (pause * 2).min(WRITE_RETRY_MAX);
      continue;
    }
    chunk = &chunk[n..];
    pause = WRITE_RETRY_MIN;
  }
  true
}

const WRITE_RETRY_MIN: Duration = Duration::from_millis(1);
const WRITE_RETRY_MAX: Duration = Duration::from_millis(20);

/// A response header's value as the webview gets it.
///
/// Header values are bytes: UTF-8 is kept, anything else is decoded as
/// Latin-1 (what browsers do with raw header bytes), instead of the header
/// being dropped.
///
/// The app's `request.url` is `http://<host>/…`, so an absolute URL it builds
/// from it (`Response.redirect(new URL("/login", req.url))`) points at a
/// plain `http:` host the webview can't reach. In `location`,
/// `content-location` and `refresh`, a URL on exactly that host is moved to
/// the app origin.
fn response_header_value(
  name: &str,
  value: &[u8],
  origin: &AppOrigin,
) -> String {
  let text = match std::str::from_utf8(value) {
    Ok(text) => text.to_string(),
    Err(_) => value.iter().map(|&b| b as char).collect(),
  };
  if name.eq_ignore_ascii_case("location")
    || name.eq_ignore_ascii_case("content-location")
  {
    return rewrite_app_url(&text, origin).unwrap_or(text);
  }
  if name.eq_ignore_ascii_case("refresh") {
    // `<seconds>; url=<url>` (the `url=` part may be quoted or absent).
    let lower = text.to_ascii_lowercase();
    if let Some(i) = lower.find("url=") {
      let start = i + "url=".len();
      let (quote, start) = match text[start..].chars().next() {
        Some(q @ ('\'' | '"')) => (Some(q), start + 1),
        _ => (None, start),
      };
      let end = quote
        .and_then(|q| text[start..].find(q).map(|j| start + j))
        .unwrap_or(text.len());
      if let Some(url) = rewrite_app_url(&text[start..end], origin) {
        return format!("{}{url}{}", &text[..start], &text[end..]);
      }
    }
  }
  text
}

/// `http://<host>[/rest]` -> `app://<host>[/rest]` for exactly the origin's
/// host (ASCII case-insensitive, as `http:` hosts are); `None` for any other
/// URL, a port or user info included.
///
/// Prefixes are compared as bytes: the value is whatever the app put in a
/// header, so slicing it at a prefix length could land inside a character,
/// and that panic would exit the app.
fn rewrite_app_url(url: &str, origin: &AppOrigin) -> Option<String> {
  let rest = strip_prefix_ignore_ascii_case(url.trim_start(), "http://")?;
  let rest = strip_prefix_ignore_ascii_case(rest, origin.host())?;
  match rest.as_bytes().first() {
    None | Some(b'/' | b'?' | b'#') => Some(format!("{origin}{rest}")),
    _ => None,
  }
}

/// `text` without `prefix` (ASCII, compared ASCII case-insensitively), or
/// `None` when `text` doesn't start with it. `text` is only split right
/// after a matched ASCII prefix, which is always a character boundary.
fn strip_prefix_ignore_ascii_case<'a>(
  text: &'a str,
  prefix: &str,
) -> Option<&'a str> {
  let head = text.as_bytes().get(..prefix.len())?;
  if prefix.is_ascii() && head.eq_ignore_ascii_case(prefix.as_bytes()) {
    text.get(prefix.len()..)
  } else {
    None
  }
}

/// Split `scheme://authority[/path][?query][#fragment]` into
/// `(authority, rest-after-authority)`. `None` if there is no `://`.
fn split_authority(url: &str) -> Option<(&str, &str)> {
  let (_, rest) = url.split_once("://")?;
  let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
  Some((&rest[..end], &rest[end..]))
}

/// Whether the request URL's authority is exactly the origin's host. The
/// scheme isn't checked again: the webview only delivers `app:` URLs.
fn authority_matches(url: &str, origin: &AppOrigin) -> bool {
  split_authority(url).map(|(authority, _)| authority) == Some(origin.host())
}

/// The path and query of a `scheme://authority/path?query` URL, the form a
/// request target needs: a bare authority is `/`, a query without a path
/// gets a `/` before it, and a fragment is dropped.
fn path_and_query(url: &str) -> String {
  match split_authority(url) {
    Some((_, rest)) => {
      let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
      if rest.is_empty() {
        "/".to_string()
      } else if rest.starts_with('?') {
        format!("/{rest}")
      } else {
        rest.to_string()
      }
    }
    None => url.to_string(),
  }
}

fn should_skip_request_header(name: &str) -> bool {
  name.eq_ignore_ascii_case(HOST.as_str())
    || is_hop_by_hop_header(name)
    || is_relay_marker(name)
    || name.trim().eq_ignore_ascii_case(CROSS_ORIGIN_MARKER_HEADER)
    // The body is forwarded whole (`Full`), and hyper sets the length of
    // what is sent. The webview's own `content-length` could contradict it
    // (a body the backend read short, or one it was never given), and
    // `expect: 100-continue` would make the server wait for a continuation
    // that is never asked for.
    || name.eq_ignore_ascii_case("content-length")
    || name.eq_ignore_ascii_case("expect")
}

/// Whether a document of another origin made the request: an `Origin` that
/// is present and not exactly the app origin (`null`, a remote page, another
/// host on the scheme), or a `Sec-Fetch-Site` other than `same-origin` /
/// `none` (`none`: the user started it). Engines don't send `Sec-Fetch-*` to
/// every custom scheme, so `Origin` is the check that always applies to a
/// state-changing request.
fn is_cross_origin_request(
  headers: &[(String, String)],
  origin: &AppOrigin,
) -> bool {
  headers.iter().any(|(name, value)| {
    let name = name.trim();
    if name.eq_ignore_ascii_case("origin") {
      return !origin.matches_origin_header(value);
    }
    if name.eq_ignore_ascii_case("sec-fetch-site") {
      let site = value.trim_matches([' ', '\t']);
      return !site.eq_ignore_ascii_case("same-origin")
        && !site.eq_ignore_ascii_case("none");
    }
    false
  })
}

/// Whether a header name is [`RELAY_MARKER_HEADER`] (any case, surrounding
/// whitespace ignored).
fn is_relay_marker(name: &str) -> bool {
  name.trim().eq_ignore_ascii_case(RELAY_MARKER_HEADER)
}

fn is_hop_by_hop_header(name: &str) -> bool {
  name.eq_ignore_ascii_case(CONNECTION.as_str())
    || name.eq_ignore_ascii_case("keep-alive")
    || name.eq_ignore_ascii_case(PROXY_AUTHENTICATE.as_str())
    || name.eq_ignore_ascii_case(PROXY_AUTHORIZATION.as_str())
    || name.eq_ignore_ascii_case(TE.as_str())
    || name.eq_ignore_ascii_case(TRAILER.as_str())
    || name.eq_ignore_ascii_case(TRANSFER_ENCODING.as_str())
    || name.eq_ignore_ascii_case(UPGRADE.as_str())
}

// --- WebSocket loopback relay ----------------------------------------------

/// Bind the WebSocket relay on `127.0.0.1:0`. Hand the listener to
/// [`spawn_ws_loopback_proxy`] once the tokio runtime is up.
pub fn bind_ws_loopback_listener() -> std::io::Result<std::net::TcpListener> {
  std::net::TcpListener::bind("127.0.0.1:0")
}

/// Start the relay's accept loop on a listener from
/// [`bind_ws_loopback_listener`], forwarding admitted upgrades to the memory
/// listener named after `origin`'s host. Call it from the runtime's tokio
/// context.
pub fn spawn_ws_loopback_proxy(
  listener: std::net::TcpListener,
  origin: AppOrigin,
) {
  if let Err(e) = listener.set_nonblocking(true) {
    log::error!("[desktop] ws relay: set_nonblocking failed: {e}");
    return;
  }
  let listener = match TcpListener::from_std(listener) {
    Ok(l) => l,
    Err(e) => {
      log::error!("[desktop] ws relay: tokio listener conversion failed: {e}");
      return;
    }
  };
  if let Ok(addr) = listener.local_addr() {
    log::debug!("[desktop] ws relay listening on {addr} for origin {origin}");
  }
  tokio::spawn(run_ws_loopback_proxy(listener, origin, HEAD_READ_TIMEOUT));
}

/// The relay's accept loop: one task per connection.
async fn run_ws_loopback_proxy(
  listener: TcpListener,
  origin: AppOrigin,
  head_timeout: Duration,
) {
  loop {
    match listener.accept().await {
      Ok((stream, _peer)) => {
        let origin = origin.clone();
        tokio::spawn(async move {
          if let Err(e) =
            proxy_ws_connection(stream, &origin, head_timeout).await
          {
            log::debug!("[desktop] ws relay connection error: {e}");
          }
        });
      }
      Err(e) => {
        log::error!("[desktop] ws relay accept failed: {e}");
        // Don't spin on a broken listener.
        tokio::time::sleep(Duration::from_millis(200)).await;
      }
    }
  }
}

/// The largest request head the relay buffers. A WebSocket handshake is
/// small.
const MAX_HEAD_LEN: usize = 8 * 1024;

/// How long the relay waits for a client's complete request head. A browser
/// sends its handshake at once, but engines also open idle preconnect
/// sockets, and without a deadline each would hold a task and a file
/// descriptor for the life of the app. The deadline covers the whole head,
/// so trickling bytes doesn't extend it.
const HEAD_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the relay waits for `Ende.serve`'s answer to a forwarded
/// upgrade (the handler may do async work before it upgrades).
const UPSTREAM_HEAD_TIMEOUT: Duration = Duration::from_secs(30);

/// What the relay does with a connection, decided from its request head.
#[derive(Debug, PartialEq, Eq)]
enum RelayDecision {
  /// A WebSocket upgrade with the token, from the app origin.
  Proxy,
  /// Not a `GET` WebSocket upgrade (plain HTTP, a port scan, …).
  NotWebSocket,
  /// A WebSocket upgrade whose `Origin` is missing, repeated or another
  /// origin's.
  ForbiddenOrigin,
  /// A WebSocket upgrade whose target doesn't carry this launch's token.
  ForbiddenToken,
}

/// Classify a request head (up to and including its blank line), given this
/// launch's token (`None`: none could be drawn, nothing is admitted).
///
/// A page can't set the `Origin` of its WebSocket handshake: the browser
/// fills in the page's own. Exactly one `Origin` is required, byte for byte
/// the app origin; a repeated header is how a second value gets past a check
/// that only reads the first.
fn classify_upgrade(
  head: &[u8],
  origin: &AppOrigin,
  token: Option<&str>,
) -> RelayDecision {
  if !head.starts_with(b"GET ") || !is_websocket_upgrade(head) {
    return RelayDecision::NotWebSocket;
  }
  if relay_target(head, token).is_none() {
    return RelayDecision::ForbiddenToken;
  }
  let mut origins = header_values(head, b"origin");
  let (Some(value), None) = (origins.next(), origins.next()) else {
    return RelayDecision::ForbiddenOrigin;
  };
  match std::str::from_utf8(value) {
    Ok(value) if origin.matches_origin_header(value) => RelayDecision::Proxy,
    _ => RelayDecision::ForbiddenOrigin,
  }
}

/// The request target the app's server gets for a head whose target is
/// `/.ende-desktop-relay/<token>[rest]`: `rest` (`/` when empty, a `/` put
/// before a bare query). `None` when the target doesn't carry exactly this
/// launch's token, or there is none. The token is compared in constant time.
fn relay_target(head: &[u8], token: Option<&str>) -> Option<Vec<u8>> {
  let token = token?;
  let line_end = head.iter().position(|&b| b == b'\r' || b == b'\n')?;
  let mut parts = head[..line_end].split(|&b| b == b' ');
  let (Some(_method), Some(target), Some(_version), None) =
    (parts.next(), parts.next(), parts.next(), parts.next())
  else {
    return None;
  };
  let after_prefix = target.strip_prefix(RELAY_PATH_PREFIX.as_bytes())?;
  let candidate = after_prefix.get(..token.len())?;
  if !constant_time_eq(candidate, token.as_bytes()) {
    return None;
  }
  let rest = &after_prefix[token.len()..];
  match rest.first() {
    None => Some(b"/".to_vec()),
    Some(b'/') => Some(rest.to_vec()),
    Some(b'?') => Some([b"/".as_slice(), rest].concat()),
    // `<token>x…`: not this token.
    Some(_) => None,
  }
}

/// Byte equality whose time doesn't depend on where the inputs differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
  a.len() == b.len()
    && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn proxy_ws_connection(
  mut tcp: TcpStream,
  origin: &AppOrigin,
  head_timeout: Duration,
) -> std::io::Result<()> {
  let Some((head, end_of_head)) =
    read_request_head(&mut tcp, head_timeout).await?
  else {
    return Ok(());
  };
  let head = &head[..end_of_head];

  let token = relay_token();
  let refusal = match classify_upgrade(head, origin, token) {
    RelayDecision::Proxy => None,
    RelayDecision::NotWebSocket => Some(RELAY_400_RESPONSE),
    RelayDecision::ForbiddenToken => {
      log::debug!(
        "[desktop] ws relay: refused an upgrade without this launch's token"
      );
      Some(RELAY_403_TOKEN_RESPONSE)
    }
    RelayDecision::ForbiddenOrigin => {
      log::debug!(
        "[desktop] ws relay: refused an upgrade with Origin {:?} (expected {origin})",
        header_values(head, b"origin")
          .next()
          .map(String::from_utf8_lossy)
      );
      Some(RELAY_403_RESPONSE)
    }
  };
  if let Some(response) = refusal {
    let _ = tcp.write_all(response).await;
    let _ = tcp.shutdown().await;
    return Ok(());
  }
  // Present: `classify_upgrade` checked it.
  let target = relay_target(head, token).unwrap_or_default();

  let mut upstream = match connect_memory(origin.host()) {
    Ok(s) => s,
    Err(e) => {
      log::warn!("[desktop] ws relay: memory connect failed: {e}");
      let _ = tcp.write_all(RELAY_502_RESPONSE).await;
      let _ = tcp.shutdown().await;
      return Ok(());
    }
  };
  relay_upgrade(
    &mut tcp,
    &mut upstream,
    &relay_request_head(head, &target, origin.host()),
    UPSTREAM_HEAD_TIMEOUT,
  )
  .await
}

/// Forward one admitted upgrade: only the rewritten request `head` (bytes
/// the client pipelined after its head are dropped), then the server's
/// response. Only a `101` is passed on, after which bytes are copied both
/// ways; any other answer, or none within `timeout`, is replaced with the
/// relay's `502` and both sides are closed, so the client can neither read
/// a non-upgrade response nor send a second request.
async fn relay_upgrade<C, U>(
  client: &mut C,
  upstream: &mut U,
  head: &[u8],
  timeout: Duration,
) -> std::io::Result<()>
where
  C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
  U: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
  upstream.write_all(head).await?;
  match read_request_head(upstream, timeout).await? {
    Some((buf, end)) if is_switching_protocols(&buf[..end]) => {
      // The 101 head and any frames the server already sent after it.
      client.write_all(&buf).await?;
      let _ = tokio::io::copy_bidirectional(client, upstream).await;
    }
    response => {
      log::debug!(
        "[desktop] ws relay: the server did not switch protocols ({:?}); \
         closing the connection",
        response.map(|(buf, end)| {
          let line_end =
            buf[..end].iter().position(|&b| b == b'\r').unwrap_or(end);
          String::from_utf8_lossy(&buf[..line_end]).into_owned()
        })
      );
      let _ = client.write_all(RELAY_502_RESPONSE).await;
      let _ = client.shutdown().await;
      let _ = upstream.shutdown().await;
    }
  }
  Ok(())
}

/// The request head the relay sends upstream: the client's request line with
/// its target replaced by `target` (without the relay prefix and token), its
/// header lines without any `Host`, any [`RELAY_MARKER_HEADER`] and any line
/// without a colon, then exactly one `host: <host>` (so `request.url` is
/// `http://<host>/…`, as for the bridge's requests) and exactly one
/// `x-ende-desktop-relay: 1`.
fn relay_request_head(head: &[u8], target: &[u8], host: &str) -> Vec<u8> {
  let mut out = Vec::with_capacity(head.len() + 64);
  let mut lines = head
    .split(|&b| b == b'\n')
    .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
    .filter(|line| !line.is_empty());
  if let Some(request_line) = lines.next() {
    let mut parts = request_line.splitn(3, |&b| b == b' ');
    let method = parts.next().unwrap_or_default();
    let _client_target = parts.next();
    let version = parts.next().unwrap_or(b"HTTP/1.1");
    out.extend_from_slice(method);
    out.push(b' ');
    out.extend_from_slice(target);
    out.push(b' ');
    out.extend_from_slice(version);
    out.extend_from_slice(b"\r\n");
  }
  for line in lines {
    let Some(colon) = line.iter().position(|&b| b == b':') else {
      continue;
    };
    let name = trim_ascii(&line[..colon]);
    if name.eq_ignore_ascii_case(b"host")
      || name.eq_ignore_ascii_case(RELAY_MARKER_HEADER.as_bytes())
    {
      continue;
    }
    out.extend_from_slice(line);
    out.extend_from_slice(b"\r\n");
  }
  out.extend_from_slice(b"host: ");
  out.extend_from_slice(host.as_bytes());
  out.extend_from_slice(b"\r\n");
  out.extend_from_slice(RELAY_MARKER_HEADER.as_bytes());
  out.extend_from_slice(b": 1\r\n\r\n");
  out
}

/// Whether a response head's status is `101` (`HTTP/1.1 101 …`).
fn is_switching_protocols(head: &[u8]) -> bool {
  let Some(rest) = head.strip_prefix(b"HTTP/1.1 101") else {
    return false;
  };
  matches!(rest.first(), Some(b' ' | b'\r'))
}

/// Read an HTTP head: the bytes read so far and the offset one past its
/// blank line. `None` when the peer closes first, sends more than
/// [`MAX_HEAD_LEN`] without finishing it, or doesn't finish it within
/// `timeout`; the relay then drops the connection without an HTTP answer,
/// so a scan doesn't learn what is behind it.
async fn read_request_head<R: tokio::io::AsyncRead + Unpin>(
  reader: &mut R,
  timeout: Duration,
) -> std::io::Result<Option<(Vec<u8>, usize)>> {
  let read = async {
    let mut head = Vec::with_capacity(2048);
    let mut buf = [0u8; 2048];
    loop {
      let n = reader.read(&mut buf).await?;
      if n == 0 {
        return Ok(None);
      }
      head.extend_from_slice(&buf[..n]);
      if let Some(end) = find_end_of_head(&head) {
        return Ok(Some((head, end)));
      }
      if head.len() >= MAX_HEAD_LEN {
        return Ok(None);
      }
    }
  };
  match tokio::time::timeout(timeout, read).await {
    Ok(result) => result,
    Err(_elapsed) => {
      log::debug!(
        "[desktop] ws relay: no complete head within {timeout:?}; \
         closing the connection"
      );
      Ok(None)
    }
  }
}

const RELAY_400_RESPONSE: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\
  content-type: text/plain; charset=utf-8\r\n\
  content-length: 45\r\n\
  connection: close\r\n\r\n\
  desktop ws relay: WebSocket upgrade required\n";

const RELAY_403_RESPONSE: &[u8] = b"HTTP/1.1 403 Forbidden\r\n\
  content-type: text/plain; charset=utf-8\r\n\
  content-length: 47\r\n\
  connection: close\r\n\r\n\
  desktop ws relay: Origin is not the app origin\n";

const RELAY_403_TOKEN_RESPONSE: &[u8] = b"HTTP/1.1 403 Forbidden\r\n\
  content-type: text/plain; charset=utf-8\r\n\
  content-length: 47\r\n\
  connection: close\r\n\r\n\
  desktop ws relay: missing or wrong relay token\n";

const RELAY_502_RESPONSE: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\n\
  content-type: text/plain; charset=utf-8\r\n\
  content-length: 50\r\n\
  connection: close\r\n\r\n\
  desktop ws relay: the server did not upgrade this\n";

/// The offset one past a head's blank line (`\r\n\r\n`).
fn find_end_of_head(buf: &[u8]) -> Option<usize> {
  buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// The `(name, value)` header lines of a head, without the request line.
/// Values have surrounding spaces and tabs trimmed; names are raw.
fn header_lines(head: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
  let mut lines = head
    .split(|&b| b == b'\n')
    .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
  lines.next();
  lines.filter_map(|line| {
    let colon = line.iter().position(|&b| b == b':')?;
    Some((&line[..colon], trim_ascii(&line[colon + 1..])))
  })
}

/// Every value of header `name` (ASCII case-insensitive), in order.
fn header_values<'a>(
  head: &'a [u8],
  name: &'a [u8],
) -> impl Iterator<Item = &'a [u8]> + 'a {
  header_lines(head)
    .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
    .map(|(_, v)| v)
}

/// Whether a head carries `Upgrade: websocket` (names and values compared
/// ASCII case-insensitively; a list of protocols is allowed).
fn is_websocket_upgrade(head: &[u8]) -> bool {
  header_values(head, b"upgrade").any(|value| {
    value
      .split(|&b| b == b',')
      .any(|tok| trim_ascii(tok).eq_ignore_ascii_case(b"websocket"))
  })
}

fn trim_ascii(mut b: &[u8]) -> &[u8] {
  while let [b' ' | b'\t', rest @ ..] = b {
    b = rest;
  }
  while let [rest @ .., b' ' | b'\t'] = b {
    b = rest;
  }
  b
}

#[cfg(test)]
mod tests {
  use std::cell::Cell;
  use std::cell::RefCell;
  use std::collections::VecDeque;
  use std::sync::Arc;
  use std::sync::atomic::AtomicUsize;
  use std::sync::atomic::Ordering;

  use ende_runtime::ende_net::memory::listen_memory;

  use super::*;

  fn origin() -> AppOrigin {
    AppOrigin::new("com.example.app")
  }

  // --- the bridge -----------------------------------------------------------

  /// A scripted webview exchange: the request body it hands out (lengths
  /// to read, or an error code), what each write accepts (then everything),
  /// and what the bridge sent back.
  #[derive(Default)]
  struct FakeExchange {
    body: RefCell<VecDeque<isize>>,
    body_bytes: RefCell<VecDeque<u8>>,
    writes: RefCell<VecDeque<isize>>,
    status: Cell<Option<i32>>,
    headers: RefCell<Vec<(String, String)>>,
    written: RefCell<Vec<u8>>,
    refused: Cell<usize>,
  }

  impl FakeExchange {
    fn with_body(body: &[u8]) -> Self {
      let fake = Self::default();
      *fake.body_bytes.borrow_mut() = body.iter().copied().collect();
      fake
    }

    fn with_body_script(script: &[isize]) -> Self {
      let fake = Self::default();
      *fake.body.borrow_mut() = script.iter().copied().collect();
      fake
    }

    fn with_writes(script: &[isize]) -> Self {
      let fake = Self::default();
      *fake.writes.borrow_mut() = script.iter().copied().collect();
      fake
    }

    fn header(&self, name: &str) -> Option<String> {
      self
        .headers
        .borrow()
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
    }

    fn text(&self) -> String {
      String::from_utf8_lossy(&self.written.borrow()).into_owned()
    }
  }

  impl Exchange for FakeExchange {
    fn read_body(&self, buf: &mut [u8]) -> isize {
      if let Some(n) = self.body.borrow_mut().pop_front() {
        if n <= 0 {
          return n;
        }
        let n = (n as usize).min(buf.len());
        buf[..n].fill(b'x');
        return n as isize;
      }
      let mut bytes = self.body_bytes.borrow_mut();
      let n = bytes.len().min(buf.len());
      for (slot, byte) in buf.iter_mut().zip(bytes.drain(..n)) {
        *slot = byte;
      }
      n as isize
    }

    fn begin(&self, status: i32, headers: &[(String, String)]) {
      assert!(self.status.get().is_none(), "begin called twice");
      self.status.set(Some(status));
      *self.headers.borrow_mut() = headers.to_vec();
    }

    fn write(&self, buf: &[u8]) -> isize {
      let cap = self.writes.borrow_mut().pop_front().unwrap_or(isize::MAX);
      if cap < 0 {
        return cap;
      }
      let n = buf.len().min(cap as usize);
      if n == 0 {
        self.refused.set(self.refused.get() + 1);
      }
      self.written.borrow_mut().extend_from_slice(&buf[..n]);
      n as isize
    }
  }

  fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
      .iter()
      .map(|(n, v)| (n.to_string(), v.to_string()))
      .collect()
  }

  /// The bridge without redirect handling, as on Windows.
  async fn respond(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    origin: &AppOrigin,
    exchange: &impl Exchange,
  ) {
    Bridge::new(origin.clone(), false)
      .respond(0, method, url, headers, exchange)
      .await;
  }

  /// A stand-in for `Ende.serve` on the memory listener `host`: it answers
  /// each request with `response` and sends what it read (head and body)
  /// down the returned channel.
  fn fake_app(
    host: &str,
    response: &'static [u8],
  ) -> tokio::sync::mpsc::UnboundedReceiver<Vec<u8>> {
    fake_app_with(host, move |_| response.to_vec())
  }

  /// [`fake_app`], answering each request (head and body) with what
  /// `respond` returns for it.
  fn fake_app_with(
    host: &str,
    respond: impl Fn(&[u8]) -> Vec<u8> + Send + 'static,
  ) -> tokio::sync::mpsc::UnboundedReceiver<Vec<u8>> {
    let listener = listen_memory(host).unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
      while let Ok((mut stream, _)) = listener.accept().await {
        let (mut got, end) =
          read_request_head(&mut stream, Duration::from_secs(5))
            .await
            .unwrap()
            .expect("a request head");
        let length = header_values(&got[..end], b"content-length")
          .next()
          .map(|v| std::str::from_utf8(v).unwrap().parse::<usize>().unwrap())
          .unwrap_or(0);
        while got.len() < end + length {
          let mut buf = [0u8; 4096];
          let n = stream.read(&mut buf).await.unwrap();
          assert_ne!(n, 0, "the body was cut short");
          got.extend_from_slice(&buf[..n]);
        }
        stream.write_all(&respond(&got)).await.unwrap();
        let _ = stream.shutdown().await;
        let _ = tx.send(got);
      }
    });
    rx
  }

  #[tokio::test]
  async fn bridge_forwards_a_request_to_the_memory_listener_and_back() {
    let origin = AppOrigin::new("bridge-roundtrip.example");
    let mut app = fake_app(
      origin.host(),
      b"HTTP/1.1 303 See Other\r\n\
        location: http://bridge-roundtrip.example/done?x=1\r\n\
        content-disposition: attachment; filename=\"r\xc3\xa9sum\xc3\xa9.txt\"\r\n\
        x-ende-desktop-relay: 1\r\n\
        content-length: 5\r\n\r\nhello",
    );
    let exchange = FakeExchange::with_body(b"name=ende");
    respond(
      "POST",
      "app://bridge-roundtrip.example/submit?draft=1#top",
      &h(&[
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("Origin", "app://bridge-roundtrip.example"),
        ("Host", "spoofed.example"),
        ("Content-Length", "999"),
        ("X-Ende-Desktop-Relay", "1"),
        ("x-ende-desktop-cross-origin", "0"),
      ]),
      &origin,
      &exchange,
    )
    .await;

    // What the app got: the path and query, `Host: <host>` (so its
    // `request.url` is `http://<host>/submit?draft=1`), the body, and none
    // of the markers the page sent.
    let got = app.recv().await.unwrap();
    let end = find_end_of_head(&got).unwrap();
    let head = &got[..end];
    assert!(head.starts_with(b"POST /submit?draft=1 HTTP/1.1\r\n"));
    assert_eq!(
      header_values(head, b"host").collect::<Vec<_>>(),
      vec![b"bridge-roundtrip.example".as_slice()]
    );
    assert_eq!(&got[end..], b"name=ende");
    assert_eq!(
      header_values(head, b"content-length").collect::<Vec<_>>(),
      vec![b"9".as_slice()]
    );
    assert_eq!(
      header_values(head, RELAY_MARKER_HEADER.as_bytes()).count(),
      0
    );
    assert_eq!(
      header_values(head, CROSS_ORIGIN_MARKER_HEADER.as_bytes()).count(),
      0
    );
    assert_eq!(header_values(head, b"content-type").count(), 1);

    // What the webview got: the status, the redirect moved to the app
    // origin, a UTF-8 header kept, no relay marker, the body.
    assert_eq!(exchange.status.get(), Some(303));
    assert_eq!(
      exchange.header("location").as_deref(),
      Some("app://bridge-roundtrip.example/done?x=1")
    );
    assert_eq!(
      exchange.header("content-disposition").as_deref(),
      Some("attachment; filename=\"résumé.txt\"")
    );
    assert_eq!(exchange.header(RELAY_MARKER_HEADER), None);
    assert_eq!(exchange.text(), "hello");
  }

  #[tokio::test]
  async fn bridge_marks_a_request_from_another_origin() {
    let origin = AppOrigin::new("bridge-cross-origin.example");
    let mut app = fake_app(origin.host(), b"HTTP/1.1 204 No Content\r\n\r\n");
    let exchange = FakeExchange::with_body(b"x");
    respond(
      "POST",
      "app://bridge-cross-origin.example/callback",
      &h(&[
        ("Origin", "https://idp.example"),
        ("X-Ende-Desktop-Cross-Origin", "0"),
      ]),
      &origin,
      &exchange,
    )
    .await;
    let got = app.recv().await.unwrap();
    let head = &got[..find_end_of_head(&got).unwrap()];
    // Exactly the bridge's own marker.
    assert_eq!(
      header_values(head, CROSS_ORIGIN_MARKER_HEADER.as_bytes())
        .collect::<Vec<_>>(),
      vec![b"1".as_slice()]
    );
    assert_eq!(exchange.status.get(), Some(204));
  }

  #[tokio::test]
  async fn bridge_refuses_another_host_without_reaching_the_app() {
    // A listener under the other name, which must never be dialled.
    let origin = AppOrigin::new("bridge-misdirected.example");
    let mut other =
      fake_app("bridge-other.example", b"HTTP/1.1 200 OK\r\n\r\n");
    for url in [
      "app://bridge-other.example/",
      "app://bridge-misdirected.example.evil/",
      "app://bridge-misdirected.example:8080/",
      "app://BRIDGE-MISDIRECTED.EXAMPLE/",
      "app:///",
    ] {
      let exchange = FakeExchange::default();
      respond("GET", url, &[], &origin, &exchange).await;
      assert_eq!(exchange.status.get(), Some(421), "{url}");
      assert!(exchange.text().contains("app://bridge-misdirected.example"));
    }
    assert!(other.try_recv().is_err());
  }

  #[tokio::test]
  async fn bridge_answers_502_when_the_app_is_not_listening() {
    let origin = AppOrigin::new("bridge-not-listening.example");
    let exchange = FakeExchange::default();
    respond(
      "GET",
      "app://bridge-not-listening.example/",
      &[],
      &origin,
      &exchange,
    )
    .await;
    assert_eq!(exchange.status.get(), Some(502));
    assert!(exchange.text().starts_with("desktop transport error"));
  }

  #[tokio::test]
  async fn bridge_refuses_large_and_unreadable_bodies() {
    let origin = AppOrigin::new("bridge-bodies.example");
    let mut app = fake_app(origin.host(), b"HTTP/1.1 200 OK\r\n\r\n");
    let chunks = vec![16 * 1024; MAX_BRIDGE_REQUEST_BODY / (16 * 1024) + 1];
    let exchange = FakeExchange::with_body_script(&chunks);
    respond(
      "PUT",
      "app://bridge-bodies.example/",
      &[],
      &origin,
      &exchange,
    )
    .await;
    assert_eq!(exchange.status.get(), Some(413));
    let exchange = FakeExchange::with_body_script(&[3, -1]);
    respond(
      "POST",
      "app://bridge-bodies.example/",
      &[],
      &origin,
      &exchange,
    )
    .await;
    assert_eq!(exchange.status.get(), Some(400));
    assert!(app.try_recv().is_err());
  }

  // --- redirects --------------------------------------------------------------

  /// The request line's target, e.g. `/a?b`.
  fn target_of(request: &[u8]) -> String {
    let line = request.split(|&b| b == b'\r').next().unwrap();
    let line = std::str::from_utf8(line).unwrap();
    line.split(' ').nth(1).unwrap().to_string()
  }

  /// A navigation as WKWebView sends it.
  fn navigation_headers() -> Vec<(String, String)> {
    h(&[
      (
        "Accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
      ),
      ("Upgrade-Insecure-Requests", "1"),
    ])
  }

  /// A redirecting stand-in app: `/see-other` 303s, `/found` 302s,
  /// `/temporary` 307s and `/permanent` 308s to `/final`; `/loop` 302s to
  /// itself; `/away` 302s to another site; anything else is answered 200
  /// with its request line.
  fn redirecting_app(
    host: &'static str,
  ) -> tokio::sync::mpsc::UnboundedReceiver<Vec<u8>> {
    fake_app_with(host, move |request| {
      let target = target_of(request);
      let redirect = |status: &str, location: &str| {
        format!(
          "HTTP/1.1 {status}\r\nlocation: {location}\r\ncontent-length: 0\r\n\r\n"
        )
      };
      let response = match target.as_str() {
        "/see-other" => redirect("303 See Other", "/final"),
        "/found" => redirect("302 Found", &format!("http://{host}/final")),
        "/temporary" => redirect("307 Temporary Redirect", "final"),
        "/permanent" => redirect("308 Permanent Redirect", "/final?x=1"),
        "/loop" => redirect("302 Found", "/loop"),
        "/away" => redirect("302 Found", "https://elsewhere.example/x"),
        _ => {
          let line =
            std::str::from_utf8(request.split(|&b| b == b'\r').next().unwrap())
              .unwrap()
              .to_string();
          format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{line}",
            line.len()
          )
        }
      };
      response.into_bytes()
    })
  }

  /// The next `n` requests the app got, after which it got no more.
  async fn received(
    app: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    n: usize,
  ) -> Vec<Vec<u8>> {
    let mut got = vec![];
    for _ in 0..n {
      got.push(app.recv().await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(app.try_recv().is_err(), "more than {n} requests");
    got
  }

  #[test]
  fn redirect_actions_follow_the_fetch_standard() {
    use RedirectAction::*;
    let origin = origin();
    let base = "app://com.example.app/dir/page?q=1";
    let action = |status, location, method, navigation| {
      redirect_action(status, location, base, method, navigation, &origin)
    };
    let follow = |url: &str, method: &str| Follow {
      url: url.to_string(),
      method: method.to_string(),
    };
    // Relative, absolute-path and absolute targets on the app origin.
    assert_eq!(
      action(302, Some("next"), "GET", false),
      follow("app://com.example.app/dir/next", "GET")
    );
    assert_eq!(
      action(301, Some("/a?b#c"), "GET", false),
      follow("app://com.example.app/a?b#c", "GET")
    );
    assert_eq!(
      action(308, Some("app://com.example.app/x"), "PUT", false),
      follow("app://com.example.app/x", "PUT")
    );
    // Method rules.
    for (status, method, expected) in [
      (301, "POST", "GET"),
      (302, "POST", "GET"),
      (301, "PUT", "PUT"),
      (302, "DELETE", "DELETE"),
      (303, "POST", "GET"),
      (303, "PUT", "GET"),
      (303, "GET", "GET"),
      (303, "HEAD", "HEAD"),
      (307, "POST", "POST"),
      (308, "POST", "POST"),
      (307, "HEAD", "HEAD"),
    ] {
      assert_eq!(
        action(status, Some("/t"), method, false),
        follow("app://com.example.app/t", expected),
        "{status} {method}"
      );
    }
    // Navigations that end in a GET get the navigation page; a 307 / 308 of
    // a POST keeps its body, so the bridge follows it.
    for (status, method) in
      [(301, "GET"), (302, "POST"), (303, "POST"), (307, "GET")]
    {
      assert_eq!(
        action(status, Some("/t"), method, true),
        Navigate("app://com.example.app/t".to_string()),
        "{status} {method}"
      );
    }
    assert_eq!(
      action(307, Some("/t"), "POST", true),
      follow("app://com.example.app/t", "POST")
    );
    // Not handled: other statuses, no or a broken Location, other origins.
    for status in [200, 300, 304, 305, 306, 400] {
      assert_eq!(action(status, Some("/t"), "GET", false), PassThrough);
    }
    for location in [
      None,
      Some("https://elsewhere.example/"),
      Some("http://com.example.app/t"),
      Some("//elsewhere.example/t"),
      Some("app://com.example.app.evil/t"),
      Some("app://com.example.app:8080/t"),
      Some("app://user@com.example.app/t"),
      Some("app://COM.EXAMPLE.APP/t"),
      Some("app://other.example/t"),
      Some("data:text/html,hi"),
    ] {
      assert_eq!(
        action(302, location, "GET", false),
        PassThrough,
        "{location:?}"
      );
      assert_eq!(
        action(302, location, "GET", true),
        PassThrough,
        "{location:?}"
      );
    }
  }

  #[test]
  fn navigations_are_told_from_fetches() {
    assert!(is_navigation(&navigation_headers()));
    assert!(is_navigation(&h(&[("Sec-Fetch-Mode", "navigate")])));
    // A fetch, even one asking for HTML, and a subresource.
    assert!(!is_navigation(&h(&[("Accept", "*/*")])));
    assert!(!is_navigation(&h(&[("Accept", "text/html")])));
    assert!(!is_navigation(&h(&[("Upgrade-Insecure-Requests", "1")])));
    // Sec-Fetch-Mode decides when it is there.
    let mut headers = navigation_headers();
    headers.push(("sec-fetch-mode".to_string(), "cors".to_string()));
    assert!(!is_navigation(&headers));
  }

  #[test]
  fn the_navigation_page_cannot_be_escaped() {
    let target = "app://com.example.app/a?q=</script><script>alert(1)</script>&x=\"'\u{2028}\u{2029}<!--";
    let page = navigation_page(target);
    // One script element, and only the bridge's markup.
    assert_eq!(page.matches("<script").count(), 1);
    assert_eq!(page.matches("</script").count(), 1);
    assert!(!page.contains("<!--"));
    // The script navigates to exactly the target, and has no line
    // terminators older engines end a string literal at.
    let start =
      page.find("location.replace(").unwrap() + "location.replace(".len();
    let end = page.find(");</script>").unwrap();
    assert!(!page[start..end].contains(['\u{2028}', '\u{2029}']));
    let json: String = serde_json::from_str(&page[start..end]).unwrap();
    assert_eq!(json, target);
    // The refresh URL stays inside its quotes and its attribute.
    let content = page.split("content=\"").nth(1).unwrap();
    let content = &content[..content.find('"').unwrap()];
    assert!(!content.contains(['<', '>', '\'']));
    assert_eq!(content.matches("&quot;").count(), 2);
    assert!(content.contains("%22"));

    // A target the URL parser serialized has none of that left.
    let origin = origin();
    let RedirectAction::Navigate(target) = redirect_action(
      303,
      Some("/a?q=</script><b>\"x\" y"),
      "app://com.example.app/",
      "POST",
      true,
      &origin,
    ) else {
      panic!("not a navigation");
    };
    assert_eq!(
      target,
      "app://com.example.app/a?q=%3C/script%3E%3Cb%3E%22x%22%20y"
    );
  }

  #[tokio::test]
  async fn bridge_follows_same_origin_redirects_of_fetches() {
    const HOST: &str = "bridge-follow.example";
    let origin = AppOrigin::new(HOST);
    let bridge = Bridge::new(origin.clone(), true);
    let mut app = redirecting_app(HOST);
    let headers = h(&[("Content-Type", "text/plain"), ("X-Kept", "1")]);

    // 303: the next hop is a GET without the body or its headers.
    let exchange = FakeExchange::with_body(b"payload");
    let url = format!("app://{HOST}/see-other");
    bridge.respond(0, "POST", &url, &headers, &exchange).await;
    assert_eq!(exchange.status.get(), Some(200));
    assert_eq!(exchange.text(), "GET /final HTTP/1.1");
    let got = received(&mut app, 2).await;
    let end = find_end_of_head(&got[0]).unwrap();
    assert!(got[0].starts_with(b"POST /see-other "));
    assert_eq!(&got[0][end..], b"payload");
    let end = find_end_of_head(&got[1]).unwrap();
    let head = &got[1][..end];
    assert_eq!(header_values(head, b"content-type").count(), 0);
    assert_eq!(header_values(head, b"x-kept").count(), 1);
    assert_eq!(
      header_values(head, b"host").collect::<Vec<_>>(),
      vec![HOST.as_bytes()]
    );
    assert!(got[1][end..].is_empty());

    // 302 after a POST, to an absolute `http://<host>` URL: a GET too.
    let exchange = FakeExchange::with_body(b"payload");
    let url = format!("app://{HOST}/found");
    bridge.respond(0, "POST", &url, &headers, &exchange).await;
    assert_eq!(exchange.text(), "GET /final HTTP/1.1");
    received(&mut app, 2).await;

    // 307 and 308 keep the method, the body and its headers.
    for (path, target) in
      [("/temporary", "/final"), ("/permanent", "/final?x=1")]
    {
      let exchange = FakeExchange::with_body(b"payload");
      let url = format!("app://{HOST}{path}");
      bridge.respond(0, "POST", &url, &headers, &exchange).await;
      assert_eq!(exchange.status.get(), Some(200), "{path}");
      assert_eq!(exchange.text(), format!("POST {target} HTTP/1.1"));
      let got = received(&mut app, 2).await;
      let end = find_end_of_head(&got[1]).unwrap();
      assert_eq!(&got[1][end..], b"payload", "{path}");
      assert_eq!(header_values(&got[1][..end], b"content-type").count(), 1);
    }

    // A cross-origin redirect goes to the webview as it is.
    let exchange = FakeExchange::default();
    let url = format!("app://{HOST}/away");
    bridge.respond(0, "GET", &url, &[], &exchange).await;
    assert_eq!(exchange.status.get(), Some(302));
    assert_eq!(
      exchange.header("location").as_deref(),
      Some("https://elsewhere.example/x")
    );
    received(&mut app, 1).await;
  }

  #[tokio::test]
  async fn bridge_stops_a_redirect_loop() {
    const HOST: &str = "bridge-loop.example";
    let bridge = Bridge::new(AppOrigin::new(HOST), true);
    let mut app = redirecting_app(HOST);
    let exchange = FakeExchange::default();
    let url = format!("app://{HOST}/loop");
    bridge.respond(0, "GET", &url, &[], &exchange).await;
    assert_eq!(exchange.status.get(), Some(502));
    assert_eq!(exchange.text(), "too many redirects\n");
    // The first request and 20 redirects.
    received(&mut app, MAX_REDIRECTS + 1).await;
  }

  #[tokio::test]
  async fn bridge_turns_a_navigation_redirect_into_a_page() {
    const HOST: &str = "bridge-navigate.example";
    let bridge = Bridge::new(AppOrigin::new(HOST), true);
    let mut app = redirecting_app(HOST);
    let headers = navigation_headers();

    // A form POST answered 303: the app is asked once, and the window gets
    // a page that navigates to the target.
    let exchange = FakeExchange::with_body(b"a=b");
    let url = format!("app://{HOST}/see-other");
    bridge.respond(1, "POST", &url, &headers, &exchange).await;
    assert_eq!(exchange.status.get(), Some(200));
    assert_eq!(
      exchange.header("content-type").as_deref(),
      Some("text/html; charset=utf-8")
    );
    assert!(
      exchange
        .text()
        .contains(&format!("location.replace(\"app://{HOST}/final\")"))
    );
    received(&mut app, 1).await;
    // The window then loads the target, which isn't redirected.
    let exchange = FakeExchange::default();
    let landing = format!("app://{HOST}/final");
    bridge
      .respond(1, "GET", &landing, &headers, &exchange)
      .await;
    assert_eq!(exchange.text(), "GET /final HTTP/1.1");

    // A navigation loop stops after 20 redirects in a row in that window
    // (each navigation is its own request)...
    let url = format!("app://{HOST}/loop");
    for i in 1..=MAX_REDIRECTS {
      let exchange = FakeExchange::default();
      bridge.respond(1, "GET", &url, &headers, &exchange).await;
      assert_eq!(exchange.status.get(), Some(200), "redirect {i}");
    }
    // ...but not in another window.
    let exchange = FakeExchange::default();
    bridge.respond(2, "GET", &url, &headers, &exchange).await;
    assert_eq!(exchange.status.get(), Some(200));
    let exchange = FakeExchange::default();
    bridge.respond(1, "GET", &url, &headers, &exchange).await;
    assert_eq!(exchange.status.get(), Some(502));
    assert_eq!(exchange.text(), "too many redirects\n");

    // A navigation that isn't redirected starts the count over.
    for _ in 0..MAX_REDIRECTS {
      let exchange = FakeExchange::default();
      bridge.respond(2, "GET", &url, &headers, &exchange).await;
    }
    let exchange = FakeExchange::default();
    let landing = format!("app://{HOST}/final");
    bridge
      .respond(2, "GET", &landing, &headers, &exchange)
      .await;
    assert_eq!(exchange.status.get(), Some(200));
    let exchange = FakeExchange::default();
    bridge.respond(2, "GET", &url, &headers, &exchange).await;
    assert_eq!(exchange.status.get(), Some(200));
    assert!(exchange.text().contains("location.replace("));
  }

  #[tokio::test]
  async fn bridge_passes_redirects_through_without_redirect_handling() {
    const HOST: &str = "bridge-no-follow.example";
    let bridge = Bridge::new(AppOrigin::new(HOST), false);
    let mut app = redirecting_app(HOST);
    for headers in [vec![], navigation_headers()] {
      let exchange = FakeExchange::default();
      let url = format!("app://{HOST}/found");
      bridge.respond(0, "GET", &url, &headers, &exchange).await;
      assert_eq!(exchange.status.get(), Some(302));
      assert_eq!(
        exchange.header("location").as_deref(),
        Some(format!("app://{HOST}/final").as_str())
      );
    }
    received(&mut app, 2).await;
  }

  #[tokio::test]
  async fn bridge_stops_reading_the_app_once_the_webview_is_gone() {
    let origin = AppOrigin::new("bridge-gone.example");
    let mut app = fake_app(
      origin.host(),
      b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\nhello world",
    );
    let exchange = FakeExchange::with_writes(&[4, -1]);
    respond("GET", "app://bridge-gone.example/", &[], &origin, &exchange).await;
    assert_eq!(exchange.status.get(), Some(200));
    assert_eq!(exchange.text(), "hell");
    app.recv().await.unwrap();
  }

  #[test]
  fn a_body_read_error_is_never_forwarded_as_complete() {
    let read = |script: &[isize], method| {
      read_request_body(&FakeExchange::with_body_script(script), method)
    };
    assert_eq!(read(&[3, 4], "POST").unwrap().len(), 7);
    assert_eq!(read(&[], "POST").unwrap().len(), 0);
    // An error after some bytes, or before any on a method with a body.
    for script in [&[3, -1][..], &[-1], &[5, 5, -2]] {
      for method in ["POST", "PUT", "PATCH", "DELETE", "GET"] {
        if method == "GET" && script == [-1] {
          continue;
        }
        assert_eq!(
          read(script, method),
          Err(BodyError::Unreadable),
          "{method} {script:?}"
        );
      }
    }
    // A bodiless request on a backend that can't read bodies.
    for method in ["GET", "head", "OPTIONS"] {
      assert_eq!(read(&[-1], method), Ok(vec![]));
    }
    // The cap.
    assert_eq!(MAX_BRIDGE_REQUEST_BODY, 4 * 1024 * 1024);
    let chunks = vec![16 * 1024; MAX_BRIDGE_REQUEST_BODY / (16 * 1024)];
    assert_eq!(
      read(&chunks, "POST").unwrap().len(),
      MAX_BRIDGE_REQUEST_BODY
    );
    let mut chunks = chunks;
    chunks.push(1);
    assert_eq!(read(&chunks, "POST"), Err(BodyError::TooLarge));
  }

  #[tokio::test]
  async fn short_and_refused_writes_deliver_the_whole_chunk() {
    // The webview takes 3 bytes, nothing twice, then the rest: every byte
    // arrives (a short write used to lose the remainder).
    let sink = FakeExchange::with_writes(&[3, 0, 0, 2]);
    assert!(write_all(&sink, b"hello world").await);
    assert_eq!(sink.text(), "hello world");
    assert_eq!(sink.refused.get(), 2);
    // A webview that went away stops the copy.
    let gone = FakeExchange::with_writes(&[4, -1]);
    assert!(!write_all(&gone, b"hello world").await);
    assert_eq!(gone.text(), "hell");
  }

  #[test]
  fn request_headers_the_bridge_recomputes_are_not_forwarded() {
    for name in [
      "Content-Length",
      "expect",
      "host",
      "Connection",
      "transfer-encoding",
      "upgrade",
      "x-ende-desktop-relay",
      " X-ENDE-DESKTOP-RELAY ",
      "X-Ende-Desktop-Cross-Origin",
    ] {
      assert!(should_skip_request_header(name), "{name:?}");
    }
    for name in [
      "content-type",
      "cookie",
      "origin",
      "accept",
      "x-ende-desktop-relay-x",
    ] {
      assert!(!should_skip_request_header(name), "{name}");
    }
  }

  #[test]
  fn response_headers_keep_non_ascii_and_point_at_the_app_origin() {
    let o = origin();
    assert_eq!(response_header_value("x-name", b"caf\xe9", &o), "café");
    for (name, value, expected) in [
      (
        "Location",
        "http://com.example.app/login?x=1",
        "app://com.example.app/login?x=1",
      ),
      (
        "location",
        "http://com.example.app",
        "app://com.example.app",
      ),
      (
        "location",
        "HTTP://COM.EXAMPLE.APP/a",
        "app://com.example.app/a",
      ),
      (
        "content-location",
        "http://com.example.app?q",
        "app://com.example.app?q",
      ),
      (
        "refresh",
        "5; url=http://com.example.app/next",
        "5; url=app://com.example.app/next",
      ),
      (
        "refresh",
        "0;URL='http://com.example.app/q'",
        "0;URL='app://com.example.app/q'",
      ),
      (
        "location",
        "http://com.example.app/données?q=日本",
        "app://com.example.app/données?q=日本",
      ),
    ] {
      assert_eq!(
        response_header_value(name, value.as_bytes(), &o),
        expected,
        "{name}: {value}"
      );
    }
    // Anything else is left alone, and nothing panics on characters
    // across a prefix length.
    for (name, value) in [
      ("location", "/relative"),
      ("location", "https://idp.example/authorize"),
      ("location", "https://com.example.app/"),
      ("location", "http://com.example.app.evil/"),
      ("location", "http://com.example.app:8080/"),
      ("location", "http://user@com.example.app/"),
      ("location", "http://other.example/"),
      ("refresh", "5"),
      ("link", "<http://com.example.app/x>; rel=preload"),
      ("location", "http:/éé"),
      ("location", "ééééééééééééééé"),
      ("location", "日本語のパス/x"),
      ("location", "http://com.example.ap日本"),
      ("refresh", "5; url=ééééééééééééééé"),
      ("refresh", "5; url='日本語のパス'"),
      ("refresh", "é; url=/x"),
      ("refresh", "5; url='ééé"),
    ] {
      assert_eq!(response_header_value(name, value.as_bytes(), &o), value);
    }
  }

  #[test]
  fn strip_prefix_ignore_ascii_case_never_splits_a_character() {
    assert_eq!(
      strip_prefix_ignore_ascii_case("HTTP://x", "http://"),
      Some("x")
    );
    assert_eq!(strip_prefix_ignore_ascii_case("http:/", "http://"), None);
    assert_eq!(strip_prefix_ignore_ascii_case("", "http://"), None);
    for text in [
      "éééééééééé",
      "日本語日本語日本語",
      "😀😀😀😀😀",
      "aé日😀aé日😀",
    ] {
      for len in 1..=text.len() {
        let prefix = "x".repeat(len);
        assert_eq!(strip_prefix_ignore_ascii_case(text, &prefix), None);
      }
    }
  }

  #[test]
  fn path_and_query_extraction() {
    assert_eq!(path_and_query("app://h/foo?bar=baz"), "/foo?bar=baz");
    assert_eq!(path_and_query("app://h"), "/");
    assert_eq!(path_and_query("app://h/"), "/");
    assert_eq!(path_and_query("app://h:5173/foo"), "/foo");
    assert_eq!(path_and_query("app://h?x=1"), "/?x=1");
    assert_eq!(path_and_query("app://h/p?x=1#frag"), "/p?x=1");
    assert_eq!(path_and_query("app://h#frag"), "/");
    assert_eq!(path_and_query("/relative"), "/relative");
  }

  #[test]
  fn bridge_serves_exactly_the_origin_host() {
    let o = origin();
    for url in [
      "app://com.example.app/",
      "app://com.example.app",
      "app://com.example.app/x/y?z=1#f",
      "app://com.example.app?z",
    ] {
      assert!(authority_matches(url, &o), "{url}");
    }
    for url in [
      "app://evil/",
      "app://com.example.app.evil/",
      "app://com.example.app:8080/",
      "app://user@com.example.app/",
      // An opaque host is compared byte for byte by the browser too.
      "app://COM.EXAMPLE.APP/",
      "app:///",
      "/relative",
    ] {
      assert!(!authority_matches(url, &o), "{url}");
    }
  }

  #[test]
  fn requests_from_another_origin_are_marked() {
    let o = origin();
    for same in [
      h(&[]),
      h(&[("Origin", "app://com.example.app")]),
      h(&[("origin", " app://com.example.app ")]),
      h(&[("Sec-Fetch-Site", "same-origin")]),
      h(&[("sec-fetch-site", "none")]),
      h(&[
        ("Origin", "app://com.example.app"),
        ("Sec-Fetch-Site", "SAME-ORIGIN"),
      ]),
      h(&[("Accept", "*/*")]),
    ] {
      assert!(!is_cross_origin_request(&same, &o), "{same:?}");
    }
    for cross in [
      h(&[("Origin", "https://evil.example")]),
      h(&[("Origin", "null")]),
      h(&[("Origin", "app://other.example")]),
      h(&[("Origin", "APP://COM.EXAMPLE.APP")]),
      h(&[("Origin", "app://com.example.app/")]),
      h(&[("Origin", "http://com.example.app")]),
      h(&[("Origin", "")]),
      h(&[("Sec-Fetch-Site", "cross-site")]),
      h(&[("Sec-Fetch-Site", "same-site")]),
      h(&[
        ("Origin", "app://com.example.app"),
        ("Sec-Fetch-Site", "cross-site"),
      ]),
      h(&[
        ("Origin", "app://com.example.app"),
        ("Origin", "https://evil.example"),
      ]),
    ] {
      assert!(is_cross_origin_request(&cross, &o), "{cross:?}");
    }
  }

  #[test]
  fn app_origin_urls() {
    let o = origin();
    assert_eq!(o.to_string(), "app://com.example.app");
    assert_eq!(o.root_url(), "app://com.example.app/");
  }

  // --- the WebSocket relay --------------------------------------------------

  /// A connected (client, relay-side) TCP pair on loopback.
  async fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (client, accepted) =
      tokio::join!(TcpStream::connect(addr), listener.accept());
    (client.unwrap(), accepted.unwrap().0)
  }

  /// `/.ende-desktop-relay/<this launch's token><rest>`.
  fn token_path(rest: &str) -> String {
    format!("{RELAY_PATH_PREFIX}{}{rest}", relay_token().unwrap())
  }

  fn upgrade_head_to(target: &str, extra_headers: &str) -> Vec<u8> {
    format!(
      "GET {target} HTTP/1.1\r\n\
       Host: 127.0.0.1:1234\r\n\
       Upgrade: websocket\r\n\
       Connection: Upgrade\r\n\
       Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
       Sec-WebSocket-Version: 13\r\n\
       {extra_headers}\r\n"
    )
    .into_bytes()
  }

  /// A WebSocket handshake to `/ws` with this launch's token.
  fn upgrade_head(extra_headers: &str) -> Vec<u8> {
    upgrade_head_to(&token_path("/ws"), extra_headers)
  }

  const APP: &str = "Origin: app://com.example.app\r\n";

  #[tokio::test]
  async fn head_read_times_out_on_an_idle_socket() {
    // A preconnect-style socket that never writes: the read gives up at the
    // deadline instead of holding the task forever.
    let (_client, mut server) = tcp_pair().await;
    let start = std::time::Instant::now();
    let head = tokio::time::timeout(
      Duration::from_secs(5),
      read_request_head(&mut server, Duration::from_millis(200)),
    )
    .await
    .expect("the head read has its own deadline")
    .unwrap();
    assert!(head.is_none());
    assert!(start.elapsed() >= Duration::from_millis(200));
    assert_eq!(HEAD_READ_TIMEOUT, Duration::from_secs(10));
  }

  #[tokio::test]
  async fn head_read_deadline_covers_a_trickling_client() {
    // Bytes that never complete a head don't extend the deadline.
    let (mut client, mut server) = tcp_pair().await;
    let writer = tokio::spawn(async move {
      for _ in 0..50 {
        if client.write_all(b"G").await.is_err() {
          break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
      }
      client
    });
    let head = tokio::time::timeout(
      Duration::from_millis(900),
      read_request_head(&mut server, Duration::from_millis(200)),
    )
    .await
    .expect("the deadline covers the whole head")
    .unwrap();
    assert!(head.is_none());
    drop(writer.await);
  }

  #[tokio::test]
  async fn head_read_returns_a_complete_head_and_refuses_large_ones() {
    let (mut client, mut server) = tcp_pair().await;
    client
      .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\nextra")
      .await
      .unwrap();
    let (head, end) = read_request_head(&mut server, Duration::from_secs(5))
      .await
      .unwrap()
      .expect("a complete head");
    assert_eq!(&head[..end], b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    // A client that closes before finishing its head yields nothing.
    let (client, mut server) = tcp_pair().await;
    drop(client);
    assert!(
      read_request_head(&mut server, Duration::from_secs(5))
        .await
        .unwrap()
        .is_none()
    );
    // Nor does one that sends more than the cap without finishing it.
    let (mut client, mut server) = tcp_pair().await;
    client
      .write_all(&vec![b'a'; MAX_HEAD_LEN + 1])
      .await
      .unwrap();
    assert!(
      read_request_head(&mut server, Duration::from_secs(5))
        .await
        .unwrap()
        .is_none()
    );
  }

  #[test]
  fn find_end_of_head_handles_split() {
    assert_eq!(find_end_of_head(b"GET / HTTP/1.1\r\n\r\n"), Some(18));
    assert_eq!(
      find_end_of_head(b"GET / HTTP/1.1\r\nHost: x\r\n\r\ntail"),
      Some(27),
    );
    assert_eq!(find_end_of_head(b"GET / HTTP/1.1\r\n"), None);
  }

  #[test]
  fn header_values_are_case_insensitive_and_ordered() {
    let head = b"GET / HTTP/1.1\r\n\
      Host: 127.0.0.1:1\r\n\
      ORIGIN: a\r\n\
      X-Other: nope\r\n\
      origin:\t b \r\n\r\n";
    let values: Vec<&[u8]> = header_values(head, b"origin").collect();
    assert_eq!(values, vec![&b"a"[..], &b"b"[..]]);
    assert_eq!(header_values(head, b"host").count(), 1);
    assert_eq!(header_values(head, b"missing").count(), 0);
    // The request line is never taken for a header, colon or not.
    let head = b"GET /x?origin:1 HTTP/1.1\r\n\r\n";
    assert_eq!(header_values(head, b"origin").count(), 0);
  }

  #[test]
  fn recognises_websocket_upgrades_only() {
    assert!(is_websocket_upgrade(&upgrade_head("")));
    assert!(is_websocket_upgrade(
      b"GET / HTTP/1.1\r\nUPGRADE: WebSocket\r\n\r\n"
    ));
    assert!(is_websocket_upgrade(
      b"GET / HTTP/1.1\r\nupgrade: h2c, websocket\r\n\r\n"
    ));
    assert!(!is_websocket_upgrade(
      b"GET / HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n"
    ));
    assert!(!is_websocket_upgrade(
      b"POST /api HTTP/1.1\r\nUpgrade: h2c\r\n\r\n"
    ));
  }

  #[test]
  fn relay_admits_only_the_app_origin() {
    let o = origin();
    let token = relay_token();
    assert_eq!(
      classify_upgrade(&upgrade_head(APP), &o, token),
      RelayDecision::Proxy
    );
    // Header name case and surrounding whitespace don't matter …
    assert_eq!(
      classify_upgrade(
        &upgrade_head("ORIGIN:   app://com.example.app \r\n"),
        &o,
        token
      ),
      RelayDecision::Proxy
    );
    // … but the value is compared byte for byte.
    for foreign in [
      "https://evil.example",
      "http://127.0.0.1:1234",
      "http://com.example.app",
      "app://evil.example",
      "app://com.example.app.evil",
      "app://com.example.app/",
      "app://com.example.app:1",
      "APP://COM.EXAMPLE.APP",
      "null",
      "",
    ] {
      assert_eq!(
        classify_upgrade(
          &upgrade_head(&format!("Origin: {foreign}\r\n")),
          &o,
          token
        ),
        RelayDecision::ForbiddenOrigin,
        "Origin {foreign:?} must be refused"
      );
    }
  }

  #[test]
  fn relay_requires_exactly_one_origin_header() {
    let o = origin();
    let token = relay_token();
    // A browser always sends Origin on a WebSocket handshake.
    assert_eq!(
      classify_upgrade(&upgrade_head(""), &o, token),
      RelayDecision::ForbiddenOrigin
    );
    // Two Origins, in either order: the right one can't be smuggled next
    // to a wrong one.
    for extra in [
      "Origin: app://com.example.app\r\nOrigin: https://evil.example\r\n",
      "Origin: https://evil.example\r\nOrigin: app://com.example.app\r\n",
    ] {
      assert_eq!(
        classify_upgrade(&upgrade_head(extra), &o, token),
        RelayDecision::ForbiddenOrigin
      );
    }
    // Non-UTF-8 bytes are a mismatch, not a panic.
    let mut head = upgrade_head("");
    head.truncate(head.len() - 2);
    head.extend_from_slice(b"Origin: app://\xff\r\n\r\n");
    assert_eq!(
      classify_upgrade(&head, &o, token),
      RelayDecision::ForbiddenOrigin
    );
  }

  #[test]
  fn relay_refuses_non_websocket_requests_before_anything_else() {
    let o = origin();
    let token = relay_token();
    let target = token_path("/");
    for head in [
      format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:1\r\n{APP}\r\n"),
      // Only GET opens a WebSocket (RFC 6455 §4.1).
      format!("POST {target} HTTP/1.1\r\nUpgrade: websocket\r\n{APP}\r\n"),
      format!("GET {target} HTTP/1.1\r\nUpgrade: h2c\r\n{APP}\r\n"),
    ] {
      assert_eq!(
        classify_upgrade(head.as_bytes(), &o, token),
        RelayDecision::NotWebSocket,
        "{head}"
      );
    }
  }

  #[test]
  fn relay_requires_this_launchs_token_in_the_target() {
    let o = origin();
    let token = relay_token().unwrap();
    assert_eq!(token.len(), 64);
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(relay_token(), relay_token());
    // The right token and Origin: proxied, with the prefix removed.
    for (target, upstream) in [
      (token_path("/ws"), "/ws"),
      (token_path(""), "/"),
      (token_path("/"), "/"),
      (token_path("?room=1"), "/?room=1"),
      (token_path("/a/b?c=d"), "/a/b?c=d"),
    ] {
      let head = upgrade_head_to(&target, APP);
      assert_eq!(
        classify_upgrade(&head, &o, Some(token)),
        RelayDecision::Proxy,
        "{target}"
      );
      assert_eq!(
        relay_target(&head, Some(token)).as_deref(),
        Some(upstream.as_bytes()),
        "{target}"
      );
    }
    // The app origin isn't enough without the token: any local process
    // can send it.
    let wrong = format!("{RELAY_PATH_PREFIX}{}/ws", "0".repeat(64));
    let longer = token_path("0/ws");
    let upper = format!("{RELAY_PATH_PREFIX}{}/ws", token.to_uppercase());
    let short = format!("{RELAY_PATH_PREFIX}{}/ws", &token[..63]);
    let encoded = format!("/.ende-desktop-relay%2F{token}/ws");
    let absolute = format!("http://127.0.0.1:1{}", token_path("/ws"));
    for target in [
      "/ws",
      "/",
      RELAY_PATH_PREFIX,
      &wrong,
      &longer,
      &upper,
      &short,
      &encoded,
      &absolute,
    ] {
      assert_eq!(
        classify_upgrade(&upgrade_head_to(target, APP), &o, Some(token)),
        RelayDecision::ForbiddenToken,
        "{target}"
      );
    }
    // No token could be drawn: nothing is admitted.
    assert_eq!(
      classify_upgrade(&upgrade_head(APP), &o, None),
      RelayDecision::ForbiddenToken
    );
    // The token doesn't excuse a foreign Origin.
    assert_eq!(
      classify_upgrade(
        &upgrade_head("Origin: https://evil.example\r\n"),
        &o,
        Some(token)
      ),
      RelayDecision::ForbiddenOrigin
    );
    // A request line with extra or missing parts is refused.
    let mut bad = upgrade_head(APP);
    bad.splice(3..4, b"  ".iter().copied());
    assert_eq!(
      classify_upgrade(&bad, &o, Some(token)),
      RelayDecision::ForbiddenToken
    );
  }

  #[test]
  fn constant_time_eq_is_equality() {
    assert!(constant_time_eq(b"", b""));
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
  }

  #[test]
  fn the_relay_url_carries_the_token() {
    let addr: SocketAddr = "127.0.0.1:4321".parse().unwrap();
    assert_eq!(
      ws_relay_url(addr).unwrap(),
      format!(
        "ws://127.0.0.1:4321/.ende-desktop-relay/{}",
        relay_token().unwrap()
      )
    );
  }

  #[test]
  fn relay_rewrites_the_head_it_forwards() {
    // The client's markers, in any case and with any value, and its Host
    // are dropped; the relay's own are the only ones upstream sees.
    let head = upgrade_head(
      "Origin: app://com.example.app\r\nX-Ende-Desktop-Relay: 0\r\n\
       x-ende-desktop-relay : spoof\r\nHOST: evil.example\r\n",
    );
    let out = relay_request_head(&head, b"/ws", "com.example.app");
    assert_eq!(
      header_values(&out, RELAY_MARKER_HEADER.as_bytes()).collect::<Vec<_>>(),
      vec![b"1".as_slice()]
    );
    assert_eq!(
      header_values(&out, b"host").collect::<Vec<_>>(),
      vec![b"com.example.app".as_slice()]
    );
    // The relay's prefix and token never reach the app.
    assert!(out.starts_with(b"GET /ws HTTP/1.1\r\n"));
    assert!(!String::from_utf8_lossy(&out).contains(relay_token().unwrap()));
    assert_eq!(find_end_of_head(&out), Some(out.len()));
    // Everything else the client sent is kept.
    assert_eq!(header_values(&out, b"origin").count(), 1);
    assert_eq!(header_values(&out, b"sec-websocket-key").count(), 1);
  }

  #[test]
  fn switching_protocols_status_is_exact() {
    assert!(is_switching_protocols(
      b"HTTP/1.1 101 Switching Protocols\r\n\r\n"
    ));
    assert!(is_switching_protocols(b"HTTP/1.1 101\r\n\r\n"));
    for other in [
      &b"HTTP/1.1 200 OK\r\n\r\n"[..],
      b"HTTP/1.1 1010 x\r\n\r\n",
      b"HTTP/1.0 101 x\r\n\r\n",
      b"",
    ] {
      assert!(!is_switching_protocols(other));
    }
  }

  /// Runs [`relay_upgrade`] between in-memory pipes. The server answers the
  /// forwarded head with `server_answer`; returns what the client read and
  /// what the server read.
  async fn run_relay(
    client_sends: &[u8],
    server_answer: &'static [u8],
  ) -> (Vec<u8>, Vec<u8>) {
    let (mut client, mut relay_client_side) = tokio::io::duplex(64 * 1024);
    let (mut relay_upstream_side, mut server) = tokio::io::duplex(64 * 1024);
    client.write_all(client_sends).await.unwrap();
    let end = find_end_of_head(client_sends).unwrap();
    let head = relay_request_head(&client_sends[..end], b"/ws", "h.example");
    let relay = tokio::spawn(async move {
      relay_upgrade(
        &mut relay_client_side,
        &mut relay_upstream_side,
        &head,
        Duration::from_secs(5),
      )
      .await
      .unwrap();
    });
    let server_task = tokio::spawn(async move {
      let (mut got, _) = read_request_head(&mut server, Duration::from_secs(5))
        .await
        .unwrap()
        .expect("the forwarded head");
      server.write_all(server_answer).await.unwrap();
      let mut buf = [0u8; 1024];
      while let Ok(Ok(n)) =
        tokio::time::timeout(Duration::from_millis(300), server.read(&mut buf))
          .await
      {
        if n == 0 {
          break;
        }
        got.extend_from_slice(&buf[..n]);
      }
      got
    });
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    while let Ok(Ok(n)) =
      tokio::time::timeout(Duration::from_millis(500), client.read(&mut buf))
        .await
    {
      if n == 0 {
        break;
      }
      got.extend_from_slice(&buf[..n]);
    }
    drop(client);
    let upstream = server_task.await.unwrap();
    relay.abort();
    (got, upstream)
  }

  #[tokio::test]
  async fn relay_never_carries_a_non_101_response() {
    let mut sends = upgrade_head(APP);
    // A second request pipelined behind the upgrade.
    sends.extend_from_slice(b"GET /second HTTP/1.1\r\nHost: x\r\n\r\n");
    let (client_got, upstream_got) = run_relay(
      &sends,
      b"HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\nsecret",
    )
    .await;
    let text = String::from_utf8_lossy(&client_got);
    assert!(text.starts_with("HTTP/1.1 502 "), "{text}");
    assert!(
      !text.contains("200 OK") && !text.contains("secret"),
      "{text}"
    );
    // Only the upgrade head went upstream; the pipelined request never did.
    assert_eq!(find_end_of_head(&upstream_got), Some(upstream_got.len()));
    assert!(!String::from_utf8_lossy(&upstream_got).contains("/second"));
  }

  #[tokio::test]
  async fn relay_passes_a_101_and_what_follows() {
    let (client_got, _) = run_relay(
      &upgrade_head(APP),
      b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n\x81\x02hi",
    )
    .await;
    assert!(client_got.starts_with(b"HTTP/1.1 101 "));
    assert!(client_got.ends_with(b"\x81\x02hi"));
  }

  #[test]
  fn canned_responses_have_correct_content_length() {
    for resp in [
      RELAY_400_RESPONSE,
      RELAY_403_RESPONSE,
      RELAY_403_TOKEN_RESPONSE,
      RELAY_502_RESPONSE,
    ] {
      let text = std::str::from_utf8(resp).unwrap();
      let (head, body) = text.split_once("\r\n\r\n").unwrap();
      let declared: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .unwrap()
        .parse()
        .unwrap();
      assert_eq!(declared, body.len(), "{head}");
    }
  }

  #[test]
  fn ws_relay_binds_loopback_ephemeral() {
    let l = bind_ws_loopback_listener().unwrap();
    let addr = l.local_addr().unwrap();
    assert!(addr.ip().is_loopback());
    assert_ne!(addr.port(), 0);
  }

  /// Send `head` to the relay at `addr`; what it answers until the end of a
  /// response head or EOF.
  async fn exchange_with(addr: SocketAddr, head: &[u8]) -> String {
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    tcp.write_all(head).await.unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
      let n = tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut buf))
        .await
        .expect("the relay must answer")
        .unwrap();
      if n == 0 {
        break;
      }
      out.extend_from_slice(&buf[..n]);
      if find_end_of_head(&out).is_some() {
        break;
      }
    }
    String::from_utf8_lossy(&out).into_owned()
  }

  /// End to end over real sockets: a relay in front of a memory listener
  /// that answers every upgrade with `101`. Only the app origin with this
  /// launch's token gets through; everything else is refused at the relay
  /// and never reaches the server.
  #[tokio::test]
  async fn ws_relay_end_to_end_enforces_token_and_origin() {
    let origin = AppOrigin::new("relay-e2e.example");
    let server = listen_memory(origin.host()).unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepted_srv = accepted.clone();
    tokio::spawn(async move {
      while let Ok((mut stream, _)) = server.accept().await {
        accepted_srv.fetch_add(1, Ordering::SeqCst);
        let (buf, end) = read_request_head(&mut stream, Duration::from_secs(5))
          .await
          .unwrap()
          .unwrap();
        let head = &buf[..end];
        assert!(head.starts_with(b"GET /ws HTTP/1.1\r\n"));
        assert_eq!(
          header_values(head, RELAY_MARKER_HEADER.as_bytes()).count(),
          1
        );
        assert_eq!(
          header_values(head, b"host").collect::<Vec<_>>(),
          vec![b"relay-e2e.example".as_slice()]
        );
        // Nothing the client pipelined after its head.
        assert_eq!(header_values(head, b"x-second").count(), 0);
        assert_eq!(buf.len(), end);
        stream
          .write_all(b"HTTP/1.1 101 Switching Protocols\r\n\r\n")
          .await
          .unwrap();
      }
    });

    let listener = bind_ws_loopback_listener().unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_ws_loopback_proxy(listener, origin);
    let app = "Origin: app://relay-e2e.example\r\n";

    // Another page's Origin, a rebound page's included: refused.
    for foreign in ["https://evil.example", "http://127.0.0.1:1234"] {
      let resp =
        exchange_with(addr, &upgrade_head(&format!("Origin: {foreign}\r\n")))
          .await;
      assert!(resp.starts_with("HTTP/1.1 403 "), "{resp}");
    }
    // No Origin at all (a raw local client): refused.
    let resp = exchange_with(addr, &upgrade_head("")).await;
    assert!(resp.starts_with("HTTP/1.1 403 "), "{resp}");
    // The app origin without the token, or with another one: refused.
    for target in [
      "/ws".to_string(),
      format!("{RELAY_PATH_PREFIX}{}/ws", "0".repeat(64)),
    ] {
      let resp = exchange_with(addr, &upgrade_head_to(&target, app)).await;
      assert!(resp.starts_with("HTTP/1.1 403 "), "{resp}");
      assert!(resp.contains("relay token"), "{resp}");
    }
    // Plain HTTP with the token and the app origin: 400.
    let plain = format!(
      "GET {} HTTP/1.1\r\nHost: 127.0.0.1\r\n{app}\r\n",
      token_path("/ws")
    );
    let resp = exchange_with(addr, plain.as_bytes()).await;
    assert!(resp.starts_with("HTTP/1.1 400 "), "{resp}");
    assert_eq!(accepted.load(Ordering::SeqCst), 0);

    // The app's own page: through to the server's 101, without what it
    // pipelined after its head.
    let mut sends = upgrade_head(app);
    sends.extend_from_slice(b"GET /second HTTP/1.1\r\nX-Second: 1\r\n\r\n");
    let resp = exchange_with(addr, &sends).await;
    assert!(resp.starts_with("HTTP/1.1 101 "), "{resp}");
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
  }

  #[tokio::test]
  async fn ws_relay_drops_an_idle_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(run_ws_loopback_proxy(
      listener,
      AppOrigin::new("relay-idle.example"),
      Duration::from_millis(200),
    ));
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut buf))
      .await
      .expect("the relay must close an idle connection")
      .unwrap();
    // Closed without an HTTP answer.
    assert_eq!(n, 0);
  }
}
