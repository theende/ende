// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;

use ende_runtime::ende_telemetry::OtelRuntimeConfig;

pub fn otel_runtime_config() -> OtelRuntimeConfig {
  OtelRuntimeConfig {
    runtime_name: Cow::Borrowed("ende"),
    runtime_version: Cow::Borrowed(crate::version::ENDE_VERSION_INFO.ende),
  }
}

const GIT_COMMIT_HASH: &str = env!("GIT_COMMIT_HASH");
const TYPESCRIPT: &str = "6.0.3";
pub const ENDE_VERSION: &str = env!("ENDE_VERSION");

pub static ENDE_VERSION_INFO: std::sync::LazyLock<EndeVersionInfo> =
  std::sync::LazyLock::new(|| EndeVersionInfo {
    ende: ENDE_VERSION,
    git_hash: GIT_COMMIT_HASH,
    // Keep in sync with `ende` field.
    user_agent: concat!("Ende/", env!("ENDE_VERSION")),
    typescript: TYPESCRIPT,
  });

pub struct EndeVersionInfo {
  /// Human-readable version of the current Ende binary, eg. `2.9.7`.
  pub ende: &'static str,

  /// A full git hash.
  pub git_hash: &'static str,

  /// A user-agent header that will be used in HTTP client.
  pub user_agent: &'static str,

  pub typescript: &'static str,
}
