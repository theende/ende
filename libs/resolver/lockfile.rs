// Copyright 2018-2026 the Deno authors. MIT license.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Error as AnyError;
use capacity_builder::StringBuilder;
use deno_error::JsErrorBox;
use deno_path_util::fs::atomic_write_file_with_retries;
use deno_semver::package::PackageNv;
use ende_config::workspace::Workspace;
use ende_lockfile::Lockfile;
use ende_lockfile::WorkspaceMemberConfig;
use parking_lot::Mutex;
use parking_lot::MutexGuard;

/// Ende doesn't support npm packages, so npm entries in lockfiles written by
/// older versions are discarded. When upgrading an old lockfile format, the
/// npm package info the upgrade asks for is irrelevant, so empty info is
/// returned for each package.
pub struct NoNpmPackageInfoProvider;

#[async_trait::async_trait(?Send)]
impl ende_lockfile::NpmPackageInfoProvider for NoNpmPackageInfoProvider {
  async fn get_npm_package_info(
    &self,
    values: &[PackageNv],
  ) -> Result<
    Vec<ende_lockfile::Lockfile5NpmInfo>,
    Box<dyn std::error::Error + Send + Sync>,
  > {
    Ok(vec![Default::default(); values.len()])
  }
}

#[derive(Debug)]
pub struct LockfileReadFromPathOptions {
  pub file_path: PathBuf,
  pub frozen: bool,
  /// Causes the lockfile to only be read from, but not written to.
  pub skip_write: bool,
}

#[sys_traits::auto_impl]
pub trait LockfileSys:
  deno_path_util::fs::AtomicWriteFileWithRetriesSys
  + sys_traits::FsRead
  + sys_traits::FsCanonicalize
  + std::fmt::Debug
{
}

pub struct Guard<'a, T> {
  guard: MutexGuard<'a, T>,
}

impl<T> std::ops::Deref for Guard<'_, T> {
  type Target = T;

  fn deref(&self) -> &Self::Target {
    &self.guard
  }
}

impl<T> std::ops::DerefMut for Guard<'_, T> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.guard
  }
}

#[derive(Debug, Clone)]
pub struct LockfileFlags {
  pub no_lock: bool,
  pub frozen_lockfile: Option<bool>,
  pub lock: Option<PathBuf>,
  pub skip_write: bool,
  pub no_config: bool,
}

#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum LockfileWriteError {
  #[class(inherit)]
  #[error(transparent)]
  Changed(JsErrorBox),
  #[class(inherit)]
  #[error("Failed writing lockfile")]
  Io(#[source] std::io::Error),
}

#[allow(clippy::disallowed_types, reason = "definition")]
pub type LockfileLockRc<TSys> = ende_maybe_sync::MaybeArc<LockfileLock<TSys>>;

#[derive(Debug)]
pub struct LockfileLock<TSys: LockfileSys> {
  sys: TSys,
  lockfile: Mutex<Lockfile>,
  pub filename: PathBuf,
  frozen: bool,
  skip_write: bool,
}

impl<TSys: LockfileSys> LockfileLock<TSys> {
  /// Get the inner ende_lockfile::Lockfile.
  pub fn lock(&self) -> Guard<'_, Lockfile> {
    Guard {
      guard: self.lockfile.lock(),
    }
  }

  /// Creates an adapter for the lockfile that can be provided to deno_graph.
  #[cfg(feature = "graph")]
  pub fn as_deno_graph_locker(&self) -> DenoGraphLocker<'_, TSys> {
    DenoGraphLocker(self)
  }

  pub fn set_workspace_config(
    &self,
    options: ende_lockfile::SetWorkspaceConfigOptions,
  ) {
    self.lockfile.lock().set_workspace_config(options);
  }

  #[cfg(feature = "graph")]
  pub fn fill_graph(&self, graph: &mut deno_graph::ModuleGraph) {
    let lockfile = self.lockfile.lock();
    graph.fill_from_lockfile(deno_graph::FillFromLockfileOptions {
      redirects: lockfile
        .content
        .redirects
        .iter()
        .map(|(from, to)| (from.as_str(), to.as_str())),
      package_specifiers: lockfile
        .content
        .packages
        .specifiers
        .iter()
        .map(|(dep, id)| (dep, id.as_str())),
    });
  }

  pub fn overwrite(&self) -> bool {
    self.lockfile.lock().overwrite
  }

  pub fn write_if_changed(&self) -> Result<(), LockfileWriteError> {
    if self.skip_write {
      return Ok(());
    }

    self
      .error_if_changed()
      .map_err(LockfileWriteError::Changed)?;
    let mut lockfile = self.lockfile.lock();
    let Some(bytes) = lockfile.resolve_write_bytes() else {
      return Ok(()); // nothing to do
    };
    // If the lockfile path is a symlink, resolve it to its target so the
    // atomic write below replaces the target file rather than clobbering the
    // symlink with a freshly created regular file. This matches how the
    // ende.json/package.json writes follow symlinks (they write in place).
    let write_path = if self.sys.fs_is_symlink_no_err(&lockfile.filename) {
      self
        .sys
        .fs_canonicalize(&lockfile.filename)
        .unwrap_or_else(|_| lockfile.filename.clone())
    } else {
      lockfile.filename.clone()
    };
    // do an atomic write to reduce the chance of multiple ende
    // processes corrupting the file
    const CACHE_PERM: u32 = 0o644;
    atomic_write_file_with_retries(&self.sys, &write_path, &bytes, CACHE_PERM)
      .map_err(LockfileWriteError::Io)?;
    lockfile.has_content_changed = false;
    Ok(())
  }

  pub async fn discover(
    sys: TSys,
    flags: LockfileFlags,
    workspace: &Workspace,
    maybe_external_import_map: Option<&serde_json::Value>,
  ) -> Result<Option<Self>, AnyError> {
    if flags.no_lock {
      return Ok(None);
    }
    let file_path = match flags.lock {
      Some(path) => path,
      None => match workspace.resolve_lockfile_path(&sys)? {
        Some(path) => path,
        None => return Ok(None),
      },
    };
    let root_folder = workspace.root_folder_configs();
    let frozen = flags.frozen_lockfile.unwrap_or_else(|| {
      root_folder
        .ende_json
        .as_ref()
        .and_then(|c| c.to_lock_config().ok().flatten().map(|c| c.frozen()))
        .unwrap_or(false)
    });
    let lockfile = Self::read_from_path(
      sys,
      LockfileReadFromPathOptions {
        file_path,
        frozen,
        skip_write: flags.skip_write,
      },
    )
    .await?;
    let root_url = workspace.root_dir_url();
    let config = ende_lockfile::WorkspaceConfig {
      root: WorkspaceMemberConfig {
        // package.json is not supported in Ende
        package_json_deps: Default::default(),
        dependencies: without_npm_reqs(
          if let Some(map) = maybe_external_import_map {
            ende_config::import_map::import_map_deps_from_value(map)
              .collect::<HashSet<_>>()
          } else {
            root_folder
              .ende_json
              .as_deref()
              .map(|d| d.dependencies())
              .unwrap_or_default()
          },
        ),
      },
      members: workspace
        .config_folders()
        .iter()
        .filter(|(folder_url, _)| *folder_url != root_url)
        .filter_map(|(folder_url, folder)| {
          Some((
            {
              // should never be None here, but just ignore members that
              // do fail for this
              let mut relative_path = root_url.make_relative(folder_url)?;
              if relative_path.ends_with('/') {
                // make it slightly cleaner by removing the trailing slash
                relative_path.pop();
              }
              relative_path
            },
            {
              let config = WorkspaceMemberConfig {
                package_json_deps: Default::default(),
                dependencies: without_npm_reqs(
                  folder
                    .ende_json
                    .as_deref()
                    .map(|d| d.dependencies())
                    .unwrap_or_default(),
                ),
              };
              if config.dependencies.is_empty() {
                // exclude empty workspace members
                return None;
              }
              config
            },
          ))
        })
        .collect(),
      links: workspace
        .link_ende_jsons()
        .filter_map(|ende_json| {
          let name = ende_json.json.name.as_ref()?;
          let key = StringBuilder::<String>::build(|builder| {
            builder.append("jsr:");
            builder.append(name);
            if let Some(version) = &ende_json.json.version {
              builder.append('@');
              builder.append(version);
            }
          })
          .unwrap();
          let value = ende_lockfile::LockfileLinkContent {
            dependencies: without_npm_reqs(ende_json.dependencies()),
            optional_dependencies: Default::default(),
            peer_dependencies: Default::default(),
            peer_dependencies_meta: Default::default(),
          };
          Some((key, value))
        })
        .collect(),
      npm_overrides: None,
    };
    lockfile.set_workspace_config(ende_lockfile::SetWorkspaceConfigOptions {
      no_npm: false,
      no_config: flags.no_config,
      config,
    });
    Ok(Some(lockfile))
  }

  pub async fn read_from_path(
    sys: TSys,
    opts: LockfileReadFromPathOptions,
  ) -> Result<LockfileLock<TSys>, AnyError> {
    let lockfile = match sys.fs_read_to_string(&opts.file_path) {
      Ok(text) => {
        let mut lockfile = Lockfile::new(
          ende_lockfile::NewLockfileOptions {
            file_path: opts.file_path,
            content: &text,
            overwrite: false,
          },
          &NoNpmPackageInfoProvider,
        )
        .await?;
        // Dropping stale npm entries is not a change by itself: the file is
        // left as is (and `--frozen` passes) until something else causes a
        // write, which then omits them.
        lockfile.content.remove_npm_entries();
        lockfile
      }
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
        Lockfile::new_empty(opts.file_path, false)
      }
      Err(err) => {
        return Err(err).with_context(|| {
          format!("Failed reading lockfile '{}'", opts.file_path.display())
        });
      }
    };
    Ok(LockfileLock {
      sys,
      filename: lockfile.filename.clone(),
      lockfile: Mutex::new(lockfile),
      frozen: opts.frozen,
      skip_write: opts.skip_write,
    })
  }

  pub fn error_if_changed(&self) -> Result<(), JsErrorBox> {
    if !self.frozen {
      return Ok(());
    }
    let lockfile = self.lockfile.lock();
    if lockfile.has_content_changed {
      let contents = self
        .sys
        .fs_read_to_string(&lockfile.filename)
        .unwrap_or_default();
      let new_contents = lockfile.as_json_string();
      let diff = crate::display::diff(&contents, &new_contents);
      // has an extra newline at the end
      let diff = diff.trim_end();
      const MAX_DIFF_LINES: usize = 50;
      let truncated_diff = {
        let lines: Vec<&str> = diff.lines().collect();
        if lines.len() > MAX_DIFF_LINES {
          let shown: String = lines[..MAX_DIFF_LINES].join("\n");
          let remaining = lines.len() - MAX_DIFF_LINES;
          format!("{shown}\n... {remaining} more lines omitted ...")
        } else {
          diff.to_string()
        }
      };
      Err(JsErrorBox::generic(format!(
        "The lockfile is out of date. Run `ende install --frozen=false`, or rerun with `--frozen=false` to update it.\nchanges:\n{truncated_diff}"
      )))
    } else {
      Ok(())
    }
  }
}

/// An adapter to use the lockfile with `deno_graph`.
#[cfg(feature = "graph")]
pub struct DenoGraphLocker<'a, TSys: LockfileSys>(&'a LockfileLock<TSys>);

#[cfg(feature = "graph")]
impl<TSys: LockfileSys> deno_graph::source::Locker
  for DenoGraphLocker<'_, TSys>
{
  fn get_remote_checksum(
    &self,
    specifier: &url::Url,
  ) -> Option<deno_graph::source::LoaderChecksum> {
    self
      .0
      .lock()
      .remote()
      .get(specifier.as_str())
      .map(|s| deno_graph::source::LoaderChecksum::new(s.clone()))
  }

  fn has_remote_checksum(&self, specifier: &url::Url) -> bool {
    self.0.lock().remote().contains_key(specifier.as_str())
  }

  fn set_remote_checksum(
    &mut self,
    specifier: &url::Url,
    checksum: deno_graph::source::LoaderChecksum,
  ) {
    self
      .0
      .lock()
      .insert_remote(specifier.to_string(), checksum.into_string())
  }

  fn get_pkg_manifest_checksum(
    &self,
    package_nv: &PackageNv,
  ) -> Option<deno_graph::source::LoaderChecksum> {
    self
      .0
      .lock()
      .content
      .packages
      .jsr
      .get(package_nv)
      .map(|s| deno_graph::source::LoaderChecksum::new(s.integrity.clone()))
  }

  fn set_pkg_manifest_checksum(
    &mut self,
    package_nv: &PackageNv,
    checksum: deno_graph::source::LoaderChecksum,
  ) {
    // a value would only exist in here if two workers raced
    // to insert the same package manifest checksum
    self
      .0
      .lock()
      .insert_package(package_nv.clone(), checksum.into_string());
  }
}

/// npm packages aren't supported, so npm requirements never go in the
/// lockfile.
fn without_npm_reqs(
  reqs: HashSet<deno_semver::jsr::JsrDepPackageReq>,
) -> HashSet<deno_semver::jsr::JsrDepPackageReq> {
  reqs
    .into_iter()
    .filter(|req| req.kind != deno_semver::package::PackageKind::Npm)
    .collect()
}
