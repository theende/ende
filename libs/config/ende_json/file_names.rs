// Copyright 2018-2026 the Deno authors. MIT license.

//! Names of the config file and lockfile. The old `deno.*` names are still
//! read as deprecated fallbacks.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use sys_traits::FsMetadata;

/// Config file names recognized during discovery, in priority order. The
/// `deno.*` names are deprecated fallbacks.
pub const CONFIG_FILE_NAMES: [&str; 4] =
  ["ende.json", "ende.jsonc", "deno.json", "deno.jsonc"];

/// The file name used when creating a new config file.
pub const DEFAULT_CONFIG_FILE_NAME: &str = "ende.json";

/// The default lockfile name.
pub const LOCKFILE_NAME: &str = "ende.lock";

/// The deprecated lockfile name, used when it exists and `ende.lock` doesn't.
pub const DEPRECATED_LOCKFILE_NAME: &str = "deno.lock";

/// Whether `name` is one of the [`CONFIG_FILE_NAMES`].
pub fn is_config_file_name(name: &str) -> bool {
  CONFIG_FILE_NAMES.contains(&name)
}

/// Resolves the default lockfile path for a config in `dir`: `ende.lock`, or
/// an existing `deno.lock` when there is no `ende.lock` (with a deprecation
/// warning).
pub fn resolve_default_lockfile_path(
  sys: &impl FsMetadata,
  dir: &Path,
) -> PathBuf {
  let path = dir.join(LOCKFILE_NAME);
  if !sys.fs_exists_no_err(&path) {
    let deprecated_path = dir.join(DEPRECATED_LOCKFILE_NAME);
    if sys.fs_exists_no_err(&deprecated_path) {
      warn_if_deprecated_file_name(&deprecated_path);
      return deprecated_path;
    }
  }
  path
}

/// Prints a deprecation warning if `path` has a deprecated `deno.*` config or
/// lockfile name. Warns at most once per path per process.
pub fn warn_if_deprecated_file_name(path: &Path) {
  let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
    return;
  };
  let replacement = match name {
    "deno.json" => "ende.json",
    "deno.jsonc" => "ende.jsonc",
    "deno.lock" => "ende.lock",
    _ => return,
  };
  static WARNED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
  if WARNED.lock().unwrap().insert(path.to_path_buf()) {
    let dir = path.parent().unwrap_or(path);
    log::warn!(
      "Warning: \"{}\" in {} is deprecated. Rename it to \"{}\".",
      name,
      dir.display(),
      replacement
    );
  }
}

#[cfg(test)]
mod test {
  use sys_traits::impls::InMemorySys;

  use super::*;

  #[test]
  fn default_lockfile_path() {
    let sys = InMemorySys::default();
    let dir = PathBuf::from("/project");
    sys.fs_insert(dir.join("ende.json"), "{}");
    // neither exists
    assert_eq!(
      resolve_default_lockfile_path(&sys, &dir),
      dir.join("ende.lock")
    );
    // only deno.lock exists
    sys.fs_insert(dir.join("deno.lock"), "{}");
    assert_eq!(
      resolve_default_lockfile_path(&sys, &dir),
      dir.join("deno.lock")
    );
    // both exist
    sys.fs_insert(dir.join("ende.lock"), "{}");
    assert_eq!(
      resolve_default_lockfile_path(&sys, &dir),
      dir.join("ende.lock")
    );
  }
}
