// Copyright 2018-2026 the Deno authors. MIT license.

use std::sync::Arc;

use ende_core::Extension;

use crate::ops;
use crate::shared::runtime;

pub fn get_extensions_in_snapshot() -> Vec<Extension> {
  // NOTE(bartlomieju): ordering is important here, keep it in sync with
  // `runtime/worker.rs`, `runtime/web_worker.rs`, `runtime/snapshot_info.rs`
  // and `runtime/snapshot.rs`!
  let fs = std::sync::Arc::new(ende_fs::RealFs);
  vec![
    ende_telemetry::ende_telemetry::init(),
    ende_webidl::ende_webidl::init(),
    ende_web::ende_web::init(
      ende_web::BlobStore::default_arc(),
      Default::default(),
      Default::default(),
      ende_web::InMemoryBroadcastChannel::default(),
    ),
    ende_webgpu::ende_webgpu::init(),
    ende_image::ende_image::init(),
    ende_canvas::ende_canvas::init(),
    ende_fetch::ende_fetch::init(Default::default()),
    ende_cache::ende_cache::init(None),
    ende_websocket::ende_websocket::init(),
    ende_webstorage::ende_webstorage::init(None),
    ende_crypto::ende_crypto::init(None),
    ende_ffi::ende_ffi::init(None),
    ende_net::ende_net::init(None, None),
    ende_tls::ende_tls::init(),
    ende_cron::ende_cron::init(Box::new(
      ende_cron::local::LocalCronHandler::new(),
    )),
    ende_http::ende_http::init(ende_http::Options::default()),
    ende_io::ende_io::init(Some(Default::default())),
    ende_fs::ende_fs::init(fs),
    ende_os::ende_os::init(Default::default()),
    ende_process::ende_process::init(),
    ops::runtime::ende_runtime::init("deno:runtime".parse().unwrap()),
    ops::worker_host::ende_worker_host::init(
      Arc::new(|_| unreachable!("not used in snapshot.")),
      None,
    ),
    ops::fs_events::ende_fs_events::init(),
    ops::permissions::ende_permissions::init(),
    ops::tty::ende_tty::init(),
    ops::http::ende_http_runtime::init(),
    ops::desktop::ende_desktop::init(),
    ops::bootstrap::ende_bootstrap::init(None, false),
    runtime::init(),
    ops::web_worker::ende_web_worker::init(),
  ]
}
