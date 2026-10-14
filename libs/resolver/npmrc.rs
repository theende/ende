// Copyright 2018-2026 the Deno authors. MIT license.

use std::collections::HashMap;

use ende_npmrc::NpmRegistryUrl;
use ende_npmrc::RegistryConfigWithUrl;
use ende_npmrc::ResolvedNpmRc;
use sys_traits::EnvVar;

#[allow(clippy::disallowed_types, reason = "definition")]
pub type ResolvedNpmRcRc = ende_maybe_sync::MaybeArc<ResolvedNpmRc>;

/// Creates the npm registry configuration. Ende does not read `.npmrc` files;
/// npm packages are fetched from the default registry, which can only be
/// overridden with the `NPM_CONFIG_REGISTRY` environment variable.
pub fn create_default_npmrc(sys: &impl EnvVar) -> ResolvedNpmRc {
  ResolvedNpmRc {
    default_config: ende_npmrc::RegistryConfigWithUrl {
      registry_url: NpmRegistryUrl::for_npm(sys).url,
      config: Default::default(),
    },
    scopes: HashMap::from([(
      "jsr".to_string(),
      RegistryConfigWithUrl {
        registry_url: NpmRegistryUrl::for_jsr(sys).url,
        config: Default::default(),
      },
    )]),
    registry_configs: Default::default(),
    replace_registry_host: Default::default(),
    min_release_age_days: ende_npmrc::min_release_age_days_from_env(sys),
    trust_policy: Default::default(),
    trust_policy_ignore_after_minutes: None,
    trust_policy_exclude: Vec::new(),
  }
}
