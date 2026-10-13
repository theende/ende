// Copyright 2018-2026 the Deno authors. MIT license.

//! Reads Ende environment variables (`ENDE_*`, and names with an `ENDE`
//! segment such as `OTEL_ENDE_*`). When the variable is not set, the
//! deprecated name with `DENO` in place of `ENDE` (`DENO_X`, `OTEL_DENO_X`) is
//! read instead and a deprecation warning is printed to stderr (once per
//! variable per process).

use std::collections::BTreeSet;
use std::env::VarError;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;

use sys_traits::EnvVar;

/// Reads `name` (an Ende name, see the module docs) from `sys`, falling back
/// to the deprecated `DENO` name.
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

/// Reads `name` (an Ende name, see the module docs) from the process
/// environment, falling back to the deprecated `DENO` name.
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
  let Some(deprecated_name) = deprecated_name(name) else {
    debug_assert!(false, "expected a name with an ENDE segment, got {name}");
    return get(name);
  };
  if let Some(value) = get(name) {
    return Some(value);
  }
  let value = get(&deprecated_name)?;
  if first_use_of_deprecated_name(&deprecated_name) {
    #[allow(
      clippy::print_stderr,
      reason = "env vars can be read before any logger is initialized"
    )]
    {
      eprintln!(
        "Warning: {deprecated_name} is deprecated. Use {name} instead."
      );
    }
  }
  Some(value)
}

/// The deprecated name for `name`: its first `ENDE` segment (between `_`s)
/// replaced with `DENO`, e.g. `OTEL_ENDE_CONSOLE` -> `OTEL_DENO_CONSOLE`.
fn deprecated_name(name: &str) -> Option<String> {
  let mut segments = name.split('_').collect::<Vec<_>>();
  let index = segments.iter().position(|segment| *segment == "ENDE")?;
  segments[index] = "DENO";
  Some(segments.join("_"))
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
  fn falls_back_for_names_with_an_ende_segment() {
    let sys = InMemorySys::default();
    sys.env_set_var("OTEL_DENO", "true");
    sys.env_set_var("OTEL_DENO_TEST_ONLY_CONSOLE", "capture");
    sys.env_set_var("X_DENO_TEST_ONLY_TOKEN", "token");
    sys.env_set_var("NO_DENO_TEST_ONLY_HACK", "1");
    assert_eq!(env_var(&sys, "OTEL_ENDE").unwrap(), "true");
    assert_eq!(
      env_var(&sys, "OTEL_ENDE_TEST_ONLY_CONSOLE").unwrap(),
      "capture"
    );
    assert_eq!(env_var(&sys, "X_ENDE_TEST_ONLY_TOKEN").unwrap(), "token");
    assert_eq!(env_var(&sys, "NO_ENDE_TEST_ONLY_HACK").unwrap(), "1");
  }

  #[test]
  fn ende_segment_name_wins_over_deno_segment_name() {
    let sys = InMemorySys::default();
    sys.env_set_var("OTEL_ENDE_TEST_BOTH", "new");
    sys.env_set_var("OTEL_DENO_TEST_BOTH", "old");
    assert_eq!(env_var(&sys, "OTEL_ENDE_TEST_BOTH").unwrap(), "new");
  }

  #[test]
  fn deprecated_names() {
    assert_eq!(deprecated_name("ENDE_DIR").as_deref(), Some("DENO_DIR"));
    assert_eq!(deprecated_name("OTEL_ENDE").as_deref(), Some("OTEL_DENO"));
    assert_eq!(
      deprecated_name("OTEL_ENDE_VSOCK").as_deref(),
      Some("OTEL_DENO_VSOCK")
    );
    assert_eq!(
      deprecated_name("X_ENDE_FETCH_TOKEN").as_deref(),
      Some("X_DENO_FETCH_TOKEN")
    );
    // Only a whole `ENDE` segment is replaced.
    assert_eq!(deprecated_name("OTEL_EXPORTER_ENDPOINT"), None);
    assert_eq!(deprecated_name("LEGENDE_X"), None);
  }

  #[test]
  fn warns_once_per_name() {
    assert!(first_use_of_deprecated_name("DENO_TEST_WARN_ONCE_A"));
    assert!(!first_use_of_deprecated_name("DENO_TEST_WARN_ONCE_A"));
    assert!(first_use_of_deprecated_name("DENO_TEST_WARN_ONCE_B"));
  }
}
