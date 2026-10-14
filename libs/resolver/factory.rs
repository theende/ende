// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::path::PathBuf;

use boxed_error::Boxed;
use ende_cache_dir::EndeDirResolutionError;
use ende_cache_dir::GlobalHttpCacheRc;
use ende_cache_dir::GlobalOrLocalHttpCache;
use ende_cache_dir::LocalHttpCache;
use ende_config::ende_json::MinimumDependencyAgeConfig;
use ende_config::ende_json::NewestDependencyDate;
use ende_config::workspace::VendorEnablement;
use ende_config::workspace::WorkspaceDirectory;
use ende_config::workspace::WorkspaceDirectoryEmptyOptions;
use ende_config::workspace::WorkspaceDirectoryRc;
use ende_config::workspace::WorkspaceDiscoverError;
use ende_config::workspace::WorkspaceDiscoverOptions;
use ende_config::workspace::WorkspaceDiscoverStart;
use ende_maybe_sync::MaybeSend;
use ende_maybe_sync::MaybeSync;
use ende_maybe_sync::new_rc;
use futures::future::FutureExt;
use thiserror::Error;
use url::Url;

use crate::EndeResolverOptions;
use crate::RawEndeResolver;
use crate::RawEndeResolverRc;
use crate::WorkspaceResolverRc;
use crate::cache::EmitCache;
use crate::cache::EmitCacheRc;
use crate::cache::EndeDir;
use crate::cache::EndeDirOptions;
use crate::cache::EndeDirProvider;
use crate::cache::EndeDirProviderRc;
use crate::cache::EndeDirSys;
use crate::ende_json::CompilerOptionsOverrides;
use crate::ende_json::CompilerOptionsResolver;
use crate::ende_json::CompilerOptionsResolverRc;
use crate::import_map::WorkspaceExternalImportMapLoader;
use crate::import_map::WorkspaceExternalImportMapLoaderRc;
use crate::loader::AllowJsonImports;
use crate::lockfile::LockfileLock;
use crate::lockfile::LockfileLockRc;
use crate::workspace::FsCacheOptions;
use crate::workspace::SloppyImportsOptions;
use crate::workspace::WorkspaceResolver;

const DEFAULT_MINIMUM_DEPENDENCY_AGE_MINUTES: i64 = 1440;

// todo(https://github.com/rust-lang/rust/issues/109737): remove once_cell after get_or_try_init is stabilized
#[cfg(feature = "sync")]
type Deferred<T> = once_cell::sync::OnceCell<T>;
#[cfg(not(feature = "sync"))]
type Deferred<T> = once_cell::unsync::OnceCell<T>;

#[cfg(feature = "graph")]
#[allow(clippy::disallowed_types, reason = "definition")]
pub type JsrVersionResolverRc =
  ende_maybe_sync::MaybeArc<deno_graph::packages::JsrVersionResolver>;

#[derive(Debug, Boxed)]
pub struct HttpCacheCreateError(pub Box<HttpCacheCreateErrorKind>);

#[derive(Debug, Error)]
pub enum HttpCacheCreateErrorKind {
  #[error(transparent)]
  EndeDirResolution(#[from] EndeDirResolutionError),
  #[error(transparent)]
  WorkspaceDiscover(#[from] WorkspaceDiscoverError),
}

#[derive(Debug, Default)]
pub enum ConfigDiscoveryOption {
  #[default]
  DiscoverCwd,
  Discover {
    start_paths: Vec<PathBuf>,
  },
  Path(PathBuf),
  Disabled,
}

/// Resolves the JSR registry URL to use for the given system.
pub fn resolve_jsr_url(sys: &impl sys_traits::EnvVar) -> Url {
  let env_var_name = "JSR_URL";
  if let Ok(registry_url) = sys.env_var(env_var_name) {
    // ensure there is a trailing slash for the directory
    let registry_url = format!("{}/", registry_url.trim_end_matches('/'));
    match Url::parse(&registry_url) {
      Ok(url) => {
        return url;
      }
      Err(err) => {
        log::debug!("Invalid {} environment variable: {:#}", env_var_name, err,);
      }
    }
  }

  Url::parse("https://jsr.io/").unwrap()
}

#[async_trait::async_trait(?Send)]
pub trait SpecifiedImportMapProvider:
  std::fmt::Debug + MaybeSend + MaybeSync
{
  async fn get(
    &self,
  ) -> Result<Option<crate::workspace::SpecifiedImportMap>, anyhow::Error>;
}

#[derive(Debug, Default)]
pub struct WorkspaceFactoryOptions {
  pub additional_config_file_names: &'static [&'static str],
  pub config_discovery: ConfigDiscoveryOption,
  pub frozen_lockfile: Option<bool>,
  pub lock_arg: Option<PathBuf>,
  /// Whether to skip writing to the lockfile.
  pub lockfile_skip_write: bool,
  pub maybe_custom_ende_dir_root: Option<PathBuf>,
  pub no_lock: bool,
  pub vendor: Option<bool>,
}

#[allow(clippy::disallowed_types, reason = "definition")]
pub type WorkspaceFactoryRc<TSys> =
  ende_maybe_sync::MaybeArc<WorkspaceFactory<TSys>>;

#[sys_traits::auto_impl]
pub trait WorkspaceFactorySys:
  EndeDirSys
  + crate::lockfile::LockfileSys
  + crate::EndeResolverSys
  + ende_cache_dir::GlobalHttpCacheSys
  + ende_cache_dir::LocalHttpCacheSys
  + sys_traits::SystemTimeNow
{
}

pub struct WorkspaceFactory<TSys: WorkspaceFactorySys> {
  sys: TSys,
  ende_dir_provider: Deferred<EndeDirProviderRc<TSys>>,
  emit_cache: Deferred<EmitCacheRc<TSys>>,
  global_http_cache: Deferred<GlobalHttpCacheRc<TSys>>,
  http_cache: Deferred<GlobalOrLocalHttpCache<TSys>>,
  jsr_url: Deferred<Url>,
  lockfile: async_once_cell::OnceCell<Option<LockfileLockRc<TSys>>>,
  workspace_directory: Deferred<WorkspaceDirectoryRc>,
  workspace_external_import_map_loader:
    Deferred<WorkspaceExternalImportMapLoaderRc<TSys>>,
  initial_cwd: PathBuf,
  options: WorkspaceFactoryOptions,
}

impl<TSys: WorkspaceFactorySys> WorkspaceFactory<TSys> {
  pub fn new(
    sys: TSys,
    initial_cwd: PathBuf,
    options: WorkspaceFactoryOptions,
  ) -> Self {
    Self {
      sys,
      ende_dir_provider: Default::default(),
      emit_cache: Default::default(),
      global_http_cache: Default::default(),
      http_cache: Default::default(),
      jsr_url: Default::default(),
      lockfile: Default::default(),
      workspace_directory: Default::default(),
      workspace_external_import_map_loader: Default::default(),
      initial_cwd,
      options,
    }
  }

  pub fn set_workspace_directory(
    &mut self,
    workspace_directory: WorkspaceDirectoryRc,
  ) {
    self.workspace_directory = Deferred::from(workspace_directory);
  }

  pub fn ende_dir_provider(&self) -> &EndeDirProviderRc<TSys> {
    self.ende_dir_provider.get_or_init(|| {
      new_rc(EndeDirProvider::new(
        self.sys.clone(),
        EndeDirOptions {
          maybe_initial_cwd: Some(self.initial_cwd.clone()),
          maybe_custom_root: self.options.maybe_custom_ende_dir_root.clone(),
        },
      ))
    })
  }

  pub fn emit_cache(&self) -> Result<&EmitCacheRc<TSys>, anyhow::Error> {
    self.emit_cache.get_or_try_init(|| {
      Ok(new_rc(EmitCache::new(
        &self.sys,
        self.ende_dir()?.gen_cache.clone(),
        #[cfg(feature = "deno_ast")]
        Cow::Borrowed(deno_ast::VERSION),
        #[cfg(not(feature = "deno_ast"))]
        Cow::Borrowed(env!("CARGO_PKG_VERSION")),
      )))
    })
  }

  pub fn jsr_url(&self) -> &Url {
    self.jsr_url.get_or_init(|| resolve_jsr_url(&self.sys))
  }

  pub fn initial_cwd(&self) -> &PathBuf {
    &self.initial_cwd
  }

  pub fn ende_dir(&self) -> Result<&EndeDir<TSys>, EndeDirResolutionError> {
    self.ende_dir_provider().get_or_create()
  }

  pub fn global_http_cache(
    &self,
  ) -> Result<&GlobalHttpCacheRc<TSys>, EndeDirResolutionError> {
    self.global_http_cache.get_or_try_init(|| {
      let global_cache_dir = self.ende_dir()?.remote_folder_path();
      let global_http_cache = new_rc(ende_cache_dir::GlobalHttpCache::new(
        self.sys.clone(),
        global_cache_dir,
      ));
      Ok(global_http_cache)
    })
  }

  pub fn http_cache(
    &self,
  ) -> Result<&ende_cache_dir::GlobalOrLocalHttpCache<TSys>, HttpCacheCreateError>
  {
    self.http_cache.get_or_try_init(|| {
      let global_cache = self.global_http_cache()?.clone();
      match self.workspace_directory()?.workspace.vendor_dir_path() {
        Some(local_path) => {
          let local_cache = LocalHttpCache::new(
            local_path.clone(),
            global_cache,
            ende_cache_dir::GlobalToLocalCopy::Allow,
            self.jsr_url().clone(),
          );
          Ok(new_rc(local_cache).into())
        }
        None => Ok(global_cache.into()),
      }
    })
  }

  pub async fn maybe_lockfile(
    &self,
  ) -> Result<Option<&LockfileLockRc<TSys>>, anyhow::Error> {
    self
      .lockfile
      .get_or_try_init(async move {
        let workspace_directory = self.workspace_directory()?;
        let maybe_external_import_map =
          self.workspace_external_import_map_loader()?.get_or_load()?;

        let maybe_lock_file = LockfileLock::discover(
          self.sys().clone(),
          crate::lockfile::LockfileFlags {
            no_lock: self.options.no_lock,
            frozen_lockfile: self.options.frozen_lockfile,
            lock: self.options.lock_arg.as_ref().map(|path| {
              #[cfg(not(target_arch = "wasm32"))]
              debug_assert!(path.is_absolute());
              path.clone()
            }),
            skip_write: self.options.lockfile_skip_write,
            no_config: matches!(
              self.options.config_discovery,
              ConfigDiscoveryOption::Disabled
            ),
          },
          &workspace_directory.workspace,
          maybe_external_import_map.as_ref().map(|v| &v.value),
        )
        .await?
        .map(ende_maybe_sync::new_rc);

        Ok(maybe_lock_file)
      })
      .await
      .map(|c| c.as_ref())
  }

  pub fn sys(&self) -> &TSys {
    &self.sys
  }

  pub fn workspace_directory(
    &self,
  ) -> Result<&WorkspaceDirectoryRc, WorkspaceDiscoverError> {
    self.workspace_directory.get_or_try_init(|| {
      let maybe_vendor_override = self.options.vendor.map(|v| match v {
        true => VendorEnablement::Enable {
          cwd: &self.initial_cwd,
        },
        false => VendorEnablement::Disable,
      });
      let resolve_workspace_discover_options = || WorkspaceDiscoverOptions {
        ende_json_cache: None,
        workspace_cache: None,
        additional_config_file_names: self.options.additional_config_file_names,
        maybe_vendor_override,
      };
      let resolve_empty_options = || WorkspaceDirectoryEmptyOptions {
        root_dir: new_rc(
          deno_path_util::url_from_directory_path(&self.initial_cwd).unwrap(),
        ),
        use_vendor_dir: maybe_vendor_override
          .unwrap_or(VendorEnablement::Disable),
      };

      let dir = match &self.options.config_discovery {
        ConfigDiscoveryOption::DiscoverCwd => WorkspaceDirectory::discover(
          &self.sys,
          WorkspaceDiscoverStart::Paths(std::slice::from_ref(
            &self.initial_cwd,
          )),
          &resolve_workspace_discover_options(),
        )?,
        ConfigDiscoveryOption::Discover { start_paths } => {
          WorkspaceDirectory::discover(
            &self.sys,
            WorkspaceDiscoverStart::Paths(start_paths),
            &resolve_workspace_discover_options(),
          )?
        }
        ConfigDiscoveryOption::Path(path) => {
          #[cfg(not(target_arch = "wasm32"))]
          debug_assert!(path.is_absolute());
          WorkspaceDirectory::discover(
            &self.sys,
            WorkspaceDiscoverStart::ConfigFile(path),
            &resolve_workspace_discover_options(),
          )?
        }
        ConfigDiscoveryOption::Disabled => {
          WorkspaceDirectory::empty(resolve_empty_options())
        }
      };
      Ok(dir)
    })
  }

  pub fn workspace_external_import_map_loader(
    &self,
  ) -> Result<&WorkspaceExternalImportMapLoaderRc<TSys>, WorkspaceDiscoverError>
  {
    self
      .workspace_external_import_map_loader
      .get_or_try_init(|| {
        Ok(new_rc(WorkspaceExternalImportMapLoader::new(
          self.sys().clone(),
          self.workspace_directory()?.workspace.clone(),
        )))
      })
  }
}

#[derive(Default)]
pub struct ResolverFactoryOptions {
  pub compiler_options_overrides: CompilerOptionsOverrides,
  /// Prevents installing packages newer than the specified date.
  pub newest_dependency_date: Option<NewestDependencyDate>,
  pub specified_import_map: Option<Box<dyn SpecifiedImportMapProvider>>,
  pub unstable_sloppy_imports: bool,
  #[cfg(feature = "graph")]
  pub on_mapped_resolution_diagnostic:
    Option<crate::graph::OnMappedResolutionDiagnosticFn>,
  pub allow_json_imports: AllowJsonImports,
}

pub struct ResolverFactory<TSys: WorkspaceFactorySys> {
  options: ResolverFactoryOptions,
  compiler_options_resolver: Deferred<CompilerOptionsResolverRc>,
  #[cfg(feature = "graph")]
  ende_resolver: async_once_cell::OnceCell<crate::graph::EndeResolverRc<TSys>>,
  #[cfg(feature = "deno_ast")]
  emitter: Deferred<crate::emit::EmitterRc<TSys>>,
  #[cfg(feature = "graph")]
  jsr_version_resolver: Deferred<JsrVersionResolverRc>,
  minimum_dependency_age: Deferred<MinimumDependencyAgeConfig>,
  #[cfg(feature = "deno_ast")]
  parsed_source_cache: crate::cache::ParsedSourceCacheRc,
  #[cfg(all(feature = "graph", feature = "deno_ast"))]
  module_loader: Deferred<crate::loader::ModuleLoaderRc<TSys>>,
  raw_ende_resolver: async_once_cell::OnceCell<RawEndeResolverRc<TSys>>,
  workspace_factory: WorkspaceFactoryRc<TSys>,
  workspace_resolver: async_once_cell::OnceCell<WorkspaceResolverRc<TSys>>,
}

impl<TSys: WorkspaceFactorySys> ResolverFactory<TSys> {
  pub fn new(
    workspace_factory: WorkspaceFactoryRc<TSys>,
    options: ResolverFactoryOptions,
  ) -> Self {
    Self {
      compiler_options_resolver: Default::default(),
      raw_ende_resolver: Default::default(),
      #[cfg(feature = "graph")]
      ende_resolver: Default::default(),
      #[cfg(feature = "deno_ast")]
      emitter: Default::default(),
      #[cfg(feature = "graph")]
      jsr_version_resolver: Default::default(),
      minimum_dependency_age: Default::default(),
      #[cfg(feature = "deno_ast")]
      parsed_source_cache: Default::default(),
      #[cfg(all(feature = "graph", feature = "deno_ast"))]
      module_loader: Default::default(),
      workspace_factory,
      workspace_resolver: Default::default(),
      options,
    }
  }

  pub async fn raw_ende_resolver(
    &self,
  ) -> Result<&RawEndeResolverRc<TSys>, anyhow::Error> {
    self
      .raw_ende_resolver
      .get_or_try_init(
        async {
          Ok(new_rc(RawEndeResolver::new(EndeResolverOptions {
            maybe_vendor_dir: self
              .workspace_factory
              .workspace_directory()?
              .workspace
              .vendor_dir_path(),
            workspace_resolver: self.workspace_resolver().await?.clone(),
          })))
        }
        // boxed to prevent the futures getting big and exploding the stack
        .boxed_local(),
      )
      .await
  }

  pub fn compiler_options_resolver(
    &self,
  ) -> Result<&CompilerOptionsResolverRc, anyhow::Error> {
    self.compiler_options_resolver.get_or_try_init(|| {
      Ok(new_rc(CompilerOptionsResolver::new(
        self.workspace_factory.sys(),
        &self.workspace_factory.workspace_directory()?.workspace,
        &self.workspace_factory.options.config_discovery,
        &self.options.compiler_options_overrides,
      )))
    })
  }

  #[cfg(feature = "deno_ast")]
  pub fn emitter(
    &self,
  ) -> Result<&crate::emit::EmitterRc<TSys>, anyhow::Error> {
    self.emitter.get_or_try_init(|| {
      Ok(new_rc(crate::emit::Emitter::new(
        self.workspace_factory.emit_cache()?.clone(),
        self.parsed_source_cache().clone(),
        self.compiler_options_resolver()?.clone(),
      )))
    })
  }

  #[cfg(feature = "graph")]
  pub async fn ende_resolver(
    &self,
  ) -> Result<&crate::graph::EndeResolverRc<TSys>, anyhow::Error> {
    self
      .ende_resolver
      .get_or_try_init(async {
        Ok(new_rc(crate::graph::EndeResolver::new(
          self.raw_ende_resolver().await?.clone(),
          self.options.on_mapped_resolution_diagnostic.clone(),
        )))
      })
      .await
  }

  #[cfg(feature = "graph")]
  pub fn jsr_version_resolver(
    &self,
  ) -> Result<&JsrVersionResolverRc, anyhow::Error> {
    self.jsr_version_resolver.get_or_try_init(|| {
      let minimum_dependency_age_config =
        self.minimum_dependency_age_config()?;

      let (exclude_jsr_pkgs, exclude_jsr_pkg_prefixes) =
        split_minimum_dependency_age_excludes(
          &minimum_dependency_age_config.exclude,
        );

      Ok(new_rc(deno_graph::packages::JsrVersionResolver {
        newest_dependency_date_options:
          deno_graph::packages::NewestDependencyDateOptions {
            date: minimum_dependency_age_config
              .age
              .as_ref()
              .and_then(|d| d.into_option())
              .map(deno_graph::packages::NewestDependencyDate),
            exclude_jsr_pkgs,
            exclude_jsr_pkg_prefixes,
          },
      }))
    })
  }

  /// The newest allowed dependency date.
  pub fn minimum_dependency_age_config(
    &self,
  ) -> Result<&MinimumDependencyAgeConfig, anyhow::Error> {
    self.minimum_dependency_age.get_or_try_init(|| {
      let workspace_factory = self.workspace_factory();
      let mut config = if let Some(date) = self.options.newest_dependency_date {
        MinimumDependencyAgeConfig {
          age: Some(date),
          exclude: Vec::new(),
        }
      } else {
        let workspace = &workspace_factory.workspace_directory()?.workspace;
        workspace.minimum_dependency_age(workspace_factory.sys())?
      };
      if config.age.is_none() {
        apply_minimum_dependency_age_fallbacks(
          &mut config,
          minimum_dependency_age_from_env(workspace_factory.sys()),
          chrono::DateTime::<chrono::Utc>::from(
            workspace_factory.sys().sys_time_now(),
          ),
        );
      }
      if let Some(newest_dependency_date) =
        config.age.and_then(|d| d.into_option())
      {
        log::debug!("Newest dependency date: {}", newest_dependency_date);
      }
      Ok(config)
    })
  }

  #[cfg(feature = "deno_ast")]
  pub fn parsed_source_cache(&self) -> &crate::cache::ParsedSourceCacheRc {
    &self.parsed_source_cache
  }

  #[cfg(all(feature = "graph", feature = "deno_ast"))]
  pub fn module_loader(
    &self,
  ) -> Result<&crate::loader::ModuleLoaderRc<TSys>, anyhow::Error> {
    self.module_loader.get_or_try_init(|| {
      Ok(new_rc(crate::loader::ModuleLoader::new(
        self.emitter()?.clone(),
        self.parsed_source_cache.clone(),
        self.workspace_factory.sys.clone(),
        self.options.allow_json_imports,
      )))
    })
  }

  pub fn workspace_factory(&self) -> &WorkspaceFactoryRc<TSys> {
    &self.workspace_factory
  }

  pub async fn workspace_resolver(
    &self,
  ) -> Result<&WorkspaceResolverRc<TSys>, anyhow::Error> {
    self
      .workspace_resolver
      .get_or_try_init(
        async {
          let directory = self.workspace_factory.workspace_directory()?;
          let workspace = &directory.workspace;
          let specified_import_map = match &self.options.specified_import_map {
            Some(import_map) => import_map.get().await?,
            None => None,
          };
          let options = crate::workspace::CreateResolverOptions {
            specified_import_map,
            sloppy_imports_options: if self.options.unstable_sloppy_imports
              || workspace.has_unstable("sloppy-imports")
            {
              SloppyImportsOptions::Enabled
            } else {
              SloppyImportsOptions::Unspecified
            },
            fs_cache_options: FsCacheOptions::Enabled,
          };
          let resolver = WorkspaceResolver::from_workspace(
            workspace,
            self.workspace_factory.sys.clone(),
            options,
          )?;
          resolver.set_compiler_options_resolver(
            self.compiler_options_resolver()?.clone(),
          );
          if !resolver.diagnostics().is_empty() {
            // todo(dsherret): do not log this in this crate... that should be
            // a CLI responsibility
            log::warn!(
              "Resolver diagnostics:\n{}",
              resolver
                .diagnostics()
                .iter()
                .map(|d| format!("  - {d}"))
                .collect::<Vec<_>>()
                .join("\n")
            );
          }
          Ok(new_rc(resolver))
        }
        // boxed to prevent the futures getting big and exploding the stack
        .boxed_local(),
      )
      .await
  }
}

/// Splits the `jsr:` entries of `minimumDependencyAge.exclude` into exact
/// package names and package name prefixes (from trailing `*` wildcard entries
/// like `jsr:@scope/*`).
#[cfg(feature = "graph")]
fn split_minimum_dependency_age_excludes(
  exclude: &[String],
) -> (
  std::collections::BTreeSet<deno_semver::package::PackageName>,
  Vec<deno_semver::package::PackageName>,
) {
  let mut exact = std::collections::BTreeSet::new();
  let mut prefixes = Vec::new();
  for entry in exclude.iter().filter_map(|v| v.strip_prefix("jsr:")) {
    match entry.strip_suffix('*') {
      // an empty prefix (from an invalid entry like `jsr:*` that a config
      // diagnostic warns about) would exclude every package
      Some(prefix) if !prefix.is_empty() => prefixes.push(prefix.into()),
      Some(_) => {}
      None => {
        exact.insert(entry.into());
      }
    }
  }
  (exact, prefixes)
}

/// Reads the `ENDE_MINIMUM_DEPENDENCY_AGE` environment variable, which takes
/// the same values as `--minimum-dependency-age` (`0` disables the check).
/// It only applies when neither the flag nor ende.json configure the age.
fn minimum_dependency_age_from_env(
  sys: &(impl sys_traits::EnvVar + sys_traits::SystemTimeNow),
) -> Option<NewestDependencyDate> {
  let value = ende_env::env_var(sys, "ENDE_MINIMUM_DEPENDENCY_AGE").ok()?;
  match ende_config::parse_minutes_duration_or_date(sys, value.trim()) {
    Ok(date) => Some(date),
    Err(err) => {
      log::warn!("Ignoring invalid ENDE_MINIMUM_DEPENDENCY_AGE: {err:#}");
      None
    }
  }
}

fn apply_minimum_dependency_age_fallbacks(
  config: &mut MinimumDependencyAgeConfig,
  env_age: Option<NewestDependencyDate>,
  now: chrono::DateTime<chrono::Utc>,
) {
  if config.age.is_some() {
    return;
  }

  // Fall back to the `ENDE_MINIMUM_DEPENDENCY_AGE` environment variable when
  // not configured in ende.json, then to the default.
  config.age = Some(env_age.unwrap_or_else(|| {
    NewestDependencyDate::Enabled(
      now - chrono::Duration::minutes(DEFAULT_MINIMUM_DEPENDENCY_AGE_MINUTES),
    )
  }));
}

#[cfg(test)]
mod tests {
  use chrono::TimeZone;
  use ende_config::ende_json::MinimumDependencyAgeConfig;
  use ende_config::ende_json::NewestDependencyDate;

  use super::apply_minimum_dependency_age_fallbacks;

  fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap()
  }

  #[test]
  fn minimum_dependency_age_falls_back_to_default() {
    let mut config = MinimumDependencyAgeConfig::default();
    apply_minimum_dependency_age_fallbacks(&mut config, None, now());
    assert_eq!(
      config.age,
      Some(NewestDependencyDate::Enabled(
        now() - chrono::Duration::minutes(1440)
      ))
    );
  }

  #[test]
  fn minimum_dependency_age_preserves_exclude_with_default() {
    let mut config = MinimumDependencyAgeConfig {
      age: None,
      exclude: vec!["jsr:@std/fs".to_string()],
    };
    apply_minimum_dependency_age_fallbacks(&mut config, None, now());
    assert_eq!(
      config.age,
      Some(NewestDependencyDate::Enabled(
        now() - chrono::Duration::minutes(1440)
      ))
    );
    assert_eq!(config.exclude, vec!["jsr:@std/fs".to_string()]);
  }

  #[test]
  fn minimum_dependency_age_prefers_env_over_default() {
    let mut config = MinimumDependencyAgeConfig::default();
    let env_age =
      NewestDependencyDate::Enabled(now() - chrono::Duration::days(2));
    apply_minimum_dependency_age_fallbacks(&mut config, Some(env_age), now());
    assert_eq!(config.age, Some(env_age));
  }

  #[test]
  fn minimum_dependency_age_env_can_disable() {
    let mut config = MinimumDependencyAgeConfig::default();
    apply_minimum_dependency_age_fallbacks(
      &mut config,
      Some(NewestDependencyDate::Disabled),
      now(),
    );
    assert_eq!(config.age, Some(NewestDependencyDate::Disabled));
  }

  #[test]
  fn minimum_dependency_age_preserves_explicit_disable() {
    let mut config = MinimumDependencyAgeConfig {
      age: Some(NewestDependencyDate::Disabled),
      exclude: Vec::new(),
    };
    let env_age =
      NewestDependencyDate::Enabled(now() - chrono::Duration::days(2));
    apply_minimum_dependency_age_fallbacks(&mut config, Some(env_age), now());
    assert_eq!(config.age, Some(NewestDependencyDate::Disabled));
  }
}
