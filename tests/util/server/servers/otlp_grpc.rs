// Copyright 2026 the Ende authors. MIT license.

//! An OTLP gRPC collector that reports `grpc-status` in HTTP/2 trailers (the
//! normal gRPC response form) instead of in the initial response headers
//! (the Trailers-Only form that `Ende.serve` produces).
//!
//! - `POST <service path>` stores the request under the `x-test-session`
//!   request header and responds with `grpc-status` set to the
//!   `x-test-grpc-status` header (default `0`) in the trailers, plus
//!   `grpc-message` when `x-test-grpc-message` is set.
//! - `GET /requests/<session>` returns and forgets the stored requests as
//!   JSON: `[{ "path": "<service path>", "body": "<base64 gRPC frame>" }]`.

use std::collections::HashMap;
use std::sync::LazyLock;

use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use bytes::Bytes;
use futures::StreamExt;
use hyper::header::HeaderValue;
use parking_lot::Mutex;
use rustls_tokio_stream::TlsStream;
use tokio::net::TcpStream;

use super::SupportedHttpVersions;
use super::get_tls_listener_stream;

/// Stored request bodies by session, in arrival order.
static REQUESTS: LazyLock<Mutex<HashMap<String, Vec<serde_json::Value>>>> =
  LazyLock::new(Default::default);

pub async fn otlp_grpc_server(port: u16) {
  let mut tls = get_tls_listener_stream(
    "otlp grpc (trailers)",
    port,
    SupportedHttpVersions::Http2Only,
  )
  .await;
  while let Some(Ok(tls)) = tls.next().await {
    tokio::spawn(async move {
      let _ = serve(tls).await;
    });
  }
}

async fn serve(socket: TlsStream<TcpStream>) -> Result<(), anyhow::Error> {
  let mut connection = h2::server::handshake(socket).await?;
  while let Some(result) = connection.accept().await {
    let (request, respond) = result?;
    tokio::spawn(async move {
      let _ = handle_request(request, respond).await;
    });
  }
  Ok(())
}

fn header_str<'a>(
  request: &'a hyper::Request<h2::RecvStream>,
  name: &str,
) -> Option<&'a str> {
  request.headers().get(name).and_then(|v| v.to_str().ok())
}

async fn handle_request(
  mut request: hyper::Request<h2::RecvStream>,
  mut respond: h2::server::SendResponse<Bytes>,
) -> Result<(), anyhow::Error> {
  let path = request.uri().path().to_string();

  if request.method() == hyper::Method::GET {
    let session = path.strip_prefix("/requests/").unwrap_or_default();
    let requests = REQUESTS.lock().remove(session).unwrap_or_default();
    let body = serde_json::to_vec(&requests)?;
    let response = hyper::Response::builder()
      .header("content-type", "application/json")
      .body(())?;
    let mut send = respond.send_response(response, false)?;
    send.send_data(body.into(), true)?;
    return Ok(());
  }

  let session = header_str(&request, "x-test-session")
    .unwrap_or_default()
    .to_string();
  let grpc_status = header_str(&request, "x-test-grpc-status")
    .unwrap_or("0")
    .to_string();
  let grpc_message =
    header_str(&request, "x-test-grpc-message").map(|v| v.to_string());

  let body = request.body_mut();
  let mut bytes = Vec::new();
  while let Some(data) = body.data().await {
    let data = data?;
    let _ = body.flow_control().release_capacity(data.len());
    bytes.extend_from_slice(&data);
  }
  REQUESTS
    .lock()
    .entry(session)
    .or_default()
    .push(serde_json::json!({
      "path": path,
      "body": BASE64_STANDARD.encode(&bytes),
    }));

  // The initial headers don't carry `grpc-status`, so the client has to read
  // it from the trailers.
  let response = hyper::Response::builder()
    .header("content-type", "application/grpc")
    .body(())?;
  let mut send = respond.send_response(response, false)?;
  if grpc_status == "0" {
    // an empty Export*ServiceResponse message
    send.send_data(Bytes::from_static(&[0, 0, 0, 0, 0]), false)?;
  }
  let mut trailers = hyper::HeaderMap::new();
  trailers.insert("grpc-status", HeaderValue::from_str(&grpc_status)?);
  if let Some(message) = grpc_message {
    trailers.insert("grpc-message", HeaderValue::from_str(&message)?);
  }
  send.send_trailers(trailers)?;
  Ok(())
}
