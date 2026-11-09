// Copyright 2026 the Ende authors. MIT license.

//! Finds the TypeScript 7 compiler (`tsc`) for `--unstable-tsc7`.
//!
//! Release builds embed the pinned compiler (cli/tsc7_sums.lock) when built
//! with `ENDE_TSC7_DIR` (see `embed_tsc7` in cli/build.rs). On first use it
//! is extracted to `<ende_dir>/tsc/<version>/<archive hash>/`, next to the
//! `lib.*.d.ts` files it loads from its own directory, and the SHA-256 of
//! `tsc` is checked. A `.verified` marker written after that check lets later
//! runs reuse the directory without hashing `tsc` again; they only compare
//! its size.
//!
//! An extraction is about 26 MB, so after extracting one, Ende removes the
//! others (other TypeScript versions and builds); an Ende that needs one
//! again extracts it again. A process that uses one holds a shared lock on
//! `<ende_dir>/tsc/.lock` until it exits, and removing takes the lock
//! exclusively, so it only happens while no other Ende uses a tsc from the
//! cache.

use std::fs::File;
use std::path::Path;
use std::path::PathBuf;

use ende_core::anyhow::Context;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::parking_lot::Mutex;
use sha2::Digest;

use crate::factory::CliFactory;

mod check;
mod libs;
mod options;
mod output;
mod project;

pub use check::Request;
pub use check::Tsc7Env;
pub use check::exec;

/// The embedded compiler: a zstd-compressed tar of `tsc[.exe]`, the
/// `lib.*.d.ts` files, LICENSE and NOTICE.txt.
pub struct Tsc7Bundle {
  pub version: &'static str,
  pub archive: &'static [u8],
  /// Names the cache directory, so different bundles never share one.
  pub archive_hash: &'static str,
  pub exe_name: &'static str,
  pub tsc_sha256: &'static str,
  pub tsc_len: u64,
}

// Defines `BUNDLE: Option<Tsc7Bundle>`, `None` unless built with
// `ENDE_TSC7_DIR`.
include!(concat!(env!("OUT_DIR"), "/tsc7_bundle.rs"));

const TSC_BIN_ENV: &str = "ENDE_TSC_BIN";
const VERIFIED_MARKER: &str = ".verified";

const CACHE_LOCK: &str = ".lock";

/// Shared locks on the cache, held until the process exits.
static CACHE_LOCKS: Mutex<Vec<File>> = Mutex::new(Vec::new());

/// Returns the path of the TypeScript 7 `tsc` to run: `ENDE_TSC_BIN` if set,
/// else the bundled one, extracted to the cache if it isn't there yet.
pub fn resolve_tsc7(factory: &CliFactory) -> Result<PathBuf, AnyError> {
  let (path, lock) = resolve(
    std::env::var_os(TSC_BIN_ENV).map(PathBuf::from),
    factory.cli_options()?.initial_cwd(),
    BUNDLE,
    || Ok(factory.ende_dir()?.root.join("tsc")),
  )?;
  CACHE_LOCKS.lock().extend(lock);
  Ok(path)
}

/// `cwd` resolves a relative `env_bin`: tsc runs in another directory.
/// Returns the path, and for the bundled tsc, the cache lock to hold while
/// using it.
fn resolve(
  env_bin: Option<PathBuf>,
  cwd: &Path,
  bundle: Option<Tsc7Bundle>,
  cache_root: impl FnOnce() -> Result<PathBuf, AnyError>,
) -> Result<(PathBuf, Option<File>), AnyError> {
  if let Some(path) = env_bin {
    let path = if path.has_root() {
      path
    } else {
      cwd.join(path)
    };
    if !path.is_file() {
      bail!(
        "{TSC_BIN_ENV} is set to {}, which isn't a file",
        path.display()
      );
    }
    return Ok((path, None));
  }
  let Some(bundle) = bundle else {
    bail!(
      "TypeScript 7 type checking isn't available in this build yet: it has \
       no bundled TypeScript 7 compiler (set {TSC_BIN_ENV} to the path of a \
       TypeScript 7 `tsc` to use one)"
    );
  };
  ensure_extracted(&bundle, &cache_root()?)
}

/// Extracts `bundle` under `root` unless an earlier run already did, and
/// returns the path of its `tsc` and the cache lock to hold while using it
/// (`None` if the file system can't lock). Safe to run from several
/// processes at once.
fn ensure_extracted(
  bundle: &Tsc7Bundle,
  root: &Path,
) -> Result<(PathBuf, Option<File>), AnyError> {
  let dir = root.join(bundle.version).join(bundle.archive_hash);
  let tsc = dir.join(bundle.exe_name);
  let lock = lock_cache(root);
  if is_verified(bundle, &dir) {
    return Ok((tsc, lock));
  }
  extract(bundle, &dir)?;
  let Some(lock) = lock else {
    return Ok((tsc, None));
  };
  let lock = remove_others(lock, root, &dir);
  // Another process may have removed this one between the locks.
  if !is_verified(bundle, &dir) {
    extract(bundle, &dir)?;
  }
  Ok((tsc, lock))
}

/// Takes a shared lock on the cache. Waits while another process removes
/// extractions.
fn lock_cache(root: &Path) -> Option<File> {
  let path = root.join(CACHE_LOCK);
  let lock = || -> std::io::Result<File> {
    std::fs::create_dir_all(root)?;
    let file = std::fs::OpenOptions::new()
      .create(true)
      .truncate(false)
      .write(true)
      .open(&path)?;
    file.lock_shared()?;
    Ok(file)
  };
  lock()
    .inspect_err(|err| {
      log::debug!("tsc7: failed to lock {}: {err}", path.display())
    })
    .ok()
}

/// Removes the extractions under `root` other than `keep` if no other
/// process holds the cache lock, then takes the shared lock again.
fn remove_others(lock: File, root: &Path, keep: &Path) -> Option<File> {
  let relock = || -> std::io::Result<File> {
    lock.unlock()?;
    if lock.try_lock().is_ok() {
      remove_extractions(root, keep);
      lock.unlock()?;
    }
    lock.lock_shared()?;
    Ok(lock)
  };
  relock()
    .inspect_err(|err| {
      log::debug!("tsc7: failed to lock {}: {err}", root.display())
    })
    .ok()
}

/// Removes every `<root>/<version>/<entry>` but `keep`, and the version
/// directories that leaves empty.
fn remove_extractions(root: &Path, keep: &Path) {
  let Ok(versions) = std::fs::read_dir(root) else {
    return;
  };
  for version in versions.flatten() {
    if !version.file_type().is_ok_and(|t| t.is_dir()) {
      continue;
    }
    let version_dir = version.path();
    let Ok(entries) = std::fs::read_dir(&version_dir) else {
      continue;
    };
    for entry in entries.flatten() {
      let path = entry.path();
      if path == keep {
        continue;
      }
      let result = if entry.file_type().is_ok_and(|t| t.is_dir()) {
        std::fs::remove_dir_all(&path)
      } else {
        std::fs::remove_file(&path)
      };
      match result {
        Ok(()) => log::debug!("tsc7: removed {}", path.display()),
        Err(err) => {
          log::debug!("tsc7: failed to remove {}: {err}", path.display())
        }
      }
    }
    // Fails, as it should, if `keep` is in it.
    let _ = std::fs::remove_dir(&version_dir);
  }
}

/// Extracts `bundle` to `dir`.
fn extract(bundle: &Tsc7Bundle, dir: &Path) -> Result<(), AnyError> {
  let parent = dir.parent().unwrap();
  std::fs::create_dir_all(parent)
    .with_context(|| format!("failed to create {}", parent.display()))?;
  // Extract into a sibling directory and rename it into place, so no process
  // ever sees a partly written `dir`. The marker goes in before the rename.
  let staging = tempfile::Builder::new()
    .prefix(".staging-")
    .tempdir_in(parent)
    .with_context(|| {
      format!("failed to create a directory in {}", parent.display())
    })?;
  unpack(bundle, staging.path())?;
  std::fs::write(
    staging.path().join(VERIFIED_MARKER),
    format!("{}\n", bundle.tsc_sha256),
  )?;
  install(bundle, staging, dir)
}

/// Whether `dir` holds a verified extraction of `bundle`. Checks the size of
/// `tsc` rather than its hash, which catches truncation and replacement
/// without reading 24 MB on every run.
fn is_verified(bundle: &Tsc7Bundle, dir: &Path) -> bool {
  let marker = std::fs::read_to_string(dir.join(VERIFIED_MARKER));
  let tsc_len = std::fs::metadata(dir.join(bundle.exe_name)).map(|m| m.len());
  matches!(marker, Ok(m) if m.trim() == bundle.tsc_sha256)
    && matches!(tsc_len, Ok(len) if len == bundle.tsc_len)
}

/// Unpacks the archive into `dest` and checks the SHA-256 of `tsc`.
fn unpack(bundle: &Tsc7Bundle, dest: &Path) -> Result<(), AnyError> {
  let decoder = zstd::stream::read::Decoder::new(bundle.archive)?;
  let mut archive = tar::Archive::new(decoder);
  // Files get the extraction time, not the archive's zero mtime.
  archive.set_preserve_mtime(false);
  archive
    .unpack(dest)
    .context("failed to extract the bundled TypeScript 7 compiler")?;

  let tsc = dest.join(bundle.exe_name);
  let contents = std::fs::read(&tsc)
    .with_context(|| format!("failed to read {}", tsc.display()))?;
  let actual = faster_hex::hex_string(&sha2::Sha256::digest(&contents));
  if actual != bundle.tsc_sha256 {
    bail!(
      "the bundled TypeScript {} compiler is corrupt: {} has SHA-256 \
       {actual}, expected {}",
      bundle.version,
      bundle.exe_name,
      bundle.tsc_sha256
    );
  }
  Ok(())
}

/// Moves the verified `staging` directory to `dir`, unless another process
/// got there first. `staging` is removed on drop if it is still there.
fn install(
  bundle: &Tsc7Bundle,
  staging: tempfile::TempDir,
  dir: &Path,
) -> Result<(), AnyError> {
  let move_err = |err: std::io::Error| {
    AnyError::from(err).context(format!(
      "failed to move the TypeScript 7 compiler to {}",
      dir.display()
    ))
  };
  let Err(err) = std::fs::rename(staging.path(), dir) else {
    return Ok(());
  };
  if is_verified(bundle, dir) {
    // Another process finished first.
    return Ok(());
  }
  if !dir.exists() {
    return Err(move_err(err));
  }
  // Left by an interrupted extraction, or `tsc` was changed since.
  std::fs::remove_dir_all(dir)
    .with_context(|| format!("failed to remove {}", dir.display()))?;
  if let Err(err) = std::fs::rename(staging.path(), dir)
    && !is_verified(bundle, dir)
  {
    return Err(move_err(err));
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  const FAKE_TSC: &[u8] = b"#!/bin/sh\necho 7.0.2\n";

  fn fake_bundle(tsc_sha256: &'static str) -> Tsc7Bundle {
    let mut tar = tar::Builder::new(Vec::new());
    for (name, contents, mode) in [
      ("tsc", FAKE_TSC, 0o755),
      ("lib.d.ts", b"// lib\n".as_slice(), 0o644),
    ] {
      let mut header = tar::Header::new_gnu();
      header.set_size(contents.len() as u64);
      header.set_mode(mode);
      tar.append_data(&mut header, name, contents).unwrap();
    }
    let archive = zstd::bulk::compress(&tar.into_inner().unwrap(), 3).unwrap();
    Tsc7Bundle {
      version: "7.0.2",
      archive: archive.leak(),
      archive_hash: "0123456789abcdef",
      exe_name: "tsc",
      tsc_sha256,
      tsc_len: FAKE_TSC.len() as u64,
    }
  }

  fn good_bundle() -> Tsc7Bundle {
    let sha256 = faster_hex::hex_string(&sha2::Sha256::digest(FAKE_TSC));
    fake_bundle(sha256.leak())
  }

  fn staging_dirs(root: &Path) -> Vec<String> {
    std::fs::read_dir(root.join("7.0.2"))
      .unwrap()
      .map(|e| e.unwrap().file_name().into_string().unwrap())
      .filter(|name| name.starts_with(".staging-"))
      .collect()
  }

  #[test]
  fn extracts_then_reuses() {
    let root = tempfile::tempdir().unwrap();
    let bundle = good_bundle();
    let tsc = ensure_extracted(&bundle, root.path()).unwrap().0;
    assert_eq!(tsc, root.path().join("7.0.2/0123456789abcdef/tsc"));
    assert_eq!(std::fs::read(&tsc).unwrap(), FAKE_TSC);
    let lib = tsc.with_file_name("lib.d.ts");
    assert_eq!(std::fs::read(&lib).unwrap(), b"// lib\n");
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      let mode = std::fs::metadata(&tsc).unwrap().permissions().mode();
      assert_eq!(mode & 0o111, 0o111, "tsc should be executable: {mode:o}");
    }

    // A second run reuses the directory instead of extracting again.
    std::fs::write(&lib, "changed").unwrap();
    assert_eq!(ensure_extracted(&bundle, root.path()).unwrap().0, tsc);
    assert_eq!(std::fs::read(&lib).unwrap(), b"changed");
    assert!(staging_dirs(root.path()).is_empty());
  }

  #[test]
  fn reextracts_a_corrupted_tsc() {
    let root = tempfile::tempdir().unwrap();
    let bundle = good_bundle();
    let tsc = ensure_extracted(&bundle, root.path()).unwrap().0;
    std::fs::write(&tsc, "truncated").unwrap();
    assert_eq!(ensure_extracted(&bundle, root.path()).unwrap().0, tsc);
    assert_eq!(std::fs::read(&tsc).unwrap(), FAKE_TSC);
  }

  #[test]
  fn reextracts_without_the_marker() {
    // A directory without the marker, as an older or interrupted run might
    // leave, is replaced.
    let root = tempfile::tempdir().unwrap();
    let bundle = good_bundle();
    let dir = root.path().join("7.0.2/0123456789abcdef");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tsc"), FAKE_TSC).unwrap();
    std::fs::write(dir.join("stray"), "").unwrap();
    ensure_extracted(&bundle, root.path()).unwrap();
    assert!(dir.join(VERIFIED_MARKER).exists());
    assert!(!dir.join("stray").exists());
  }

  #[test]
  fn rejects_a_corrupt_bundle() {
    let root = tempfile::tempdir().unwrap();
    let bundle = fake_bundle(
      "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let err = ensure_extracted(&bundle, root.path()).unwrap_err();
    assert!(
      err.to_string().starts_with(
        "the bundled TypeScript 7.0.2 compiler is corrupt: tsc has SHA-256"
      ),
      "{err}"
    );
    assert!(!root.path().join("7.0.2/0123456789abcdef").exists());
    assert!(staging_dirs(root.path()).is_empty());
  }

  #[test]
  fn keeps_the_copy_of_a_process_that_won() {
    let root = tempfile::tempdir().unwrap();
    let bundle = good_bundle();
    let tsc = ensure_extracted(&bundle, root.path()).unwrap().0;
    let lib = tsc.with_file_name("lib.d.ts");
    std::fs::write(&lib, "theirs").unwrap();

    // This process extracted too, but the other one renamed first.
    let staging = tempfile::Builder::new()
      .prefix(".staging-")
      .tempdir_in(root.path().join("7.0.2"))
      .unwrap();
    unpack(&bundle, staging.path()).unwrap();
    install(&bundle, staging, tsc.parent().unwrap()).unwrap();
    assert_eq!(std::fs::read(&lib).unwrap(), b"theirs");
    assert!(staging_dirs(root.path()).is_empty());
  }

  #[test]
  fn concurrent_extractions_agree() {
    let root = tempfile::tempdir().unwrap();
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
      let handles = (0..8)
        .map(|_| {
          scope.spawn(|| {
            let bundle = good_bundle();
            barrier.wait();
            ensure_extracted(&bundle, root.path()).unwrap().0
          })
        })
        .collect::<Vec<_>>();
      for handle in handles {
        let tsc = handle.join().unwrap();
        assert_eq!(std::fs::read(tsc).unwrap(), FAKE_TSC);
      }
    });
    assert!(staging_dirs(root.path()).is_empty());
  }

  /// `good_bundle` as another TypeScript version.
  fn newer_bundle() -> Tsc7Bundle {
    Tsc7Bundle {
      version: "7.0.3",
      archive_hash: "fedcba9876543210",
      ..good_bundle()
    }
  }

  #[test]
  fn extracting_a_new_build_removes_the_others() {
    let root = tempfile::tempdir().unwrap();
    let (old, lock) = ensure_extracted(&good_bundle(), root.path()).unwrap();
    drop(lock);
    // Another build of the same version, left by an earlier Ende.
    let sibling = root.path().join("7.0.2/aaaaaaaaaaaaaaaa");
    std::fs::create_dir_all(&sibling).unwrap();
    let new = ensure_extracted(&newer_bundle(), root.path()).unwrap().0;
    assert_eq!(std::fs::read(&new).unwrap(), FAKE_TSC);
    assert!(!old.exists());
    assert!(!sibling.exists());
    assert!(!root.path().join("7.0.2").exists());
  }

  #[test]
  fn keeps_a_build_another_process_uses() {
    let root = tempfile::tempdir().unwrap();
    // Held, as a running Ende holds it.
    let (old, _lock) = ensure_extracted(&good_bundle(), root.path()).unwrap();
    let (new, lock) = ensure_extracted(&newer_bundle(), root.path()).unwrap();
    assert!(old.exists());
    assert!(new.exists());
    // Still shared: a third process can lock it too.
    assert!(lock.is_some());
    let other = File::open(root.path().join(CACHE_LOCK)).unwrap();
    assert!(other.try_lock_shared().is_ok());
  }

  #[test]
  fn reextracts_a_build_removed_by_another_process() {
    let root = tempfile::tempdir().unwrap();
    let bundle = good_bundle();
    let (tsc, lock) = ensure_extracted(&bundle, root.path()).unwrap();
    drop(lock);
    // Another Ende extracted a newer one and removed this one.
    drop(ensure_extracted(&newer_bundle(), root.path()).unwrap());
    assert!(!tsc.exists());
    let (again, _lock) = ensure_extracted(&bundle, root.path()).unwrap();
    assert_eq!(again, tsc);
    assert_eq!(std::fs::read(&tsc).unwrap(), FAKE_TSC);
  }

  #[test]
  fn env_bin_must_exist() {
    let missing = PathBuf::from("/nonexistent/tsc");
    let err =
      resolve(Some(missing), Path::new("/"), Some(good_bundle()), || {
        unreachable!("ENDE_TSC_BIN skips the cache")
      })
      .unwrap_err();
    assert_eq!(
      err.to_string(),
      "ENDE_TSC_BIN is set to /nonexistent/tsc, which isn't a file"
    );
  }

  #[test]
  fn env_bin_wins_over_the_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tsc");
    std::fs::write(&path, "").unwrap();
    let resolved = resolve(
      Some(path.clone()),
      Path::new("/"),
      Some(good_bundle()),
      || unreachable!("ENDE_TSC_BIN skips the cache"),
    )
    .unwrap();
    assert_eq!(resolved.0, path);
    assert!(resolved.1.is_none());
  }

  #[test]
  fn relative_env_bin_is_resolved_against_cwd() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tsc"), "").unwrap();
    let resolved =
      resolve(Some(PathBuf::from("tsc")), dir.path(), None, || {
        unreachable!("ENDE_TSC_BIN skips the cache")
      })
      .unwrap()
      .0;
    assert_eq!(resolved, dir.path().join("tsc"));
  }

  #[test]
  fn no_bundle_mentions_env_bin() {
    let err =
      resolve(None, Path::new("/"), None, || unreachable!()).unwrap_err();
    let message = err.to_string();
    assert!(
      message.starts_with(
        "TypeScript 7 type checking isn't available in this build yet"
      ) && message.contains("ENDE_TSC_BIN"),
      "{message}"
    );
  }

  #[test]
  fn uses_the_bundle_cache() {
    let root = tempfile::tempdir().unwrap();
    let (tsc, lock) =
      resolve(None, Path::new("/"), Some(good_bundle()), || {
        Ok(root.path().to_path_buf())
      })
      .unwrap();
    assert_eq!(tsc, root.path().join("7.0.2/0123456789abcdef/tsc"));
    assert!(lock.is_some());
  }
}
