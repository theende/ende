// Copyright 2018-2026 the Deno authors. MIT license.

//! Reads `ENDE_*` environment variables. When `ENDE_X` is not set, the
//! deprecated `DENO_X` is read instead and a deprecation warning is printed
//! to stderr (once per variable per process).

use std::collections::BTreeSet;
use std::env::VarError;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;

use sys_traits::EnvVar;

const PREFIX: &str = "ENDE_";
const DEPRECATED_PREFIX: &str = "DENO_";

/// Reads `name` (an `ENDE_*` name) from `sys`, falling back to the
/// deprecated `DENO_*` name.
pub fn env_var_os(sys: &impl EnvVar, name: &str) -> Option<OsString> {
  read(name, |key| sys.env_var_os(key))
}

/// Like [`env_var_os`], but returns the value as a `String`.
pub fn env_var(sys: &impl EnvVar, name: &str) -> Result<String, VarError> {
  into_string(env_var_os(sys, name))
}

/// Like [`env_var_os`], but returns a path and treats an empty value as
/// unset.
pub fn env_var_path(sys: &impl EnvVar, name: &str) -> Option<PathBuf> {
  env_var_os(sys, name)
    .filter(|value| !value.is_empty())
    .map(PathBuf::from)
}

/// Reads `name` (an `ENDE_*` name) from the process environment, falling
/// back to the deprecated `DENO_*` name.
pub fn var_os(name: &str) -> Option<OsString> {
  read(name, |key| std::env::var_os(key))
}

/// Like [`var_os`], but returns the value as a `String`.
pub fn var(name: &str) -> Result<String, VarError> {
  into_string(var_os(name))
}

fn read(
  name: &str,
  get: impl Fn(&str) -> Option<OsString>,
) -> Option<OsString> {
  let Some(suffix) = name.strip_prefix(PREFIX) else {
    debug_assert!(false, "expected an {PREFIX}* name, got {name}");
    return get(name);
  };
  if let Some(value) = get(name) {
    return Some(value);
  }
  let deprecated_name = format!("{DEPRECATED_PREFIX}{suffix}");
  let value = get(&deprecated_name)?;
  if first_use_of_deprecated_name(&deprecated_name) {
    #[allow(
      clippy::print_stderr,
      reason = "env vars can be read before any logger is initialized"
    )]
    {
      eprintln!("Warning: {deprecated_name} is deprecated. Use {name} instead.");
    }
  }
  Some(value)
}

/// Returns `true` only the first time it's called for `deprecated_name`.
fn first_use_of_deprecated_name(deprecated_name: &str) -> bool {
  static WARNED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());
  WARNED.lock().unwrap().insert(deprecated_name.to_string())
}

fn into_string(value: Option<OsString>) -> Result<String, VarError> {
  match value {
    Some(value) => value.into_string().map_err(VarError::NotUnicode),
    None => Err(VarError::NotPresent),
  }
}

#[cfg(test)]
mod test {
  use sys_traits::EnvSetVar;
  use sys_traits::impls::InMemorySys;

  use super::*;

  #[test]
  fn reads_ende_name() {
    let sys = InMemorySys::default();
    sys.env_set_var("ENDE_TEST_ONLY_NEW", "new");
    assert_eq!(env_var(&sys, "ENDE_TEST_ONLY_NEW").unwrap(), "new");
  }

  #[test]
  fn falls_back_to_deno_name() {
    let sys = InMemorySys::default();
    sys.env_set_var("DENO_TEST_ONLY_OLD", "old");
    assert_eq!(env_var(&sys, "ENDE_TEST_ONLY_OLD").unwrap(), "old");
  }

  #[test]
  fn ende_name_wins_over_deno_name() {
    let sys = InMemorySys::default();
    sys.env_set_var("ENDE_TEST_BOTH", "new");
    sys.env_set_var("DENO_TEST_BOTH", "old");
    assert_eq!(env_var(&sys, "ENDE_TEST_BOTH").unwrap(), "new");
  }

  #[test]
  fn unset_is_not_present() {
    let sys = InMemorySys::default();
    assert_eq!(env_var(&sys, "ENDE_TEST_UNSET"), Err(VarError::NotPresent));
    assert_eq!(env_var_path(&sys, "ENDE_TEST_UNSET"), None);
  }

  #[test]
  fn empty_path_is_unset() {
    let sys = InMemorySys::default();
    sys.env_set_var("ENDE_TEST_EMPTY_PATH", "");
    assert_eq!(env_var_path(&sys, "ENDE_TEST_EMPTY_PATH"), None);
  }

  #[test]
  fn warns_once_per_name() {
    assert!(first_use_of_deprecated_name("DENO_TEST_WARN_ONCE_A"));
    assert!(!first_use_of_deprecated_name("DENO_TEST_WARN_ONCE_A"));
    assert!(first_use_of_deprecated_name("DENO_TEST_WARN_ONCE_B"));
  }
}
