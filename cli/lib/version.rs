// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;

use ende_runtime::ende_telemetry::OtelRuntimeConfig;

use crate::shared::ReleaseChannel;

pub fn otel_runtime_config() -> OtelRuntimeConfig {
  OtelRuntimeConfig {
    runtime_name: Cow::Borrowed("ende"),
    runtime_version: Cow::Borrowed(crate::version::ENDE_VERSION_INFO.ende),
  }
}

const GIT_COMMIT_HASH: &str = env!("GIT_COMMIT_HASH");
const TYPESCRIPT: &str = "6.0.3";
pub const ENDE_VERSION: &str = env!("ENDE_VERSION");

/// The Node.js version that Ende reports through `process.version` /
/// `process.versions.node`, used to enforce package.json `engines.node`
/// constraints.
///
/// Re-exported from `ext/node`, which is the single source of truth shared with
/// the `process.version` polyfill, so the engines check and the reported value
/// can never drift.
pub use ende_node::NODE_VERSION;
// TODO(bartlomieju): ideally we could remove this const.
const IS_CANARY: bool = option_env!("ENDE_CANARY").is_some();
// TODO(bartlomieju): this is temporary, to allow Homebrew to cut RC releases as well
const IS_RC: bool = option_env!("ENDE_RC").is_some();

pub static ENDE_VERSION_INFO: std::sync::LazyLock<EndeVersionInfo> =
  std::sync::LazyLock::new(|| {
    #[cfg(not(all(
      debug_assertions,
      target_os = "macos",
      target_arch = "x86_64"
    )))]
    let release_channel = libsui::find_section("denover")
      .ok()
      .flatten()
      .and_then(|buf| std::str::from_utf8(buf).ok())
      .and_then(|str_| ReleaseChannel::deserialize(str_).ok())
      .unwrap_or({
        if IS_CANARY {
          ReleaseChannel::Canary
        } else if IS_RC {
          ReleaseChannel::Rc
        } else {
          release_channel_from_version_string(ENDE_VERSION)
        }
      });

    #[cfg(all(debug_assertions, target_os = "macos", target_arch = "x86_64"))]
    let release_channel = if IS_CANARY {
      ReleaseChannel::Canary
    } else if IS_RC {
      ReleaseChannel::Rc
    } else {
      release_channel_from_version_string(ENDE_VERSION)
    };

    EndeVersionInfo {
      ende: if release_channel == ReleaseChannel::Canary {
        concat!(env!("ENDE_VERSION"), "+", env!("GIT_COMMIT_HASH_SHORT"))
      } else {
        env!("ENDE_VERSION")
      },

      release_channel,

      git_hash: GIT_COMMIT_HASH,

      // Keep in sync with `ende` field.
      user_agent: if release_channel == ReleaseChannel::Canary {
        concat!(
          "Ende/",
          env!("ENDE_VERSION"),
          "+",
          env!("GIT_COMMIT_HASH_SHORT")
        )
      } else {
        concat!("Ende/", env!("ENDE_VERSION"))
      },

      typescript: TYPESCRIPT,
    }
  });

pub struct EndeVersionInfo {
  /// Human-readable version of the current Ende binary.
  ///
  /// For stable release, a semver, eg. `v1.46.2`.
  /// For canary release, a semver + 7-char git hash, eg. `v1.46.3+asdfqwq`.
  pub ende: &'static str,

  pub release_channel: ReleaseChannel,

  /// A full git hash.
  pub git_hash: &'static str,

  /// A user-agent header that will be used in HTTP client.
  pub user_agent: &'static str,

  pub typescript: &'static str,
}

impl EndeVersionInfo {
  /// For stable release, a semver like, eg. `v1.46.2`.
  /// For canary release a full git hash, eg. `9bdab6fb6b93eb43b1930f40987fa4997287f9c8`.
  pub fn version_or_git_hash(&self) -> &'static str {
    if self.release_channel == ReleaseChannel::Canary {
      self.git_hash
    } else {
      ENDE_VERSION
    }
  }
}

fn release_channel_from_version_string(version: &str) -> ReleaseChannel {
  let v = deno_semver::Version::parse_standard(version).ok();
  match v.and_then(|v| v.pre.first().map(|s| s.as_str().to_string())) {
    Some(ref s) if s == "alpha" => ReleaseChannel::Alpha,
    Some(ref s) if s == "beta" => ReleaseChannel::Beta,
    Some(ref s) if s == "rc" => ReleaseChannel::Rc,
    _ => ReleaseChannel::Stable,
  }
}
