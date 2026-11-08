// Copyright 2018-2026 the Deno authors. MIT license.

//! The cookie jar the `app://` bridge keeps for the app's origin on macOS.
//!
//! WKWebView keeps no cookies for a custom scheme: it ignores `Set-Cookie`
//! from a scheme handler and sends no `Cookie` to it. So the bridge does what
//! the browser would, for its one host: it stores the `Set-Cookie` of the
//! app's responses here and sends the matching cookies as `Cookie` on later
//! requests (see [`crate::scheme_bridge`]). Page JavaScript doesn't see the
//! jar: `document.cookie` stays empty.
//!
//! The rules are RFC 6265bis's, for a single host that is always a secure
//! origin:
//!
//! * `Domain` must be the app host itself (a leading dot and case ignored);
//!   a cookie for any other domain is refused.
//! * `Path`, `Expires` / `Max-Age` (capped at 400 days; a past date or
//!   `Max-Age<=0` deletes), `Secure` (kept: `app://` counts as secure),
//!   `HttpOnly` (kept, nothing else can read the jar), the `__Secure-` and
//!   `__Host-` prefixes, and `SameSite=None` only with `Secure`.
//! * A name and value of at most 4096 bytes, without control characters; at
//!   most [`MAX_COOKIES`] cookies, the least recently used evicted first. A
//!   nameless cookie (`Set-Cookie: value`) is ignored.
//! * `SameSite`, against what the bridge can tell about the document that
//!   made the request ([`Initiator`]): every cookie goes to a request the app
//!   origin made. Any other request gets cookies only with `GET` / `HEAD`,
//!   and then not the `SameSite=Strict` ones: a cross-origin one only when
//!   it's a navigation, as `Lax` allows, but one that tells nothing about
//!   its document ([`Initiator::Unknown`]) for any `GET` / `HEAD`, so the
//!   app's XHR and subresources get them (weaker than a browser's `Lax`:
//!   a remote page's `<img>` and `<script>` get them too). A header-less
//!   `POST` gets none. `SameSite=None` is treated as `Lax`, and no
//!   attribute means `Lax`. `Set-Cookie` is only taken from a response to a
//!   same-origin request or to a navigation.
//!
//! Persistent cookies are kept in one JSON file ([`cookie_file`]), written
//! (owner-only, temp file and rename) at most once per [`WRITE_DELAY`] after a
//! change, and when the app exits. Session cookies live only as long as the
//! process and are never written.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// The most cookies the jar keeps (Chromium's and Firefox's per-domain
/// limit; RFC 6265bis asks for at least 50).
pub const MAX_COOKIES: usize = 180;

/// The longest name plus value, in bytes (RFC 6265bis 5.6).
const MAX_NAME_VALUE_LEN: usize = 4096;

/// The longest attribute value honored, in bytes; a longer one is ignored
/// (RFC 6265bis 5.6).
const MAX_ATTRIBUTE_LEN: usize = 1024;

/// The latest expiry a cookie can ask for: 400 days (RFC 6265bis 5.6.1).
const MAX_LIFETIME_MS: i64 = 400 * 24 * 60 * 60 * 1000;

/// How long the jar waits after a change to a persistent cookie before it
/// writes the file, so a burst of changes is one write.
pub const WRITE_DELAY: Duration = Duration::from_secs(1);

/// The jar file's name, in the app's data folder.
const FILE_NAME: &str = "cookies.json";

/// The jar file of the app with this identifier:
/// `<data dir>/<identifier>/cookies.json`, next to the app's other
/// per-identifier data (on macOS `~/Library/Application Support`). `None`
/// when the identifier isn't a single, plain folder name.
pub fn cookie_file(data_dir: &Path, identifier: &str) -> Option<PathBuf> {
  let plain = !identifier.is_empty()
    && !identifier.starts_with('.')
    && identifier
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
  plain.then(|| data_dir.join(identifier).join(FILE_NAME))
}

/// What the bridge can tell about the document that made a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Initiator {
  /// A document of another origin (an `Origin` or `Referer` elsewhere, or
  /// `Origin: null`).
  CrossSite,
  /// Nothing to tell: no `Origin`, `Referer` or `Sec-Fetch-Site`. The app's
  /// own `navigate()`, its first page load, and its XHR and subresources
  /// look like this in WKWebView, but so do a remote page's images and
  /// scripts, and its `fetch(url, { referrer: "" })`, POSTs included.
  Unknown,
  /// A document on the app origin.
  SameOrigin,
}

/// The request a cookie is sent with or received from.
#[derive(Clone, Copy, Debug)]
pub struct RequestContext {
  pub initiator: Initiator,
  /// The webview loads it as a document.
  pub navigation: bool,
  /// `GET` or `HEAD`.
  pub safe_method: bool,
}

impl RequestContext {
  pub fn new(initiator: Initiator, navigation: bool, method: &str) -> Self {
    Self {
      initiator,
      navigation,
      safe_method: ["GET", "HEAD"]
        .iter()
        .any(|m| m.eq_ignore_ascii_case(method)),
    }
  }

  /// Whether the request may carry a cookie with this `SameSite`: every
  /// cookie for the app's own request; otherwise all but `Strict` ones for
  /// a `GET` / `HEAD` that is a navigation or tells nothing about its
  /// document (the app's XHR, images, scripts and stylesheets look like
  /// that, but so do a remote page's). A header-less `POST` gets none: a
  /// remote page can send one (`fetch(url, { referrer: "" })`).
  fn sends(&self, same_site: SameSite) -> bool {
    self.initiator == Initiator::SameOrigin
      || (same_site != SameSite::Strict
        && self.safe_method
        && (self.navigation || self.initiator == Initiator::Unknown))
  }

  /// Whether `Set-Cookie` of the response to the request is stored.
  fn stores(&self) -> bool {
    self.initiator == Initiator::SameOrigin || self.navigation
  }
}

#[derive(
  Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
enum SameSite {
  Strict,
  Lax,
  None,
  /// No (or an unknown) attribute: treated as `Lax`.
  Default,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCookie {
  name: String,
  value: String,
  path: String,
  /// Milliseconds since the Unix epoch; `None` for a session cookie.
  expires: Option<i64>,
  secure: bool,
  http_only: bool,
  same_site: SameSite,
  /// Set without a `Domain` attribute. The jar has one host, so it only
  /// matters for the `__Host-` prefix and is kept for completeness.
  host_only: bool,
  created: i64,
  last_access: i64,
}

impl StoredCookie {
  fn persistent(&self) -> bool {
    self.expires.is_some()
  }

  fn expired(&self, now: i64) -> bool {
    self.expires.is_some_and(|expires| expires <= now)
  }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CookieFile {
  version: u32,
  cookies: Vec<StoredCookie>,
}

const FILE_VERSION: u32 = 1;

#[derive(Default)]
struct State {
  /// In insertion order.
  cookies: Vec<StoredCookie>,
  /// Persistent cookies changed since the file was last written.
  dirty: bool,
}

pub struct CookieJar {
  host: String,
  file: Option<PathBuf>,
  state: Mutex<State>,
  /// A write is scheduled ([`WRITE_DELAY`]).
  write_scheduled: AtomicBool,
  /// Held while the file is written, so writes don't interleave.
  write_lock: Mutex<()>,
  /// A failed write was logged already.
  write_failed: AtomicBool,
}

impl CookieJar {
  /// The jar of the app served at `app://<host>`, persisted to `file` (see
  /// [`cookie_file`]) or, without one, kept in memory only. Loads the
  /// persistent cookies from `file`; a missing or unreadable file starts an
  /// empty jar.
  pub fn open(host: &str, file: Option<PathBuf>) -> Arc<Self> {
    let cookies = file
      .as_deref()
      .map(|file| load(file, now_ms()))
      .unwrap_or_default();
    Arc::new(Self {
      host: host.to_string(),
      file,
      state: Mutex::new(State {
        cookies,
        dirty: false,
      }),
      write_scheduled: AtomicBool::new(false),
      write_lock: Mutex::new(()),
      write_failed: AtomicBool::new(false),
    })
  }

  /// The `Cookie` header value for a request to `path` (no query), or
  /// `None` when no cookie goes with it.
  pub fn cookie_header(
    &self,
    path: &str,
    context: RequestContext,
  ) -> Option<String> {
    self.cookie_header_at(path, context, now_ms())
  }

  fn cookie_header_at(
    &self,
    path: &str,
    context: RequestContext,
    now: i64,
  ) -> Option<String> {
    let mut state = self.state.lock().unwrap();
    state.cookies.retain(|c| !c.expired(now));
    let mut matching = state
      .cookies
      .iter_mut()
      .filter(|c| path_matches(&c.path, path) && context.sends(c.same_site))
      .collect::<Vec<_>>();
    if matching.is_empty() {
      return None;
    }
    // Longer paths first, then the older cookie (RFC 6265bis 5.8.3).
    matching.sort_by(|a, b| {
      b.path
        .len()
        .cmp(&a.path.len())
        .then(a.created.cmp(&b.created))
    });
    let mut header = String::new();
    for cookie in matching {
      cookie.last_access = now;
      if !header.is_empty() {
        header.push_str("; ");
      }
      header.push_str(&cookie.name);
      header.push('=');
      header.push_str(&cookie.value);
    }
    Some(header)
  }

  /// Store the `Set-Cookie` header values of the response to a request for
  /// `path` (no query). Values that aren't UTF-8 are ignored. Schedules a
  /// write of the file when a persistent cookie changed.
  pub fn store<'a>(
    self: &Arc<Self>,
    path: &str,
    set_cookie: impl IntoIterator<Item = &'a [u8]>,
    context: RequestContext,
  ) {
    let now = now_ms();
    let mut changed = false;
    for value in set_cookie {
      if let Ok(value) = std::str::from_utf8(value) {
        changed |= self.store_at(path, value, context, now);
      }
    }
    if changed {
      self.schedule_write();
    }
  }

  /// Store one `Set-Cookie` value. True when a persistent cookie was added,
  /// changed or removed.
  fn store_at(
    &self,
    request_path: &str,
    set_cookie: &str,
    context: RequestContext,
    now: i64,
  ) -> bool {
    if !context.stores() {
      return false;
    }
    let Some(cookie) =
      parse_set_cookie(set_cookie, request_path, &self.host, now)
    else {
      return false;
    };
    let mut state = self.state.lock().unwrap();
    state.cookies.retain(|c| !c.expired(now));
    let mut changed = false;
    let mut created = now;
    if let Some(i) = state
      .cookies
      .iter()
      .position(|c| c.name == cookie.name && c.path == cookie.path)
    {
      let old = state.cookies.remove(i);
      changed |= old.persistent();
      created = old.created;
    }
    if !cookie.expired(now) {
      changed |= cookie.persistent();
      state.cookies.push(StoredCookie {
        created,
        last_access: now,
        ..cookie
      });
      // The least recently used go first; never the one just stored.
      while state.cookies.len() > MAX_COOKIES {
        let candidates = state.cookies.len() - 1;
        let Some(lru) = state.cookies[..candidates]
          .iter()
          .enumerate()
          .min_by_key(|(_, c)| (c.last_access, c.created))
          .map(|(i, _)| i)
        else {
          break;
        };
        changed |= state.cookies.remove(lru).persistent();
      }
    }
    state.dirty |= changed;
    changed
  }

  /// Write the file a little later, unless a write is scheduled already.
  /// Without a tokio runtime, write now.
  fn schedule_write(self: &Arc<Self>) {
    if self.file.is_none() || self.write_scheduled.swap(true, Ordering::AcqRel)
    {
      return;
    }
    let jar = self.clone();
    match tokio::runtime::Handle::try_current() {
      Ok(handle) => {
        handle.spawn(async move {
          tokio::time::sleep(WRITE_DELAY).await;
          // Cleared before the snapshot is taken: a change after this point
          // schedules another write.
          jar.write_scheduled.store(false, Ordering::Release);
          let _ = tokio::task::spawn_blocking(move || jar.flush()).await;
        });
      }
      Err(_) => {
        jar.write_scheduled.store(false, Ordering::Release);
        jar.flush();
      }
    }
  }

  /// Write the persistent cookies to the file now, if they changed since
  /// the last write.
  pub fn flush(&self) {
    let Some(file) = &self.file else {
      return;
    };
    let _writing = self.write_lock.lock().unwrap();
    let snapshot = {
      let mut state = self.state.lock().unwrap();
      if !state.dirty {
        return;
      }
      state.dirty = false;
      let now = now_ms();
      CookieFile {
        version: FILE_VERSION,
        cookies: state
          .cookies
          .iter()
          .filter(|c| c.persistent() && !c.expired(now))
          .cloned()
          .collect(),
      }
    };
    match write_file(file, &snapshot) {
      Ok(()) => {
        self.write_failed.store(false, Ordering::Relaxed);
      }
      Err(e) => {
        // Written again with the next change.
        self.state.lock().unwrap().dirty = true;
        if !self.write_failed.swap(true, Ordering::Relaxed) {
          log::warn!(
            "[desktop] could not save the app's cookies to {}: {e}",
            file.display()
          );
        }
      }
    }
  }
}

/// The jar whose file is written when the app exits.
static EXIT_JAR: OnceLock<Arc<CookieJar>> = OnceLock::new();

/// Write `jar`'s file when the app exits (`Ende.exit()`, or the backend
/// quitting: [`flush_on_exit`]), so a change in the last [`WRITE_DELAY`]
/// isn't lost.
pub fn flush_at_exit(jar: &Arc<CookieJar>) {
  if EXIT_JAR.set(jar.clone()).is_ok() {
    ende_signals::before_exit(flush_on_exit);
  }
}

/// Write the file of the jar given to [`flush_at_exit`], if any.
pub fn flush_on_exit() {
  if let Some(jar) = EXIT_JAR.get() {
    jar.flush();
  }
}

fn now_ms() -> i64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The cookie a `Set-Cookie` value sets for `host`, received for a request
/// to `request_path`, or `None` when it is to be ignored. The result may be
/// expired already: then it deletes. `created` and `last_access` are `now`.
fn parse_set_cookie(
  set_cookie: &str,
  request_path: &str,
  host: &str,
  now: i64,
) -> Option<StoredCookie> {
  let cookie = cookie::Cookie::parse(set_cookie).ok()?;
  let (name, value) = (cookie.name(), cookie.value());
  if name.len() + value.len() > MAX_NAME_VALUE_LEN
    || has_control_character(name)
    || has_control_character(value)
  {
    return None;
  }

  let expires = if let Some(max_age) = cookie.max_age() {
    let ms = max_age.whole_milliseconds();
    Some(if ms <= 0 {
      i64::MIN
    } else {
      now.saturating_add(
        i64::try_from(ms).unwrap_or(i64::MAX).min(MAX_LIFETIME_MS),
      )
    })
  } else {
    cookie.expires_datetime().map(|at| {
      let ms = i64::try_from(at.unix_timestamp_nanos() / 1_000_000).unwrap_or(
        if at.unix_timestamp() < 0 {
          i64::MIN
        } else {
          i64::MAX
        },
      );
      ms.min(now.saturating_add(MAX_LIFETIME_MS))
    })
  };

  // `domain()` has a leading dot removed.
  let domain = cookie.domain().filter(|d| d.len() <= MAX_ATTRIBUTE_LEN);
  if domain.is_some_and(|d| !d.eq_ignore_ascii_case(host)) {
    return None;
  }
  let path_attribute = cookie
    .path()
    .filter(|p| p.starts_with('/') && p.len() <= MAX_ATTRIBUTE_LEN);
  let path =
    path_attribute.map_or_else(|| default_path(request_path), str::to_string);
  let secure = cookie.secure() == Some(true);
  let same_site = match cookie.same_site() {
    Some(cookie::SameSite::Strict) => SameSite::Strict,
    Some(cookie::SameSite::Lax) => SameSite::Lax,
    Some(cookie::SameSite::None) => SameSite::None,
    None => SameSite::Default,
  };
  if same_site == SameSite::None && !secure {
    return None;
  }
  let prefixed = |prefix: &str| {
    name
      .get(..prefix.len())
      .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
  };
  if prefixed("__Secure-") && !secure {
    return None;
  }
  if prefixed("__Host-")
    && !(secure && domain.is_none() && path_attribute == Some("/"))
  {
    return None;
  }

  Some(StoredCookie {
    name: name.to_string(),
    value: value.to_string(),
    path,
    expires,
    secure,
    http_only: cookie.http_only() == Some(true),
    same_site,
    host_only: domain.is_none(),
    created: now,
    last_access: now,
  })
}

/// A control character a cookie name or value can't hold (RFC 6265bis 5.6:
/// %x00-08, %x0A-1F, %x7F; a tab is allowed).
fn has_control_character(text: &str) -> bool {
  text.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f)
}

/// The default cookie path of a request path (RFC 6265bis 5.1.4): up to,
/// not including, its last `/`; `/` when that leaves nothing.
fn default_path(request_path: &str) -> String {
  if !request_path.starts_with('/') {
    return "/".to_string();
  }
  match request_path.rfind('/') {
    Some(0) | None => "/".to_string(),
    Some(i) => request_path[..i].to_string(),
  }
}

/// RFC 6265bis 5.1.4 path-match.
fn path_matches(cookie_path: &str, request_path: &str) -> bool {
  match request_path.strip_prefix(cookie_path) {
    Some("") => true,
    Some(rest) => cookie_path.ends_with('/') || rest.starts_with('/'),
    None => false,
  }
}

/// The persistent cookies in `file`, or none when it is missing or can't be
/// read (logged).
fn load(file: &Path, now: i64) -> Vec<StoredCookie> {
  let bytes = match std::fs::read(file) {
    Ok(bytes) => bytes,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
    Err(e) => {
      log::warn!(
        "[desktop] could not read the app's cookies from {}: {e}; starting without cookies",
        file.display()
      );
      return Vec::new();
    }
  };
  let parsed = serde_json::from_slice::<CookieFile>(&bytes)
    .map_err(|e| e.to_string())
    .and_then(|f| {
      if f.version == FILE_VERSION {
        Ok(f.cookies)
      } else {
        Err(format!("unknown version {}", f.version))
      }
    });
  match parsed {
    Ok(mut cookies) => {
      // The file is ours, but a damaged or edited entry mustn't end up in a
      // request header.
      cookies.retain(|c| {
        c.persistent()
          && !c.expired(now)
          && !c.name.is_empty()
          && c.name.len() + c.value.len() <= MAX_NAME_VALUE_LEN
          && !has_control_character(&c.name)
          && !has_control_character(&c.value)
          && !c.name.contains(['=', ';'])
          && !c.value.contains(';')
          && c.path.starts_with('/')
          && !has_control_character(&c.path)
      });
      cookies.truncate(MAX_COOKIES);
      cookies
    }
    Err(e) => {
      log::warn!(
        "[desktop] the app's cookie file {} is damaged ({e}); starting without cookies",
        file.display()
      );
      Vec::new()
    }
  }
}

/// Replace `file` with `contents`: written to a temporary file in the same
/// folder (owner-only on unix), synced, then renamed over it.
fn write_file(file: &Path, contents: &CookieFile) -> std::io::Result<()> {
  let json = serde_json::to_vec(contents)?;
  let dir = file.parent().unwrap_or(Path::new("."));
  std::fs::create_dir_all(dir)?;
  let tmp = dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
  let _ = std::fs::remove_file(&tmp);
  let mut options = std::fs::OpenOptions::new();
  options.write(true).create_new(true);
  #[cfg(unix)]
  {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
  }
  let result = (|| {
    use std::io::Write;
    let mut out = options.open(&tmp)?;
    out.write_all(&json)?;
    out.sync_all()?;
    std::fs::rename(&tmp, file)
  })();
  if result.is_err() {
    let _ = std::fs::remove_file(&tmp);
  }
  result
}

#[cfg(test)]
mod tests {
  use super::*;

  const HOST: &str = "com.example.app";
  const NOW: i64 = 1_800_000_000_000;
  const SECOND: i64 = 1000;

  fn same_origin() -> RequestContext {
    RequestContext::new(Initiator::SameOrigin, false, "GET")
  }

  fn jar() -> Arc<CookieJar> {
    CookieJar::open(HOST, None)
  }

  /// Store `set_cookie`, received for `path` from a same-origin request.
  fn set(jar: &CookieJar, path: &str, set_cookie: &str) -> bool {
    jar.store_at(path, set_cookie, same_origin(), NOW)
  }

  fn header(jar: &CookieJar, path: &str) -> Option<String> {
    jar.cookie_header_at(path, same_origin(), NOW)
  }

  fn header_at(jar: &CookieJar, path: &str, now: i64) -> Option<String> {
    jar.cookie_header_at(path, same_origin(), now)
  }

  fn cookies(jar: &CookieJar) -> Vec<StoredCookie> {
    jar.state.lock().unwrap().cookies.clone()
  }

  #[test]
  fn stores_and_sends_a_session_cookie() {
    let jar = jar();
    // A session cookie changes nothing on disk.
    assert!(!set(&jar, "/login", "sid=abc123; Path=/; HttpOnly"));
    assert_eq!(header(&jar, "/").as_deref(), Some("sid=abc123"));
    assert_eq!(header(&jar, "/deep/page").as_deref(), Some("sid=abc123"));
    let stored = cookies(&jar);
    assert_eq!(stored.len(), 1);
    assert!(stored[0].http_only);
    assert!(stored[0].host_only);
    assert_eq!(stored[0].expires, None);
    // The value is sent as it was set, quotes included.
    set(&jar, "/", "q=\"quoted value\"; Path=/");
    assert_eq!(
      header(&jar, "/").as_deref(),
      Some("sid=abc123; q=\"quoted value\"")
    );
  }

  #[test]
  fn a_cookie_with_the_same_name_and_path_is_replaced() {
    let jar = jar();
    set(&jar, "/", "a=1; Path=/");
    jar.store_at("/", "a=2; Path=/", same_origin(), NOW + SECOND);
    set(&jar, "/", "a=3; Path=/x");
    let stored = cookies(&jar);
    assert_eq!(stored.len(), 2);
    let root = stored.iter().find(|c| c.path == "/").unwrap();
    assert_eq!(root.value, "2");
    // The replacement keeps the first one's creation time.
    assert_eq!(root.created, NOW);
  }

  #[test]
  fn the_default_path_is_the_request_directory() {
    assert_eq!(default_path("/a/b/c"), "/a/b");
    assert_eq!(default_path("/a/"), "/a");
    assert_eq!(default_path("/a"), "/");
    assert_eq!(default_path("/"), "/");
    assert_eq!(default_path(""), "/");
    assert_eq!(default_path("x/y"), "/");

    let jar = jar();
    set(&jar, "/a/b/c", "d=1");
    assert_eq!(cookies(&jar)[0].path, "/a/b");
    assert!(header(&jar, "/a/b").is_some());
    assert!(header(&jar, "/a/b/other").is_some());
    assert!(header(&jar, "/a/bc").is_none());
    assert!(header(&jar, "/a").is_none());
    // A Path that doesn't start with `/` is the default path.
    set(&jar, "/x/y", "e=1; Path=relative");
    assert_eq!(cookies(&jar)[1].path, "/x");
  }

  #[test]
  fn paths_match_per_rfc_6265bis() {
    assert!(path_matches("/docs", "/docs"));
    assert!(path_matches("/docs", "/docs/"));
    assert!(path_matches("/docs", "/docs/web"));
    assert!(path_matches("/docs/", "/docs/web"));
    assert!(path_matches("/", "/anything"));
    assert!(!path_matches("/docs", "/docsets"));
    assert!(!path_matches("/docs", "/"));
    assert!(!path_matches("/docs/", "/docs"));
  }

  #[test]
  fn longer_paths_and_older_cookies_come_first() {
    let jar = jar();
    jar.store_at("/", "b=1; Path=/", same_origin(), NOW + SECOND);
    jar.store_at("/", "a=1; Path=/", same_origin(), NOW + 2 * SECOND);
    jar.store_at("/", "c=1; Path=/app", same_origin(), NOW + 3 * SECOND);
    assert_eq!(
      header_at(&jar, "/app/x", NOW + 4 * SECOND).as_deref(),
      Some("c=1; b=1; a=1")
    );
  }

  #[test]
  fn domain_must_be_the_app_host() {
    let jar = jar();
    set(&jar, "/", "a=1; Domain=com.example.app");
    set(&jar, "/", "b=1; Domain=.COM.Example.App");
    assert_eq!(header(&jar, "/").as_deref(), Some("a=1; b=1"));
    assert!(cookies(&jar).iter().all(|c| !c.host_only));
    // Another domain, a parent domain and a subdomain are all refused.
    for domain in ["other.example", "example.app", "x.com.example.app", "app"] {
      set(&jar, "/", &format!("c=1; Domain={domain}"));
    }
    assert_eq!(cookies(&jar).len(), 2);
    // An empty Domain is no Domain.
    set(&jar, "/", "d=1; Domain=");
    assert!(cookies(&jar)[2].host_only);
  }

  #[test]
  fn max_age_and_expires_set_the_expiry() {
    let jar = jar();
    assert!(set(&jar, "/", "a=1; Max-Age=10"));
    assert!(header_at(&jar, "/", NOW + 9 * SECOND).is_some());
    assert!(header_at(&jar, "/", NOW + 10 * SECOND).is_none());
    assert!(cookies(&jar).is_empty(), "an expired cookie is dropped");

    // Max-Age wins over Expires.
    set(
      &jar,
      "/",
      "b=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Max-Age=60",
    );
    assert_eq!(cookies(&jar)[0].expires, Some(NOW + 60 * SECOND));
    // An Expires in the future.
    set(&jar, "/", "c=1; Expires=Sat, 01 Jan 2028 00:00:00 GMT");
    let c = cookies(&jar).into_iter().find(|c| c.name == "c").unwrap();
    assert_eq!(c.expires, Some(1_830_297_600_000));
    // An unparsable Expires is ignored: a session cookie.
    set(&jar, "/", "d=1; Expires=someday");
    let d = cookies(&jar).into_iter().find(|c| c.name == "d").unwrap();
    assert_eq!(d.expires, None);
  }

  #[test]
  fn the_lifetime_is_capped_at_400_days() {
    let jar = jar();
    set(&jar, "/", "a=1; Max-Age=999999999999");
    set(&jar, "/", "b=1; Expires=Fri, 01 Jan 9999 00:00:00 GMT");
    for cookie in cookies(&jar) {
      assert_eq!(
        cookie.expires,
        Some(NOW + MAX_LIFETIME_MS),
        "{}",
        cookie.name
      );
    }
  }

  #[test]
  fn a_past_expiry_deletes() {
    let jar = jar();
    for deletion in [
      "sid=; Path=/; Max-Age=0",
      "sid=; Path=/; Max-Age=-1",
      "sid=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
    ] {
      set(&jar, "/", "sid=1; Path=/; Max-Age=3600");
      assert!(header(&jar, "/").is_some());
      assert!(set(&jar, "/", deletion), "{deletion}");
      assert!(header(&jar, "/").is_none(), "{deletion}");
    }
    // A session cookie is deleted the same way, and only on its own path.
    set(&jar, "/", "s=1; Path=/");
    set(&jar, "/", "s=1; Path=/x");
    assert!(!set(&jar, "/", "s=; Path=/x; Max-Age=0"));
    assert_eq!(cookies(&jar).len(), 1);
    assert_eq!(cookies(&jar)[0].path, "/");
    // Deleting what isn't there stores nothing.
    assert!(!set(&jar, "/", "nothing=; Max-Age=0"));
    assert_eq!(cookies(&jar).len(), 1);
  }

  #[test]
  fn oversized_and_malformed_cookies_are_ignored() {
    let jar = jar();
    let value = "v".repeat(MAX_NAME_VALUE_LEN - 1);
    set(&jar, "/", &format!("n={value}"));
    assert_eq!(cookies(&jar).len(), 1, "4096 bytes fit");
    set(&jar, "/", &format!("nn={value}"));
    assert_eq!(cookies(&jar).len(), 1, "4097 bytes don't");
    // Control characters, a missing name and a missing `=`.
    for bad in ["a=b\u{1}c", "a\u{7f}=b", "=value", "value", ""] {
      set(&jar, "/", bad);
    }
    assert_eq!(cookies(&jar).len(), 1);
    // A tab is allowed.
    set(&jar, "/", "t=a\tb");
    assert_eq!(cookies(&jar).len(), 2);
    // An overlong Path is ignored (the default path applies).
    let long = format!("/{}", "p".repeat(MAX_ATTRIBUTE_LEN));
    set(&jar, "/dir/page", &format!("lp=1; Path={long}"));
    let lp = cookies(&jar).into_iter().find(|c| c.name == "lp").unwrap();
    assert_eq!(lp.path, "/dir");
  }

  #[test]
  fn the_least_recently_used_cookie_is_evicted_beyond_the_limit() {
    let jar = jar();
    for i in 0..MAX_COOKIES {
      jar.store_at(
        "/",
        &format!("c{i}=1; Path=/p{i}"),
        same_origin(),
        NOW + i as i64,
      );
    }
    // c0 is used, so c1 is now the least recently used.
    assert!(header_at(&jar, "/p0", NOW + 1000).is_some());
    jar.store_at("/", "new=1", same_origin(), NOW + 2000);
    let names = cookies(&jar)
      .into_iter()
      .map(|c| c.name)
      .collect::<Vec<_>>();
    assert_eq!(names.len(), MAX_COOKIES);
    assert!(names.contains(&"c0".to_string()));
    assert!(!names.contains(&"c1".to_string()));
    assert!(names.contains(&"new".to_string()));
  }

  #[test]
  fn prefixes_and_samesite_none_need_secure() {
    let jar = jar();
    for refused in [
      "__Secure-a=1",
      "__secure-b=1; Path=/",
      "__Host-c=1; Path=/",
      "__Host-d=1; Secure",
      "__Host-e=1; Secure; Path=/x",
      "__Host-f=1; Secure; Path=/; Domain=com.example.app",
      "g=1; SameSite=None",
    ] {
      set(&jar, "/", refused);
    }
    assert!(cookies(&jar).is_empty());
    for accepted in [
      "__Secure-a=1; Secure",
      "__Host-c=1; Secure; Path=/",
      "g=1; SameSite=None; Secure",
    ] {
      set(&jar, "/", accepted);
    }
    assert_eq!(cookies(&jar).len(), 3);
  }

  /// The jar with a Strict, a Lax, a None and a default cookie.
  fn samesite_jar() -> Arc<CookieJar> {
    let jar = jar();
    set(&jar, "/", "strict=1; SameSite=Strict");
    set(&jar, "/", "lax=1; SameSite=Lax");
    set(&jar, "/", "none=1; SameSite=None; Secure");
    set(&jar, "/", "default=1");
    jar
  }

  fn sent(
    jar: &CookieJar,
    initiator: Initiator,
    nav: bool,
    method: &str,
  ) -> String {
    jar
      .cookie_header_at("/", RequestContext::new(initiator, nav, method), NOW)
      .unwrap_or_default()
  }

  #[test]
  fn same_origin_requests_get_every_cookie() {
    let jar = samesite_jar();
    let all = "strict=1; lax=1; none=1; default=1";
    assert_eq!(sent(&jar, Initiator::SameOrigin, false, "GET"), all);
    assert_eq!(sent(&jar, Initiator::SameOrigin, false, "POST"), all);
    assert_eq!(sent(&jar, Initiator::SameOrigin, true, "POST"), all);
  }

  #[test]
  fn cross_site_requests_get_lax_cookies_on_safe_navigations_only() {
    let jar = samesite_jar();
    // A cross-site fetch or subresource gets no cookie, SameSite=None
    // included.
    assert_eq!(sent(&jar, Initiator::CrossSite, false, "GET"), "");
    assert_eq!(sent(&jar, Initiator::CrossSite, false, "HEAD"), "");
  }

  #[test]
  fn header_less_get_requests_get_lax_cookies_but_not_strict() {
    let jar = samesite_jar();
    // The app's XHR, images, scripts and stylesheets.
    for method in ["GET", "HEAD", "get"] {
      assert_eq!(
        sent(&jar, Initiator::Unknown, false, method),
        "lax=1; none=1; default=1",
        "{method}"
      );
    }
  }

  #[test]
  fn header_less_unsafe_requests_get_no_cookie() {
    let jar = samesite_jar();
    for method in ["POST", "PUT", "DELETE", "PATCH", "OPTIONS"] {
      assert_eq!(
        sent(&jar, Initiator::Unknown, false, method),
        "",
        "{method}"
      );
    }
  }

  #[test]
  fn other_navigations_get_lax_cookies_with_a_safe_method_only() {
    let jar = samesite_jar();
    for initiator in [Initiator::CrossSite, Initiator::Unknown] {
      // A form POST gets no cookie.
      assert_eq!(sent(&jar, initiator, true, "POST"), "", "{initiator:?}");
      // A navigation gets all but Strict.
      assert_eq!(
        sent(&jar, initiator, true, "GET"),
        "lax=1; none=1; default=1",
        "{initiator:?}"
      );
    }
  }

  #[test]
  fn set_cookie_is_stored_from_same_origin_requests_and_navigations_only() {
    let jar = jar();
    let ctx = RequestContext::new;
    for (initiator, navigation, method) in [
      (Initiator::CrossSite, false, "GET"),
      (Initiator::CrossSite, false, "POST"),
      (Initiator::Unknown, false, "GET"),
    ] {
      jar.store_at(
        "/",
        "a=1; SameSite=None; Secure",
        ctx(initiator, navigation, method),
        NOW,
      );
      jar.store_at("/", "b=1", ctx(initiator, navigation, method), NOW);
    }
    assert!(cookies(&jar).is_empty());
    jar.store_at("/", "c=1", ctx(Initiator::CrossSite, true, "POST"), NOW);
    jar.store_at("/", "d=1", ctx(Initiator::Unknown, true, "GET"), NOW);
    jar.store_at("/", "e=1", ctx(Initiator::SameOrigin, false, "POST"), NOW);
    assert_eq!(cookies(&jar).len(), 3);
  }

  #[test]
  fn the_cookie_file_is_under_the_identifier() {
    let dir = Path::new("/data");
    assert_eq!(
      cookie_file(dir, "com.Example.App"),
      Some(PathBuf::from("/data/com.Example.App/cookies.json"))
    );
    for bad in [
      "",
      ".hidden",
      "..",
      "a/b",
      "a\\b",
      "com.example app",
      "é.app",
    ] {
      assert_eq!(cookie_file(dir, bad), None, "{bad:?}");
    }
  }

  #[test]
  fn persistent_cookies_survive_a_reopen_and_session_cookies_dont() {
    let dir = tempfile::tempdir().unwrap();
    let file = cookie_file(dir.path(), "com.example.app").unwrap();
    let jar = CookieJar::open(HOST, Some(file.clone()));
    jar.store(
      "/",
      [
        b"persistent=1; Max-Age=3600; HttpOnly; SameSite=Strict".as_slice(),
        b"session=1; HttpOnly".as_slice(),
      ],
      same_origin(),
    );
    jar.flush();
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      let mode = std::fs::metadata(&file).unwrap().permissions().mode();
      assert_eq!(mode & 0o777, 0o600);
    }
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("persistent"));
    assert!(!text.contains("session"), "{text}");
    // No temporary file is left behind.
    assert_eq!(
      std::fs::read_dir(file.parent().unwrap()).unwrap().count(),
      1
    );

    let reopened = CookieJar::open(HOST, Some(file.clone()));
    let stored = cookies(&reopened);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name, "persistent");
    assert!(stored[0].http_only);
    assert_eq!(stored[0].same_site, SameSite::Strict);

    // Deleting it empties the file.
    reopened.store("/", [b"persistent=; Max-Age=0".as_slice()], same_origin());
    reopened.flush();
    assert!(cookies(&CookieJar::open(HOST, Some(file))).is_empty());
  }

  #[test]
  fn a_damaged_or_missing_file_starts_an_empty_jar() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cookies.json");
    assert!(cookies(&CookieJar::open(HOST, Some(file.clone()))).is_empty());
    for damaged in [
      b"{not json".as_slice(),
      b"".as_slice(),
      b"{\"version\":99,\"cookies\":[]}".as_slice(),
      b"[1,2,3]".as_slice(),
    ] {
      std::fs::write(&file, damaged).unwrap();
      let jar = CookieJar::open(HOST, Some(file.clone()));
      assert!(cookies(&jar).is_empty());
      // The next write replaces it.
      jar.store("/", [b"a=1; Max-Age=60".as_slice()], same_origin());
      jar.flush();
      assert_eq!(cookies(&CookieJar::open(HOST, Some(file.clone()))).len(), 1);
    }
  }

  #[test]
  fn loaded_cookies_are_checked() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cookies.json");
    let entry = |name: &str, value: &str, path: &str, expires: Option<i64>| {
      serde_json::json!({
        "name": name, "value": value, "path": path, "expires": expires,
        "secure": true, "httpOnly": true, "sameSite": "lax",
        "hostOnly": true, "created": 1, "lastAccess": 1,
      })
    };
    let later = now_ms() + 3_600_000;
    let contents = serde_json::json!({
      "version": 1,
      "cookies": [
        entry("ok", "1", "/", Some(later)),
        entry("crlf", "1\r\nx-injected: 1", "/", Some(later)),
        entry("semi", "1; other=2", "/", Some(later)),
        entry("", "nameless", "/", Some(later)),
        entry("relative", "1", "x", Some(later)),
        entry("expired", "1", "/", Some(1)),
        entry("session", "1", "/", None),
      ],
    });
    std::fs::write(&file, contents.to_string()).unwrap();
    let names = cookies(&CookieJar::open(HOST, Some(file)))
      .into_iter()
      .map(|c| c.name)
      .collect::<Vec<_>>();
    assert_eq!(names, vec!["ok"]);
  }

  #[tokio::test]
  async fn writes_are_debounced_and_skip_session_changes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("id").join("cookies.json");
    let jar = CookieJar::open(HOST, Some(file.clone()));
    // A session cookie schedules no write.
    jar.store("/", [b"s=1".as_slice()], same_origin());
    assert!(!jar.write_scheduled.load(Ordering::Acquire));
    jar.store("/", [b"a=1; Max-Age=60".as_slice()], same_origin());
    jar.store("/", [b"b=1; Max-Age=60".as_slice()], same_origin());
    assert!(!file.exists(), "written at once");
    tokio::time::sleep(WRITE_DELAY + Duration::from_millis(500)).await;
    let reopened = CookieJar::open(HOST, Some(file));
    assert_eq!(cookies(&reopened).len(), 2, "both in one write");
    assert!(!jar.write_scheduled.load(Ordering::Acquire));
  }
}
