// Copyright 2018-2026 the Deno authors. MIT license.

//! Support for legacy Deno clients.
//!
//! A legacy client configures the server through the `deno` settings section,
//! calls `deno/*` custom methods and `deno.*` commands, and expects `deno:`
//! virtual document URIs, `deno.*` client commands and `deno*` payload fields
//! (`denoJson`). Other clients use `ende`, `ende/*`, `ende.*`, `ende:` and
//! `ende*` fields. The server accepts both forms on input. This module decides which form the server sends: the first
//! signal from the client decides, and the decision doesn't change afterwards.
//! Until then the server sends the `ende` form.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;

use ende_core::serde_json::Value;

const UNDECIDED: u8 = 0;
const MODERN: u8 = 1;
const LEGACY: u8 = 2;

static MODE: AtomicU8 = AtomicU8::new(UNDECIDED);
static WARNED_DEPRECATED_SETTINGS_SECTION: AtomicBool = AtomicBool::new(false);

const VIRTUAL_DOCUMENT_SCHEME: &str = "ende";
const LEGACY_VIRTUAL_DOCUMENT_SCHEME: &str = "deno";
const SETTINGS_SECTION: &str = "ende";
const LEGACY_SETTINGS_SECTION: &str = "deno";

/// Whether the server sends the legacy `deno` form to the client.
pub fn is_legacy() -> bool {
  MODE.load(Ordering::Relaxed) == LEGACY
}

fn decide(mode: u8) {
  let _ = MODE.compare_exchange(
    UNDECIDED,
    mode,
    Ordering::Relaxed,
    Ordering::Relaxed,
  );
}

/// Records that the client called a `deno/*` custom method or a `deno.*`
/// command.
pub fn on_legacy_method_called() {
  decide(LEGACY);
}

/// Picks the server settings from the values the client sent for the `ende`
/// and the deprecated `deno` sections. `ende` wins when both are present.
/// Returns the settings and, the first time the `deno` section is used, a
/// warning to show to the client.
pub fn select_settings(
  ende: Value,
  deno: Value,
) -> (Value, Option<&'static str>) {
  if !ende.is_null() {
    decide(MODERN);
    return (ende, None);
  }
  if deno.is_null() {
    return (Value::Null, None);
  }
  decide(LEGACY);
  let warning = (!WARNED_DEPRECATED_SETTINGS_SECTION
    .swap(true, Ordering::Relaxed))
  .then_some(DEPRECATED_SETTINGS_SECTION_WARNING);
  (deno, warning)
}

const DEPRECATED_SETTINGS_SECTION_WARNING: &str =
  "The \"deno\" settings section is deprecated. Use \"ende\" instead.";

/// The scheme of the virtual documents the server sends to the client.
pub fn virtual_document_scheme() -> &'static str {
  if is_legacy() {
    LEGACY_VIRTUAL_DOCUMENT_SCHEME
  } else {
    VIRTUAL_DOCUMENT_SCHEME
  }
}

/// Whether `scheme` is a virtual document scheme (`ende` or `deno`).
pub fn is_virtual_document_scheme(scheme: &str) -> bool {
  scheme.eq_ignore_ascii_case(VIRTUAL_DOCUMENT_SCHEME)
    || scheme.eq_ignore_ascii_case(LEGACY_VIRTUAL_DOCUMENT_SCHEME)
}

/// The settings section the client uses (`ende`, or `deno` for legacy
/// clients), for naming settings in messages.
pub fn settings_section() -> &'static str {
  if is_legacy() {
    LEGACY_SETTINGS_SECTION
  } else {
    SETTINGS_SECTION
  }
}

/// The id of the command `name` (e.g. `cache` or `client.test`) that the
/// server sends to the client: `ende.<name>`, or `deno.<name>` for legacy
/// clients.
pub fn command_id(name: &str) -> String {
  format!("{}.{name}", settings_section())
}

/// The ids of the commands the server executes, in both forms, since the
/// server capabilities are sent before the client kind is known.
pub fn server_command_ids(names: &[&str]) -> Vec<String> {
  [SETTINGS_SECTION, LEGACY_SETTINGS_SECTION]
    .iter()
    .flat_map(|prefix| names.iter().map(move |name| format!("{prefix}.{name}")))
    .collect()
}

/// Returns the name of the command with the id `id` sent by the client
/// (`ende.<name>` or `deno.<name>`). A `deno.*` id marks the client as a
/// legacy client.
pub fn command_name(id: &str) -> Option<&str> {
  if let Some(name) = id.strip_prefix("ende.") {
    return Some(name);
  }
  let name = id.strip_prefix("deno.")?;
  on_legacy_method_called();
  Some(name)
}
