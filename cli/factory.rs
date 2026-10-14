// Copyright 2018-2026 the Deno authors. MIT license.

use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno_error::JsErrorBox;
use deno_graph::packages::JsrVersionResolver;
use ende_cache_dir::GlobalOrLocalHttpCache;
use ende_config::workspace::WorkspaceDirectory;
use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_core::futures::FutureExt;
use ende_core::serde_json;
use ende_core::url::Url;
use ende_lib::args::CaData;
use ende_lib::args::get_root_cert_store;
use ende_lib::worker::LibMainWorkerFactory;
use ende_lib::worker::LibMainWorkerOptions;
use ende_lib::worker::LibWorkerFactoryRoots;
use ende_resolver::cache::ParsedSourceCache;
use ende_resolver::ende_json::CompilerOptionsOverrides;
use ende_resolver::ende_json::CompilerOptionsResolver;
use ende_resolver::factory::ConfigDiscoveryOption;
use ende_resolver::factory::ResolverFactoryOptions;
use ende_resolver::factory::SpecifiedImportMapProvider;
use ende_resolver::import_map::WorkspaceExternalImportMapLoader;
use ende_resolver::loader::MemoryFiles;
use ende_resolver::workspace::WorkspaceResolver;
use ende_runtime::CpuProfilerConfig;
use ende_runtime::FeatureChecker;
use ende_runtime::ende_fs;
use ende_runtime::ende_fs::RealFs;
use ende_runtime::ende_permissions::Permissions;
use ende_runtime::ende_permissions::PermissionsContainer;
use ende_runtime::ende_tls::RootCertStoreProvider;
use ende_runtime::ende_tls::rustls::RootCertStore;
use ende_runtime::ende_web::BlobStore;
use ende_runtime::ende_web::BlobStoreTrait;
use ende_runtime::permissions::RuntimePermissionDescriptorParser;
use once_cell::sync::OnceCell;

use crate::args::CliLockfile;
use crate::args::CliOptions;
use crate::args::ConfigFlag;
use crate::args::EndeSubcommand;
use crate::args::Flags;
use crate::args::FlagsExt;
use crate::args::InstallFlags;
use crate::cache::Caches;
use crate::cache::CodeCache;
use crate::cache::EndeDir;
use crate::cache::GlobalHttpCache;
use crate::cache::ModuleInfoCache;
use crate::file_fetcher::CliFileFetcher;
use crate::file_fetcher::CreateCliFileFetcherOptions;
use crate::file_fetcher::TextDecodedFile;
use crate::file_fetcher::create_cli_file_fetcher;
use crate::graph_container::MainModuleGraphContainer;
use crate::graph_util::FileWatcherReporter;
use crate::graph_util::ModuleGraphBuilder;
use crate::graph_util::ModuleGraphCreator;
use crate::http_util::HttpClientProvider;
use crate::module_loader::CliEmitter;
use crate::module_loader::CliModuleLoaderFactory;
use crate::module_loader::EszipModuleLoader;
use crate::module_loader::ModuleLoadPreparer;
use crate::resolver::CliResolver;
use crate::resolver::on_resolve_diagnostic;
use crate::standalone::binary::EndeCompileBinaryWriter;
use crate::sys::CliSys;
use crate::tools::installer::BinNameResolver;
use crate::tools::lint::LintRuleProvider;
use crate::tools::run::hmr::HmrRunnerState;
use crate::type_checker::TypeChecker;
use crate::util::file_watcher::WatcherCommunicator;
use crate::util::progress_bar::ProgressBar;
use crate::util::progress_bar::ProgressBarStyle;
use crate::worker::CliMainWorkerFactory;
use crate::worker::CliMainWorkerOptions;

struct CliRootCertStoreProvider {
  cell: OnceCell<RootCertStore>,
  maybe_root_path: Option<PathBuf>,
  maybe_ca_stores: Option<Vec<String>>,
  maybe_ca_data: Option<CaData>,
}

impl CliRootCertStoreProvider {
  pub fn new(
    maybe_root_path: Option<PathBuf>,
    maybe_ca_stores: Option<Vec<String>>,
    maybe_ca_data: Option<CaData>,
  ) -> Self {
    Self {
      cell: Default::default(),
      maybe_root_path,
      maybe_ca_stores,
      maybe_ca_data,
    }
  }
}

impl RootCertStoreProvider for CliRootCertStoreProvider {
  fn get_or_try_init(&self) -> Result<&RootCertStore, JsErrorBox> {
    self
      .cell
      .get_or_try_init(|| {
        get_root_cert_store(
          &CliSys::default(),
          self.maybe_root_path.clone(),
          self.maybe_ca_stores.clone(),
          self.maybe_ca_data.clone(),
        )
      })
      .map_err(JsErrorBox::from_err)
  }
}

#[derive(Debug)]
struct EszipModuleLoaderProvider {
  cli_options: Arc<CliOptions>,
  deferred: once_cell::sync::OnceCell<Arc<EszipModuleLoader>>,
}

impl EszipModuleLoaderProvider {
  pub async fn get(&self) -> Result<Option<&Arc<EszipModuleLoader>>, AnyError> {
    if self.cli_options.eszip()
      && let EndeSubcommand::Run(run_flags) = self.cli_options.sub_command()
    {
      if self.deferred.get().is_none() {
        let eszip_loader = EszipModuleLoader::create(
          &run_flags.script,
          self.cli_options.initial_cwd(),
        )
        .await?;
        _ = self.deferred.set(Arc::new(eszip_loader));
      }
      return Ok(Some(self.deferred.get().unwrap()));
    }
    Ok(None)
  }
}

#[derive(Debug)]
struct CliSpecifiedImportMapProvider {
  cli_options: Arc<CliOptions>,
  file_fetcher: Arc<CliFileFetcher>,
  eszip_module_loader_provider: Arc<EszipModuleLoaderProvider>,
  workspace_external_import_map_loader:
    Arc<WorkspaceExternalImportMapLoader<CliSys>>,
}

#[async_trait::async_trait(?Send)]
impl SpecifiedImportMapProvider for CliSpecifiedImportMapProvider {
  async fn get(
    &self,
  ) -> Result<Option<ende_resolver::workspace::SpecifiedImportMap>, AnyError>
  {
    async fn resolve_import_map_value_from_specifier(
      specifier: &Url,
      file_fetcher: &CliFileFetcher,
    ) -> Result<serde_json::Value, AnyError> {
      if specifier.scheme() == "data" {
        let data_url_text =
          deno_media_type::data_url::RawDataUrl::parse(specifier)?.decode()?;
        Ok(serde_json::from_str(&data_url_text)?)
      } else {
        let file = TextDecodedFile::decode(
          file_fetcher.fetch_bypass_permissions(specifier).await?,
        )?;
        Ok(serde_json::from_str(&file.source)?)
      }
    }

    let maybe_import_map_specifier =
      self.cli_options.resolve_specified_import_map_specifier()?;
    match maybe_import_map_specifier {
      Some(specifier) => {
        let value = match self.eszip_module_loader_provider.get().await? {
          Some(eszip) => eszip.load_import_map_value(&specifier)?,
          None => resolve_import_map_value_from_specifier(
            &specifier,
            &self.file_fetcher,
          )
          .await
          .with_context(|| {
            format!("Unable to load '{}' import map", specifier)
          })?,
        };
        Ok(Some(ende_resolver::workspace::SpecifiedImportMap {
          base_url: specifier,
          value,
        }))
      }
      None => {
        if let Some(import_map) =
          self.workspace_external_import_map_loader.get_or_load()?
        {
          let path_url = deno_path_util::url_from_file_path(&import_map.path)?;
          Ok(Some(ende_resolver::workspace::SpecifiedImportMap {
            base_url: path_url,
            value: import_map.value.clone(),
          }))
        } else {
          Ok(None)
        }
      }
    }
  }
}

pub type CliWorkspaceFactory = ende_resolver::factory::WorkspaceFactory<CliSys>;
pub type CliResolverFactory = ende_resolver::factory::ResolverFactory<CliSys>;

pub struct Deferred<T>(once_cell::unsync::OnceCell<T>);

impl<T> Default for Deferred<T> {
  fn default() -> Self {
    Self(once_cell::unsync::OnceCell::default())
  }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Deferred<T> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_tuple("Deferred").field(&self.0).finish()
  }
}

impl<T> Deferred<T> {
  #[inline(always)]
  pub fn get_or_try_init(
    &self,
    create: impl FnOnce() -> Result<T, AnyError>,
  ) -> Result<&T, AnyError> {
    self.0.get_or_try_init(create)
  }

  #[inline(always)]
  pub fn get_or_init(&self, create: impl FnOnce() -> T) -> &T {
    self.0.get_or_init(create)
  }

  pub async fn get_or_try_init_async(
    &self,
    // some futures passed here are boxed because it was discovered
    // that they were called a lot, causing other futures to get
    // really big causing stack overflows on Windows
    create: impl Future<Output = Result<T, AnyError>>,
  ) -> Result<&T, AnyError> {
    if self.0.get().is_none() {
      // todo(dsherret): it would be more ideal if this enforced a
      // single executor and then we could make some initialization
      // concurrent
      let val = create.await?;
      _ = self.0.set(val);
    }
    Ok(self.0.get().unwrap())
  }
}

#[derive(Default)]
struct CliFactoryServices {
  blob_store: Deferred<Arc<dyn BlobStoreTrait>>,
  caches: Deferred<Arc<Caches>>,
  cli_options: Deferred<Arc<CliOptions>>,
  code_cache: Deferred<Arc<CodeCache>>,
  eszip_module_loader_provider: Deferred<Arc<EszipModuleLoaderProvider>>,
  feature_checker: Deferred<Arc<FeatureChecker>>,
  file_fetcher: Deferred<Arc<CliFileFetcher>>,
  fs: Deferred<Arc<dyn ende_fs::FileSystem>>,
  http_client_provider: Deferred<Arc<HttpClientProvider>>,
  import_deny_permissions: Deferred<Option<PermissionsContainer>>,
  import_http_client_provider: Deferred<Arc<HttpClientProvider>>,
  main_graph_container: Deferred<Arc<MainModuleGraphContainer>>,
  graph_reporter: Deferred<Option<Arc<dyn deno_graph::source::Reporter>>>,
  memory_files: Arc<MemoryFiles>,
  module_graph_builder: Deferred<Arc<ModuleGraphBuilder>>,
  module_graph_creator: Deferred<Arc<ModuleGraphCreator>>,
  module_info_cache: Deferred<Arc<ModuleInfoCache>>,
  module_load_preparer: Deferred<Arc<ModuleLoadPreparer>>,
  permission_desc_parser:
    Deferred<Arc<RuntimePermissionDescriptorParser<CliSys>>>,
  resolver_factory: Deferred<Arc<CliResolverFactory>>,
  root_cert_store_provider: Deferred<Arc<dyn RootCertStoreProvider>>,
  root_permissions_container: Deferred<PermissionsContainer>,
  text_only_progress_bar: Deferred<ProgressBar>,
  type_checker: Deferred<Arc<TypeChecker>>,
  workspace_factory: Deferred<Arc<CliWorkspaceFactory>>,
  install_reporter:
    Deferred<Option<Arc<crate::tools::installer::InstallReporter>>>,
}

#[derive(Debug, Default)]
struct CliFactoryOverrides {
  initial_cwd: Option<PathBuf>,
  workspace_directory: Option<Arc<WorkspaceDirectory>>,
}

pub struct CliFactory {
  watcher_communicator: Option<Arc<WatcherCommunicator>>,
  flags: Arc<Flags>,
  services: CliFactoryServices,
  overrides: CliFactoryOverrides,
}

impl CliFactory {
  pub fn from_flags(flags: Arc<Flags>) -> Self {
    Self {
      flags,
      watcher_communicator: None,
      services: Default::default(),
      overrides: Default::default(),
    }
  }

  pub fn from_flags_for_watcher(
    flags: Arc<Flags>,
    watcher_communicator: Arc<WatcherCommunicator>,
  ) -> Self {
    CliFactory {
      watcher_communicator: Some(watcher_communicator),
      flags,
      services: Default::default(),
      overrides: Default::default(),
    }
  }

  pub fn set_initial_cwd(&mut self, initial_cwd: PathBuf) {
    self.overrides.initial_cwd = Some(initial_cwd);
  }

  pub fn set_workspace_dir(&mut self, dir: Arc<WorkspaceDirectory>) {
    self.overrides.workspace_directory = Some(dir);
  }

  pub async fn maybe_lockfile(
    &self,
  ) -> Result<Option<&Arc<CliLockfile>>, AnyError> {
    self.workspace_factory()?.maybe_lockfile().await
  }

  pub fn cli_options(&self) -> Result<&Arc<CliOptions>, AnyError> {
    self.services.cli_options.get_or_try_init(|| {
      let workspace_factory = self.workspace_factory()?;
      let workspace_directory = workspace_factory.workspace_directory()?;
      CliOptions::from_flags(
        self.flags.clone(),
        workspace_factory.initial_cwd().clone(),
        workspace_directory.clone(),
      )
      .map(Arc::new)
    })
  }

  pub fn ende_dir(&self) -> Result<&EndeDir, AnyError> {
    Ok(
      self
        .workspace_factory()?
        .ende_dir_provider()
        .get_or_create()?,
    )
  }

  pub fn caches(&self) -> Result<&Arc<Caches>, AnyError> {
    self.services.caches.get_or_try_init(|| {
      let cli_options = self.cli_options()?;
      let caches = Arc::new(Caches::new(
        self.workspace_factory()?.ende_dir_provider().clone(),
      ));
      // Warm up the caches we know we'll likely need based on the CLI mode
      match cli_options.sub_command() {
        EndeSubcommand::Run(_)
        | EndeSubcommand::Serve(_)
        | EndeSubcommand::Bench(_)
        | EndeSubcommand::Test(_)
        | EndeSubcommand::Check(_) => {
          _ = caches.dep_analysis_db();
          if cli_options.type_check_mode().is_true() {
            _ = caches.fast_check_db();
            _ = caches.type_checking_cache_db();
          }
          if cli_options.code_cache_enabled() {
            _ = caches.code_cache_db();
          }
        }
        _ => {}
      }
      Ok(caches)
    })
  }

  pub fn blob_store(&self) -> &Arc<dyn BlobStoreTrait> {
    self
      .services
      .blob_store
      .get_or_init(|| BlobStore::default_arc())
  }

  pub fn bin_name_resolver(&self) -> Result<BinNameResolver<'_>, AnyError> {
    Ok(BinNameResolver::new(self.http_client_provider()))
  }

  pub fn root_cert_store_provider(&self) -> &Arc<dyn RootCertStoreProvider> {
    self.services.root_cert_store_provider.get_or_init(|| {
      Arc::new(CliRootCertStoreProvider::new(
        None,
        self.flags.ca_stores.clone(),
        self.flags.ca_data.clone(),
      ))
    })
  }

  pub fn text_only_progress_bar(&self) -> &ProgressBar {
    self
      .services
      .text_only_progress_bar
      .get_or_init(|| ProgressBar::new(ProgressBarStyle::TextOnly))
  }

  pub fn global_http_cache(&self) -> Result<&Arc<GlobalHttpCache>, AnyError> {
    Ok(self.workspace_factory()?.global_http_cache()?)
  }

  pub fn http_cache(
    &self,
  ) -> Result<&GlobalOrLocalHttpCache<CliSys>, AnyError> {
    Ok(self.workspace_factory()?.http_cache()?)
  }

  pub fn http_client_provider(&self) -> &Arc<HttpClientProvider> {
    self.services.http_client_provider.get_or_init(|| {
      Arc::new(HttpClientProvider::new(
        Some(self.root_cert_store_provider().clone()),
        self.flags.unsafely_ignore_certificate_errors.clone(),
      ))
    })
  }

  fn eszip_module_loader_provider(
    &self,
  ) -> Result<&Arc<EszipModuleLoaderProvider>, AnyError> {
    self
      .services
      .eszip_module_loader_provider
      .get_or_try_init(|| {
        Ok(Arc::new(EszipModuleLoaderProvider {
          cli_options: self.cli_options()?.clone(),
          deferred: Default::default(),
        }))
      })
  }

  /// The `--deny-import` rules on their own, as a permissions container.
  ///
  /// Module fetches are checked against these so that a deny rule written as
  /// an IP literal also covers hostnames that resolve to it. Scoped to the
  /// module fetcher: npm and jsr package resolution is not an `import` and is
  /// not governed by `--deny-import`.
  ///
  /// Deliberately not `root_permissions_container`: building that resolves the
  /// main module, which needs the file fetcher this feeds.
  fn import_deny_permissions(
    &self,
  ) -> Result<&Option<PermissionsContainer>, AnyError> {
    self.services.import_deny_permissions.get_or_try_init(|| {
      let options = self.cli_options()?.import_deny_permissions_options()?;
      if options.deny_import.as_ref().is_none_or(|d| d.is_empty()) {
        // nothing to check against, so leave the plain client and its
        // connection pool alone
        return Ok(None);
      }
      let desc_parser = self.permission_desc_parser()?.clone();
      let permissions =
        Permissions::from_options(desc_parser.as_ref(), &options)?;
      Ok(Some(PermissionsContainer::new(desc_parser, permissions)))
    })
  }

  /// The HTTP client used to load code.
  ///
  /// Identical to [`Self::http_client_provider`] except that its connections
  /// re-check the address they resolved to against `--deny-import`: the check
  /// on the specifier itself happens before DNS resolution, so it cannot tell
  /// that a hostname points at a denied address.
  fn import_http_client_provider(
    &self,
  ) -> Result<&Arc<HttpClientProvider>, AnyError> {
    self
      .services
      .import_http_client_provider
      .get_or_try_init(|| {
        let Some(permissions) = self.import_deny_permissions()?.clone() else {
          return Ok(self.http_client_provider().clone());
        };
        Ok(Arc::new(
          HttpClientProvider::new(
            Some(self.root_cert_store_provider().clone()),
            self.flags.unsafely_ignore_certificate_errors.clone(),
          )
          .with_import_permissions(permissions),
        ))
      })
  }

  pub fn file_fetcher(&self) -> Result<&Arc<CliFileFetcher>, AnyError> {
    self.services.file_fetcher.get_or_try_init(|| {
      let http_client_provider = self.import_http_client_provider()?.clone();
      let cli_options = self.cli_options()?;
      Ok(Arc::new(create_cli_file_fetcher(
        self.blob_store().clone(),
        self.http_cache()?.clone(),
        http_client_provider,
        self.services.memory_files.clone(),
        self.sys(),
        CreateCliFileFetcherOptions {
          allow_remote: !cli_options.no_remote(),
          cache_setting: cli_options.cache_setting(),
          download_log_level: log::Level::Info,
          progress_bar: Some(self.text_only_progress_bar().clone()),
        },
      )))
    })
  }

  pub fn fs(&self) -> &Arc<dyn ende_fs::FileSystem> {
    self.services.fs.get_or_init(|| Arc::new(RealFs))
  }

  pub fn memory_files(&self) -> &Arc<MemoryFiles> {
    &self.services.memory_files
  }

  pub fn sys(&self) -> CliSys {
    CliSys::default() // very cheap to make
  }

  pub fn jsr_version_resolver(
    &self,
  ) -> Result<&Arc<JsrVersionResolver>, AnyError> {
    self.resolver_factory()?.jsr_version_resolver()
  }

  pub fn install_reporter(
    &self,
  ) -> Result<Option<&Arc<crate::tools::installer::InstallReporter>>, AnyError>
  {
    self
      .services
      .install_reporter
      .get_or_try_init(|| match self.cli_options()?.sub_command() {
        EndeSubcommand::Install(InstallFlags::Local(_))
        | EndeSubcommand::Add(_)
        | EndeSubcommand::Cache(_)
        | EndeSubcommand::Ci(_) => Ok(Some(Arc::new(
          crate::tools::installer::InstallReporter::new(),
        ))),
        _ => Ok(None),
      })
      .map(|opt| opt.as_ref())
  }

  fn workspace_factory(&self) -> Result<&Arc<CliWorkspaceFactory>, AnyError> {
    self.services.workspace_factory.get_or_try_init(|| {
      let initial_cwd = match self.overrides.initial_cwd.clone() {
        Some(v) => v,
        // For modes that don't depend on a real cwd (REPL, eval), fall back
        // to a sentinel path when current_dir() fails — matches Node.js
        // semantics where `node --interactive` works even after the cwd has
        // been unlinked.
        None
          if matches!(
            self.flags.subcommand,
            EndeSubcommand::Repl(_) | EndeSubcommand::Eval(_)
          ) =>
        {
          crate::util::env::resolve_cwd_or_fallback(
            self.flags.initial_cwd.as_deref(),
          )
        }
        None => {
          crate::util::env::resolve_cwd(self.flags.initial_cwd.as_deref())?
            .into_owned()
        }
      };
      let options = new_workspace_factory_options(&initial_cwd, &self.flags);
      let mut factory =
        CliWorkspaceFactory::new(self.sys(), initial_cwd, options);
      if let Some(workspace_dir) = &self.overrides.workspace_directory {
        factory.set_workspace_directory(workspace_dir.clone());
      }
      Ok(Arc::new(factory))
    })
  }

  pub async fn workspace_resolver(
    &self,
  ) -> Result<&Arc<WorkspaceResolver<CliSys>>, AnyError> {
    self.resolver_factory()?.workspace_resolver().await
  }

  pub async fn resolver(&self) -> Result<&Arc<CliResolver>, AnyError> {
    self.resolver_factory()?.ende_resolver().await
  }

  pub fn graph_reporter(
    &self,
  ) -> Result<&Option<Arc<dyn deno_graph::source::Reporter>>, AnyError> {
    match self.cli_options()?.sub_command() {
      EndeSubcommand::Install(_) | EndeSubcommand::Ci(_) => {
        self.services.graph_reporter.get_or_try_init(|| {
          self.install_reporter().map(|opt| {
            opt.map(|r| r.clone() as Arc<dyn deno_graph::source::Reporter>)
          })
        })
      }
      _ => Ok(self.services.graph_reporter.get_or_init(|| {
        self
          .watcher_communicator
          .as_ref()
          .map(|i| FileWatcherReporter::new(i.clone()))
          .map(|i| Arc::new(i) as Arc<dyn deno_graph::source::Reporter>)
      })),
    }
  }

  pub fn module_info_cache(&self) -> Result<&Arc<ModuleInfoCache>, AnyError> {
    self.services.module_info_cache.get_or_try_init(|| {
      Ok(Arc::new(ModuleInfoCache::new(
        self.caches()?.dep_analysis_db(),
        self.resolver_factory()?.parsed_source_cache().clone(),
      )))
    })
  }

  pub fn code_cache(&self) -> Result<&Arc<CodeCache>, AnyError> {
    self.services.code_cache.get_or_try_init(|| {
      Ok(Arc::new(CodeCache::new(self.caches()?.code_cache_db())))
    })
  }

  pub fn parsed_source_cache(
    &self,
  ) -> Result<&Arc<ParsedSourceCache>, AnyError> {
    Ok(self.resolver_factory()?.parsed_source_cache())
  }

  pub fn emitter(&self) -> Result<&Arc<CliEmitter>, AnyError> {
    self.resolver_factory()?.emitter()
  }

  pub async fn lint_rule_provider(&self) -> Result<LintRuleProvider, AnyError> {
    Ok(LintRuleProvider::new(Some(
      self.workspace_resolver().await?.clone(),
    )))
  }

  pub fn compiler_options_resolver(
    &self,
  ) -> Result<&Arc<CompilerOptionsResolver>, AnyError> {
    self.resolver_factory()?.compiler_options_resolver()
  }

  pub async fn type_checker(&self) -> Result<&Arc<TypeChecker>, AnyError> {
    self
      .services
      .type_checker
      .get_or_try_init_async(
        async {
          let cli_options = self.cli_options()?;
          Ok(Arc::new(TypeChecker::new(
            self.caches()?.clone(),
            cli_options.clone(),
            self.module_graph_builder().await?.clone(),
            self.sys(),
            self.compiler_options_resolver()?.clone(),
            if cli_options.code_cache_enabled() {
              Some(self.code_cache()?.clone())
            } else {
              None
            },
          )))
        }
        .boxed_local(),
      )
      .await
  }

  pub async fn module_graph_builder(
    &self,
  ) -> Result<&Arc<ModuleGraphBuilder>, AnyError> {
    self
      .services
      .module_graph_builder
      .get_or_try_init_async(
        async {
          let cli_options = self.cli_options()?;
          Ok(Arc::new(ModuleGraphBuilder::new(
            self.caches()?.clone(),
            cli_options.clone(),
            self.compiler_options_resolver()?.clone(),
            self.file_fetcher()?.clone(),
            self.global_http_cache()?.clone(),
            self.jsr_version_resolver()?.clone(),
            self.maybe_lockfile().await?.cloned(),
            self.graph_reporter()?.clone(),
            self.module_info_cache()?.clone(),
            self.resolver_factory()?.parsed_source_cache().clone(),
            self.text_only_progress_bar().clone(),
            self.resolver().await?.clone(),
            self.root_permissions_container()?.clone(),
            self.sys(),
            self.install_reporter()?.cloned().map(|r| {
              r as Arc<dyn ende_resolver::file_fetcher::GraphLoaderReporter>
            }),
          )))
        }
        .boxed_local(),
      )
      .await
  }

  pub async fn module_graph_creator(
    &self,
  ) -> Result<&Arc<ModuleGraphCreator>, AnyError> {
    self
      .services
      .module_graph_creator
      .get_or_try_init_async(
        async {
          let cli_options = self.cli_options()?;
          Ok(Arc::new(ModuleGraphCreator::new(
            cli_options.clone(),
            self.module_graph_builder().await?.clone(),
            self.type_checker().await?.clone(),
          )))
        }
        .boxed_local(),
      )
      .await
  }

  pub async fn main_module_graph_container(
    &self,
  ) -> Result<&Arc<MainModuleGraphContainer>, AnyError> {
    self
      .services
      .main_graph_container
      .get_or_try_init_async(
        async {
          Ok(Arc::new(MainModuleGraphContainer::new(
            self.cli_options()?.clone(),
            self.module_load_preparer().await?.clone(),
            self.root_permissions_container()?.clone(),
          )))
        }
        .boxed_local(),
      )
      .await
  }

  pub fn maybe_start_inspector_server(&self) -> Result<(), AnyError> {
    let cli_options = self.cli_options()?;
    if let Some((host, name, publish_uid)) =
      cli_options.resolve_inspector_server_options()
    {
      ende_runtime::ende_inspector_server::create_inspector_server(
        host,
        name,
        publish_uid,
      )?;
    }
    Ok(())
  }

  pub async fn module_load_preparer(
    &self,
  ) -> Result<&Arc<ModuleLoadPreparer>, AnyError> {
    self
      .services
      .module_load_preparer
      .get_or_try_init_async(
        async {
          let cli_options = self.cli_options()?;
          Ok(Arc::new(ModuleLoadPreparer::new(
            cli_options.clone(),
            self.maybe_lockfile().await?.cloned(),
            self.module_graph_builder().await?.clone(),
            self.text_only_progress_bar().clone(),
            self.type_checker().await?.clone(),
          )))
        }
        .boxed_local(),
      )
      .await
  }

  pub fn permission_desc_parser(
    &self,
  ) -> Result<&Arc<RuntimePermissionDescriptorParser<CliSys>>, AnyError> {
    self.services.permission_desc_parser.get_or_try_init(|| {
      Ok(Arc::new(RuntimePermissionDescriptorParser::new(self.sys())))
    })
  }

  pub fn feature_checker(&self) -> Result<&Arc<FeatureChecker>, AnyError> {
    self.services.feature_checker.get_or_try_init(|| {
      let cli_options = self.cli_options()?;
      let mut checker = FeatureChecker::default();
      checker.set_exit_cb(Box::new(crate::unstable_exit_cb));
      let unstable_features = cli_options.unstable_features();
      for feature in ende_runtime::UNSTABLE_FEATURES {
        if unstable_features.contains(&feature.name) {
          checker.enable_feature(feature.name);
        }
      }

      // Deno Deploy: when ENDE_KV_REQUIRES_DISTRIBUTED_DATABASE=error, force the
      // unstable KV feature on so `Deno.openKv()` is always exposed. The open
      // path then rejects with a clear "no database attached" error instead of
      // the runtime hiding the API and reporting "Deno.openKv is not a function".
      if ende_env::var("ENDE_KV_REQUIRES_DISTRIBUTED_DATABASE").as_deref()
        == Ok("error")
        && !checker.check("kv")
      {
        checker.enable_feature("kv");
      }

      Ok(Arc::new(checker))
    })
  }

  pub async fn create_compile_binary_writer(
    &self,
    is_desktop: bool,
  ) -> Result<EndeCompileBinaryWriter<'_>, AnyError> {
    Ok(EndeCompileBinaryWriter::new(
      self.cli_options()?,
      self.ende_dir()?,
      self.emitter()?,
      self.file_fetcher()?,
      self.http_client_provider(),
      self.workspace_resolver().await?.as_ref(),
      is_desktop,
    ))
  }

  pub fn root_permissions_container(
    &self,
  ) -> Result<&PermissionsContainer, AnyError> {
    self
      .services
      .root_permissions_container
      .get_or_try_init(|| {
        let desc_parser = self.permission_desc_parser()?.clone();
        let permissions = Permissions::from_options(
          desc_parser.as_ref(),
          &self.cli_options()?.permissions_options()?,
        )?;

        Ok(PermissionsContainer::new(desc_parser, permissions))
      })
  }

  fn workspace_external_import_map_loader(
    &self,
  ) -> Result<&Arc<WorkspaceExternalImportMapLoader<CliSys>>, AnyError> {
    Ok(
      self
        .workspace_factory()?
        .workspace_external_import_map_loader()?,
    )
  }

  pub async fn create_cli_main_worker_factory(
    &self,
  ) -> Result<CliMainWorkerFactory, AnyError> {
    self
      .create_cli_main_worker_factory_with_roots(Default::default())
      .await
  }

  pub async fn create_module_loader_factory(
    &self,
  ) -> Result<CliModuleLoaderFactory, AnyError> {
    let cli_options = self.cli_options()?;
    let resolver_factory = self.resolver_factory()?;

    let maybe_eszip_loader =
      self.eszip_module_loader_provider()?.get().await?.cloned();
    let module_loader_factory = CliModuleLoaderFactory::new(
      cli_options,
      if cli_options.code_cache_enabled() {
        Some(self.code_cache()?.clone())
      } else {
        None
      },
      self.file_fetcher()?.clone(),
      self.main_module_graph_container().await?.clone(),
      self.module_load_preparer().await?.clone(),
      resolver_factory.parsed_source_cache().clone(),
      resolver_factory.module_loader()?.clone(),
      self.resolver().await?.clone(),
      self.sys(),
      maybe_eszip_loader,
      self.watcher_communicator.clone(),
    );

    Ok(module_loader_factory)
  }

  pub async fn create_cli_main_worker_factory_with_roots(
    &self,
    roots: LibWorkerFactoryRoots,
  ) -> Result<CliMainWorkerFactory, AnyError> {
    let cli_options = self.cli_options()?;
    let fs = self.fs();
    // Provide the watcher communicator whenever running under the file watcher,
    // not just for HMR. `CliMainWorker::run()` uses its presence to detect that
    // it is under a watcher and route `Ende.exit()` through isolate termination
    // instead of `std::process::exit()`, which keeps `ende serve --watch` (which
    // goes through `run()` rather than `run_for_watcher`) alive across exits.
    // HMR-specific behaviour is gated separately on `create_hmr_runner`. See
    // issue #7590.
    let maybe_file_watcher_communicator = self.watcher_communicator.clone();
    let module_loader_factory = self.create_module_loader_factory().await?;
    self.maybe_start_inspector_server()?;

    let maybe_cpu_prof_config_for_workers =
      cli_options.cpu_prof_dir().map(|dir| CpuProfilerConfig {
        dir,
        name: cli_options.cpu_prof_name(),
        interval: cli_options.cpu_prof_interval(),
        md: cli_options.cpu_prof_md(),
        flamegraph: cli_options.cpu_prof_flamegraph(),
      });

    let lib_main_worker_factory = LibMainWorkerFactory::new(
      self.blob_store().clone(),
      if cli_options.code_cache_enabled() {
        Some(self.code_cache()?.clone())
      } else {
        None
      },
      None, // EndeRtNativeAddonLoader
      self.feature_checker()?.clone(),
      fs.clone(),
      cli_options.coverage_dir(),
      maybe_cpu_prof_config_for_workers,
      Box::new(module_loader_factory),
      self.root_cert_store_provider().clone(),
      cli_options.resolve_storage_key_resolver(),
      self.sys(),
      self.create_lib_main_worker_options()?,
      roots,
    );

    Ok(CliMainWorkerFactory::new(
      lib_main_worker_factory,
      maybe_file_watcher_communicator,
      self.create_cli_main_worker_options()?,
      self.root_permissions_container()?.clone(),
    ))
  }

  pub fn create_lib_main_worker_options(
    &self,
  ) -> Result<LibMainWorkerOptions, AnyError> {
    let cli_options = self.cli_options()?;
    Ok(LibMainWorkerOptions {
      argv: cli_options.argv().clone(),
      // This optimization is only available for "run" subcommand
      // because we need to register new ops for testing and jupyter
      // integration.
      skip_op_registration: cli_options.sub_command().is_run(),
      log_level: cli_options.log_level().unwrap_or(log::Level::Info).into(),
      enable_testing_features: cli_options.enable_testing_features(),
      inspect_brk: cli_options.inspect_brk(),
      inspect_wait: cli_options.inspect_wait(),
      trace_ops: cli_options.trace_ops().clone(),
      is_standalone: false,
      auto_serve: ende_env::var("ENDE_AUTO_SERVE").is_ok(),
      is_inspecting: cli_options.is_inspecting(),
      location: cli_options.location_flag().clone(),
      origin_data_folder_path: Some(self.ende_dir()?.origin_data_folder_path()),
      seed: cli_options.seed(),
      unsafely_ignore_certificate_errors: cli_options
        .unsafely_ignore_certificate_errors()
        .clone(),
      serve_port: cli_options.serve_port(),
      serve_host: cli_options.serve_host(),
      otel_config: cli_options.otel_config(),
      no_legacy_abort: cli_options.no_legacy_abort(),
      startup_snapshot: ende_snapshots::CLI_SNAPSHOT,
      residual_lazy_js_sources: ende_snapshots::RESIDUAL_LAZY_JS,
      residual_lazy_esm_sources: ende_snapshots::RESIDUAL_LAZY_ESM,
      enable_raw_imports: cli_options.unstable_raw_imports(),
      close_on_idle: true,
      maybe_initial_cwd: Some(deno_path_util::url_from_directory_path(
        cli_options.initial_cwd(),
      )?),
      // Deno 2.8 exposes an `OffscreenCanvas` global whose 2D context is
      // unimplemented (`getContext("2d")` returns null). Libraries that
      // feature-detect on the global's presence then crash. Setting this
      // removes the global, restoring the pre-2.8 fallback path. Truthy
      // values are `1` and `true`.
      disable_offscreen_canvas: matches!(
        ende_env::var("ENDE_DISABLE_OFFSCREEN_CANVAS").as_deref(),
        Ok("1") | Ok("true")
      ),
    })
  }

  fn create_cli_main_worker_options(
    &self,
  ) -> Result<CliMainWorkerOptions, AnyError> {
    let cli_options = self.cli_options()?;
    let create_hmr_runner = if cli_options.has_hmr() {
      let watcher_communicator = self.watcher_communicator.clone().unwrap();
      let emitter = self.emitter()?.clone();
      let fn_: crate::worker::CreateHmrRunnerCb = Box::new(move || {
        HmrRunnerState::new(emitter.clone(), watcher_communicator.clone())
      });
      Some(fn_)
    } else {
      None
    };
    let maybe_coverage_dir = cli_options.coverage_dir();
    let maybe_cpu_prof_config =
      cli_options.cpu_prof_dir().map(|dir| CpuProfilerConfig {
        dir,
        name: cli_options.cpu_prof_name(),
        interval: cli_options.cpu_prof_interval(),
        md: cli_options.cpu_prof_md(),
        flamegraph: cli_options.cpu_prof_flamegraph(),
      });

    let initial_cwd =
      deno_path_util::url_from_directory_path(cli_options.initial_cwd())?;

    Ok(CliMainWorkerOptions {
      needs_test_modules: cli_options.sub_command().needs_test(),
      create_hmr_runner,
      maybe_coverage_dir,
      maybe_cpu_prof_config,
      initial_cwd: Arc::new(initial_cwd),
    })
  }

  pub fn resolver_factory(&self) -> Result<&Arc<CliResolverFactory>, AnyError> {
    self.services.resolver_factory.get_or_try_init(|| {
      let options = self.cli_options()?;
      Ok(Arc::new(CliResolverFactory::new(
        self.workspace_factory()?.clone(),
        ResolverFactoryOptions {
          compiler_options_overrides: CompilerOptionsOverrides {
            no_transpile: false,
            force_check_js: matches!(
              &self.flags.subcommand,
              EndeSubcommand::Check(check_flags) if check_flags.check_js
            ),
            source_map_base: None,
            preserve_jsx: false,
            // Untyped execution environments (e.g. isolates running with
            // --no-check) can opt out of verbatimModuleSyntax, which
            // otherwise leaves dangling type-only re-exports that crash at
            // runtime. Truthy values are `1` and `true`.
            force_disable_verbatim_module_syntax: matches!(
              ende_env::var("ENDE_DISABLE_VERBATIM_MODULE_SYNTAX").as_deref(),
              Ok("1") | Ok("true")
            ),
          },
          newest_dependency_date: self.flags.minimum_dependency_age,
          specified_import_map: Some(Box::new(CliSpecifiedImportMapProvider {
            cli_options: options.clone(),
            eszip_module_loader_provider: self
              .eszip_module_loader_provider()?
              .clone(),
            file_fetcher: self.file_fetcher()?.clone(),
            workspace_external_import_map_loader: self
              .workspace_external_import_map_loader()?
              .clone(),
          })),
          unstable_sloppy_imports: options.unstable_sloppy_imports(),
          on_mapped_resolution_diagnostic: Some(Arc::new(
            on_resolve_diagnostic,
          )),
          allow_json_imports:
            ende_resolver::loader::AllowJsonImports::WithAttribute,
        },
      )))
    })
  }
}

fn new_workspace_factory_options(
  initial_cwd: &Path,
  flags: &Flags,
) -> ende_resolver::factory::WorkspaceFactoryOptions {
  ende_resolver::factory::WorkspaceFactoryOptions {
    additional_config_file_names: if matches!(
      flags.subcommand,
      EndeSubcommand::Publish(..)
    ) {
      &["jsr.json", "jsr.jsonc"]
    } else {
      &[]
    },
    config_discovery: match &flags.config_flag {
      ConfigFlag::Discover => {
        if let Some(start_paths) = flags.config_path_args(initial_cwd) {
          ConfigDiscoveryOption::Discover { start_paths }
        } else {
          ConfigDiscoveryOption::Disabled
        }
      }
      ConfigFlag::Path(path) => {
        ConfigDiscoveryOption::Path(initial_cwd.join(path))
      }
      ConfigFlag::Disabled => ConfigDiscoveryOption::Disabled,
    },
    maybe_custom_ende_dir_root: flags.internal.cache_path.clone(),
    no_lock: flags.no_lock
      || matches!(
        flags.subcommand,
        EndeSubcommand::Install(InstallFlags::Global(..))
          | EndeSubcommand::Uninstall(_)
      ),
    frozen_lockfile: flags.frozen_lockfile,
    lock_arg: flags.lock.as_ref().map(|l| initial_cwd.join(l)),
    lockfile_skip_write: flags.internal.lockfile_skip_write,
    vendor: flags.vendor,
  }
}
