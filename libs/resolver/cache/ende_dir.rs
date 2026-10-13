// Copyright 2018-2026 the Deno authors. MIT license.

use std::path::PathBuf;

use ende_cache_dir::EndeDirResolutionError;
use ende_cache_dir::ResolveEndeDirSys;

use super::DiskCache;
use super::DiskCacheSys;

#[derive(Debug, Clone)]
pub struct EndeDirOptions {
  pub maybe_initial_cwd: Option<PathBuf>,
  pub maybe_custom_root: Option<PathBuf>,
}

#[sys_traits::auto_impl]
pub trait EndeDirSys: DiskCacheSys + ResolveEndeDirSys + Clone {}

#[allow(clippy::disallowed_types, reason = "definition")]
pub type EndeDirProviderRc<TSys> =
  ende_maybe_sync::MaybeArc<EndeDirProvider<TSys>>;

/// Lazily creates the deno dir which might be useful in scenarios
/// where functionality wants to continue if the ENDE_DIR can't be created.
pub struct EndeDirProvider<TSys: EndeDirSys> {
  options: EndeDirOptions,
  sys: TSys,
  ende_dir_cell: once_cell::sync::OnceCell<EndeDir<TSys>>,
}

impl<TSys: EndeDirSys> EndeDirProvider<TSys> {
  pub fn new(sys: TSys, options: EndeDirOptions) -> Self {
    Self {
      options,
      sys,
      ende_dir_cell: Default::default(),
    }
  }

  pub fn get_or_create(
    &self,
  ) -> Result<&EndeDir<TSys>, EndeDirResolutionError> {
    self.ende_dir_cell.get_or_try_init(|| {
      let path = ende_cache_dir::resolve_ende_dir(
        &self.sys,
        ende_cache_dir::ResolveEndeDirOptions {
          maybe_custom_root: self.options.maybe_custom_root.as_deref(),
          maybe_initial_cwd: self.options.maybe_initial_cwd.as_deref(),
        },
      )?;
      Ok(EndeDir::new(self.sys.clone(), path.into_owned()))
    })
  }
}

/// `EndeDir` serves as coordinator for multiple `DiskCache`s containing them
/// in single directory that can be controlled with `$ENDE_DIR` env variable.
#[derive(Debug, Clone)]
pub struct EndeDir<TSys: DiskCacheSys> {
  /// Example: /Users/rld/.deno/
  pub root: PathBuf,
  /// Used by TsCompiler to cache compiler output.
  pub gen_cache: DiskCache<TSys>,
}

impl<TSys: DiskCacheSys> EndeDir<TSys> {
  pub fn new(sys: TSys, root: PathBuf) -> Self {
    #[cfg(not(target_arch = "wasm32"))]
    assert!(root.is_absolute());

    let gen_path = root.join("gen");
    Self {
      root,
      gen_cache: DiskCache::new(sys, gen_path),
    }
  }

  /// The root directory of the ENDE_DIR for display purposes only.
  pub fn root_path_for_display(&self) -> std::path::Display<'_> {
    self.root.display()
  }

  /// Path for the V8 code cache.
  pub fn code_cache_db_file_path(&self) -> PathBuf {
    // bump this version name to invalidate the entire cache
    self.root.join("v8_code_cache_v2")
  }

  /// Path for the incremental cache used for formatting.
  pub fn fmt_incremental_cache_db_file_path(&self) -> PathBuf {
    // bump this version name to invalidate the entire cache
    self.root.join("fmt_incremental_cache_v2")
  }

  /// Path for the incremental cache used for linting.
  pub fn lint_incremental_cache_db_file_path(&self) -> PathBuf {
    // bump this version name to invalidate the entire cache
    self.root.join("lint_incremental_cache_v2")
  }

  /// Path for caching swc dependency analysis.
  pub fn dep_analysis_db_file_path(&self) -> PathBuf {
    // bump this version name to invalidate the entire cache
    self.root.join("dep_analysis_cache_v2")
  }

  /// Path for the cache used for fast check.
  pub fn fast_check_cache_db_file_path(&self) -> PathBuf {
    // bump this version name to invalidate the entire cache
    self.root.join("fast_check_cache_v2")
  }

  /// Path for caching node analysis.
  pub fn node_analysis_db_file_path(&self) -> PathBuf {
    // bump this version name to invalidate the entire cache
    self.root.join("node_analysis_cache_v2")
  }

  /// Path for the cache used for type checking.
  pub fn type_checking_cache_db_file_path(&self) -> PathBuf {
    // bump this version name to invalidate the entire cache
    self.root.join("check_cache_v2")
  }

  /// Path to the registries cache, used for the lps.
  pub fn registries_folder_path(&self) -> PathBuf {
    self.root.join("registries")
  }

  /// Path to the remote cache folder.
  pub fn remote_folder_path(&self) -> PathBuf {
    self.root.join("remote")
  }

  /// Path to the origin data cache folder.
  pub fn origin_data_folder_path(&self) -> PathBuf {
    // TODO(@crowlKats): change to origin_data for 2.0
    self.root.join("location_data")
  }

  /// File used for the upgrade checker.
  pub fn upgrade_check_file_path(&self) -> PathBuf {
    self.root.join("latest.txt")
  }

  /// Folder used for the npm cache.
  pub fn npm_folder_path(&self) -> PathBuf {
    self.root.join("npm")
  }

  /// Path used for the REPL history file.
  /// Can be overridden or disabled by setting `ENDE_REPL_HISTORY` environment variable.
  pub fn repl_history_file_path(&self) -> Option<PathBuf> {
    if let Some(ende_repl_history) = ende_env::var_os("ENDE_REPL_HISTORY") {
      if ende_repl_history.is_empty() {
        None
      } else {
        Some(PathBuf::from(ende_repl_history))
      }
    } else {
      Some(self.root.join("deno_history.txt"))
    }
  }

  /// Folder path used for downloading new versions of deno.
  pub fn dl_folder_path(&self) -> PathBuf {
    self.root.join("dl")
  }
}
