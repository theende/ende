// Copyright 2018-2026 the Deno authors. MIT license.
use std::borrow::Cow;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::rc::Rc;

use deno_error::JsErrorBox;
use ende_core::OpState;
use ende_core::ResourceId;
use ende_net::raw::NetworkStream;
use ende_net::raw::NetworkStreamAddress;
use ende_net::raw::NetworkStreamListener;
use ende_net::raw::NetworkStreamType;
use ende_net::raw::take_network_stream_listener_resource;
use ende_net::raw::take_network_stream_resource;
use hyper::HeaderMap;
use hyper::Uri;
use hyper::header::HOST;

// TODO(mmastrac): I don't like that we have to clone this, but it's one-time setup
#[derive(Clone)]
pub struct HttpListenProperties {
  pub scheme: &'static str,
  pub fallback_host: String,
  pub local_port: Option<u32>,
  pub stream_type: NetworkStreamType,
}

#[derive(Clone)]
pub struct HttpConnectionProperties {
  pub peer_address: Rc<str>,
  pub peer_port: Option<u32>,
  pub local_port: Option<u32>,
  pub stream_type: NetworkStreamType,
  pub scheme: &'static str,
  pub fallback_host: Rc<str>,
}

pub struct HttpRequestProperties<'a> {
  pub authority: Option<Cow<'a, str>>,
}

/// Pluggable trait to determine listen, connection and request properties
/// for embedders that wish to provide alternative routes for incoming HTTP.
#[async_trait::async_trait(?Send)]
pub trait HttpPropertyExtractor {
  type Listener: 'static;
  type Connection;

  /// Given a listener [`ResourceId`], returns the [`HttpPropertyExtractor::Listener`].
  fn get_listener_for_rid(
    state: &mut OpState,
    listener_rid: ResourceId,
  ) -> Result<Self::Listener, JsErrorBox>;

  /// Given a connection [`ResourceId`], returns the [`HttpPropertyExtractor::Connection`].
  fn get_connection_for_rid(
    state: &mut OpState,
    connection_rid: ResourceId,
  ) -> Result<Self::Connection, JsErrorBox>;

  /// Determines the listener properties.
  fn listen_properties_from_listener(
    listener: &Self::Listener,
  ) -> Result<HttpListenProperties, std::io::Error>;

  /// Determines the listener properties given a [`HttpPropertyExtractor::Connection`].
  fn listen_properties_from_connection(
    connection: &Self::Connection,
  ) -> Result<HttpListenProperties, std::io::Error>;

  /// Accept a new [`HttpPropertyExtractor::Connection`] from the given listener [`HttpPropertyExtractor::Listener`].
  async fn accept_connection_from_listener(
    listener: &Self::Listener,
  ) -> Result<Self::Connection, JsErrorBox>;

  /// Determines the connection properties.
  fn connection_properties(
    listen_properties: &HttpListenProperties,
    connection: &Self::Connection,
  ) -> HttpConnectionProperties;

  /// Turn a given [`HttpPropertyExtractor::Connection`] into a [`NetworkStream`].
  fn to_network_stream_from_connection(
    connection: Self::Connection,
  ) -> NetworkStream;

  /// Determines the request properties.
  fn request_properties<'a>(
    connection_properties: &'a HttpConnectionProperties,
    uri: &'a Uri,
    headers: &'a HeaderMap,
  ) -> HttpRequestProperties<'a>;
}

pub struct DefaultHttpPropertyExtractor {}

#[async_trait::async_trait(?Send)]
impl HttpPropertyExtractor for DefaultHttpPropertyExtractor {
  type Listener = NetworkStreamListener;

  type Connection = NetworkStream;

  fn get_listener_for_rid(
    state: &mut OpState,
    listener_rid: ResourceId,
  ) -> Result<NetworkStreamListener, JsErrorBox> {
    take_network_stream_listener_resource(
      &mut state.resource_table,
      listener_rid,
    )
  }

  fn get_connection_for_rid(
    state: &mut OpState,
    stream_rid: ResourceId,
  ) -> Result<NetworkStream, JsErrorBox> {
    take_network_stream_resource(&mut state.resource_table, stream_rid)
      .map_err(JsErrorBox::from_err)
  }

  async fn accept_connection_from_listener(
    listener: &NetworkStreamListener,
  ) -> Result<NetworkStream, JsErrorBox> {
    listener
      .accept()
      .await
      .map_err(JsErrorBox::from_err)
      .map(|(stm, _)| stm)
  }

  fn listen_properties_from_listener(
    listener: &NetworkStreamListener,
  ) -> Result<HttpListenProperties, std::io::Error> {
    let stream_type = listener.stream();
    let local_address = listener.listen_address()?;
    listener_properties(stream_type, local_address)
  }

  fn listen_properties_from_connection(
    connection: &Self::Connection,
  ) -> Result<HttpListenProperties, std::io::Error> {
    let stream_type = connection.stream();
    let local_address = connection.local_address()?;
    listener_properties(stream_type, local_address)
  }

  fn to_network_stream_from_connection(
    connection: Self::Connection,
  ) -> NetworkStream {
    connection
  }

  fn connection_properties(
    listen_properties: &HttpListenProperties,
    connection: &NetworkStream,
  ) -> HttpConnectionProperties {
    // We always want some sort of peer address. If we can't get one, just make up one.
    let peer_address = connection.peer_address().unwrap_or_else(|_| {
      NetworkStreamAddress::Ip(SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::new(0, 0, 0, 0),
        0,
      )))
    });
    let peer_port: Option<u32> = match peer_address {
      NetworkStreamAddress::Ip(ip) => Some(ip.port() as _),
      #[cfg(unix)]
      NetworkStreamAddress::Unix(_) => None,
      #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "macos"
      ))]
      NetworkStreamAddress::Vsock(vsock) => Some(vsock.port()),
      NetworkStreamAddress::Memory(_) => None,
      #[cfg(windows)]
      NetworkStreamAddress::WindowsPipe(_) => None,
    };
    let peer_address = match peer_address {
      NetworkStreamAddress::Ip(addr) => Rc::from(addr.ip().to_string()),
      #[cfg(unix)]
      NetworkStreamAddress::Unix(_) => Rc::from("unix"),
      #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "macos"
      ))]
      NetworkStreamAddress::Vsock(addr) => {
        Rc::from(format!("vsock:{}", addr.cid()))
      }
      NetworkStreamAddress::Memory(_) => Rc::from("memory"),
      #[cfg(windows)]
      NetworkStreamAddress::WindowsPipe(_) => Rc::from("pipe"),
    };
    let local_port = listen_properties.local_port;
    let stream_type = listen_properties.stream_type;
    let scheme = listen_properties.scheme;
    let fallback_host = Rc::from(listen_properties.fallback_host.as_str());

    HttpConnectionProperties {
      peer_address,
      peer_port,
      local_port,
      stream_type,
      scheme,
      fallback_host,
    }
  }

  fn request_properties<'a>(
    connection_properties: &'a HttpConnectionProperties,
    uri: &'a Uri,
    headers: &'a HeaderMap,
  ) -> HttpRequestProperties<'a> {
    let authority = req_host(
      uri,
      headers,
      connection_properties.stream_type,
      connection_properties.local_port.unwrap_or_default(),
    );

    HttpRequestProperties { authority }
  }
}

fn listener_properties(
  stream_type: NetworkStreamType,
  local_address: NetworkStreamAddress,
) -> Result<HttpListenProperties, std::io::Error> {
  let scheme = req_scheme_from_stream_type(stream_type);
  let fallback_host = req_host_from_addr(stream_type, &local_address);
  let local_port: Option<u32> = match local_address {
    NetworkStreamAddress::Ip(ip) => Some(ip.port() as _),
    #[cfg(unix)]
    NetworkStreamAddress::Unix(_) => None,
    #[cfg(any(
      target_os = "android",
      target_os = "linux",
      target_os = "macos"
    ))]
    NetworkStreamAddress::Vsock(vsock) => Some(vsock.port()),
    NetworkStreamAddress::Memory(_) => None,
    #[cfg(windows)]
    NetworkStreamAddress::WindowsPipe(_) => None,
  };
  Ok(HttpListenProperties {
    scheme,
    fallback_host,
    local_port,
    stream_type,
  })
}

/// Compute the fallback address from the [`NetworkStreamListenAddress`]. If the request has no authority/host in
/// its URI, and there is no [`HeaderName::HOST`] header, we fall back to this.
fn req_host_from_addr(
  stream_type: NetworkStreamType,
  addr: &NetworkStreamAddress,
) -> String {
  match addr {
    NetworkStreamAddress::Ip(addr) => {
      if (stream_type == NetworkStreamType::Tls && addr.port() == 443)
        || (stream_type == NetworkStreamType::Tcp && addr.port() == 80)
      {
        if addr.ip().is_loopback() || addr.ip().is_unspecified() {
          return "localhost".to_owned();
        }
        addr.ip().to_string()
      } else {
        if addr.ip().is_loopback() || addr.ip().is_unspecified() {
          return format!("localhost:{}", addr.port());
        }
        addr.to_string()
      }
    }
    // There is no standard way for unix domain socket URLs
    // nginx and nodejs request use http://unix:[socket_path]:/ but it is not a valid URL
    // httpie uses http+unix://[percent_encoding_of_path]/ which we follow
    #[cfg(unix)]
    NetworkStreamAddress::Unix(unix) => percent_encoding::percent_encode(
      unix
        .as_pathname()
        .and_then(|x| x.to_str())
        .unwrap_or_default()
        .as_bytes(),
      percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string(),
    #[cfg(any(
      target_os = "android",
      target_os = "linux",
      target_os = "macos"
    ))]
    NetworkStreamAddress::Vsock(vsock) => {
      format!("{}:{}", vsock.cid(), vsock.port())
    }
    // The listener's name is the host its requests carry (it's checked to be
    // a domain name when `ENDE_SERVE_ADDRESS` is parsed). The peer normally
    // sends the same name in `Host`.
    NetworkStreamAddress::Memory(memory) => memory.name.clone(),
    #[cfg(windows)]
    NetworkStreamAddress::WindowsPipe(_) => "localhost".to_owned(),
  }
}

/// Whether a request that did not come over the memory transport names the
/// host of an in-process memory listener in its `request.url`.
///
/// A request bridged in over the memory transport has a plain
/// `http://<host>/…` URL (see [`req_scheme_from_stream_type`]). Without this
/// check any client of a TCP (or other) listener in the same process could
/// send that host in `Host` or an absolute-form target and pass a handler
/// that trusts the URL to mean "from the desktop webview". Such a request is
/// rejected with `400 Bad Request` before it reaches the handler.
/// `info.remoteAddr.transport === "memory"` stays the check to rely on; this
/// keeps the URL from contradicting it.
///
/// `authority` is what `request.url` is built from. It's parsed the way
/// `new URL()` parses it, so every spelling that normalizes to the same host
/// (case, a port, user info, percent-encoding, IDNA mappings) is caught.
pub(crate) fn claims_memory_host(
  authority: &str,
  stream_type: NetworkStreamType,
) -> bool {
  if stream_type == NetworkStreamType::Memory
    || !ende_net::memory::has_listeners()
  {
    return false;
  }
  let Ok(url) = ende_core::url::Url::parse(&format!("http://{authority}/"))
  else {
    return false;
  };
  url.host_str().is_some_and(ende_net::memory::is_listening)
}

/// [`claims_memory_host`] for a request served by hyper. The authority is
/// picked the way `op_http_get_request_url` picks it.
pub(crate) fn request_claims_memory_host(
  request_info: &HttpConnectionProperties,
  uri: &Uri,
  headers: &HeaderMap,
) -> bool {
  if request_info.stream_type == NetworkStreamType::Memory
    || !ende_net::memory::has_listeners()
  {
    return false;
  }
  let authority = uri
    .authority()
    .map(|authority| Cow::Borrowed(authority.as_str()))
    .or_else(|| {
      req_host(
        uri,
        headers,
        request_info.stream_type,
        request_info.local_port.unwrap_or_default(),
      )
    })
    .unwrap_or_else(|| Cow::Borrowed(request_info.fallback_host.as_ref()));
  claims_memory_host(&authority, request_info.stream_type)
}

fn req_scheme_from_stream_type(stream_type: NetworkStreamType) -> &'static str {
  match stream_type {
    NetworkStreamType::Tcp => "http://",
    NetworkStreamType::Tls => "https://",
    #[cfg(unix)]
    NetworkStreamType::Unix => "http+unix://",
    #[cfg(any(
      target_os = "android",
      target_os = "linux",
      target_os = "macos"
    ))]
    NetworkStreamType::Vsock => "http+vsock://",
    // A request bridged in from the desktop webview looks like a plain HTTP
    // request to its host. `info.remoteAddr.transport === "memory"` is what
    // tells it apart.
    NetworkStreamType::Memory => "http://",
    #[cfg(windows)]
    NetworkStreamType::WindowsPipe => "http://",
  }
}

fn req_host<'a>(
  uri: &'a Uri,
  headers: &'a HeaderMap,
  addr_type: NetworkStreamType,
  port: u32,
) -> Option<Cow<'a, str>> {
  // It is rare that an authority will be passed, but if it does, it takes priority
  if let Some(auth) = uri.authority() {
    match addr_type {
      NetworkStreamType::Tcp => {
        if port == 80 {
          return Some(Cow::Borrowed(auth.host()));
        }
      }
      NetworkStreamType::Tls => {
        if port == 443 {
          return Some(Cow::Borrowed(auth.host()));
        }
      }
      #[cfg(unix)]
      NetworkStreamType::Unix => {}
      #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "macos"
      ))]
      NetworkStreamType::Vsock => {}
      NetworkStreamType::Memory => {}
      #[cfg(windows)]
      NetworkStreamType::WindowsPipe => {}
    }
    return Some(Cow::Borrowed(auth.as_str()));
  }

  // TODO(mmastrac): Most requests will use this path and we probably will want to optimize it in the future
  if let Some(host) = headers.get(HOST) {
    // An empty `Host` header value is treated as if no Host header was sent,
    // so the listener's fallback authority is used and `request.url` stays a
    // valid URL. Hyper's HTTP/1 parser already trims OWS from header values,
    // so whitespace-only `Host:` values reach us as empty bytes.
    // See https://github.com/denoland/deno/issues/29872.
    if host.is_empty() {
      return None;
    }
    return Some(match host.to_str() {
      Ok(host) => Cow::Borrowed(host),
      Err(_) => Cow::Owned(
        host
          .as_bytes()
          .iter()
          .cloned()
          .map(char::from)
          .collect::<String>(),
      ),
    });
  }

  None
}

#[cfg(test)]
mod tests {
  use ende_net::memory::MemoryAddr;
  use ende_net::raw::NetworkStreamAddress;
  use ende_net::raw::NetworkStreamType;

  use super::claims_memory_host;
  use super::listener_properties;

  #[test]
  fn memory_listener_serves_plain_http_urls_for_its_host() {
    let properties = listener_properties(
      NetworkStreamType::Memory,
      NetworkStreamAddress::Memory(MemoryAddr {
        name: "app.example".to_string(),
        id: 0,
      }),
    )
    .unwrap();
    assert_eq!(properties.scheme, "http://");
    assert_eq!(properties.fallback_host, "app.example");
    assert_eq!(properties.local_port, None);
  }

  #[test]
  fn memory_host_only_on_the_memory_transport() {
    let _listener =
      ende_net::memory::listen_memory("claims-test.example").unwrap();
    for authority in [
      "claims-test.example",
      "CLAIMS-TEST.Example",
      "claims-test.example:8000",
      "user@claims-test.example",
      "user:pass@claims-test.example:80",
      "claims-test%2Eexample",
      "claims-test\u{3002}example",
    ] {
      assert!(
        claims_memory_host(authority, NetworkStreamType::Tcp),
        "{authority}"
      );
      assert!(
        claims_memory_host(authority, NetworkStreamType::Tls),
        "{authority}"
      );
      assert!(
        !claims_memory_host(authority, NetworkStreamType::Memory),
        "{authority}"
      );
    }
    for authority in [
      "other.example",
      "claims-test.example.",
      "claims-test.example.evil",
      "evil\\@claims-test.example",
      "localhost:8000",
      "",
    ] {
      assert!(
        !claims_memory_host(authority, NetworkStreamType::Tcp),
        "{authority}"
      );
    }
  }
}
