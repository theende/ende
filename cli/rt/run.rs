// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::OnceLock;

use deno_error::JsErrorBox;
use deno_media_type::MediaType;
use ende_config::workspace::ResolverWorkspaceJsrPackage;
use ende_core::ModuleLoadOptions;
use ende_core::ModuleLoadReferrer;
use ende_core::ModuleLoader;
use ende_core::ModuleSourceCode;
use ende_core::ModuleSpecifier;
use ende_core::ModuleType;
use ende_core::RequestedModuleType;
use ende_core::ResolutionKind;
use ende_core::SourceCodeCacheInfo;
use ende_core::error::AnyError;
use ende_core::error::ModuleLoaderError;
use ende_core::futures::FutureExt;
use ende_core::futures::future::LocalBoxFuture;
use ende_core::url::Url;
use ende_core::v8_set_flags;
use ende_lib::args::CaData;
use ende_lib::args::RootCertStoreLoadError;
use ende_lib::args::get_root_cert_store;
use ende_lib::loader::module_type_from_media_and_requested_type;
use ende_lib::util::hash::FastInsecureHasher;
use ende_lib::util::v8::construct_v8_flags;
use ende_lib::worker::CreateModuleLoaderResult;
use ende_lib::worker::LibMainWorkerFactory;
use ende_lib::worker::LibMainWorkerOptions;
use ende_lib::worker::ModuleLoaderFactory;
use ende_lib::worker::StorageKeyResolver;
use ende_resolver::workspace::MappedResolution;
use ende_resolver::workspace::SloppyImportsOptions;
use ende_resolver::workspace::WorkspaceResolver;
use ende_runtime::FeatureChecker;
use ende_runtime::WorkerExecutionMode;
use ende_runtime::WorkerLogLevel;
use ende_runtime::code_cache::CodeCache;
use ende_runtime::ende_fs::FileSystem;
use ende_runtime::ende_permissions::Permissions;
use ende_runtime::ende_permissions::PermissionsContainer;
use ende_runtime::ende_tls::RootCertStoreProvider;
use ende_runtime::ende_tls::rustls::RootCertStore;
use ende_runtime::ende_web::Blob;
use ende_runtime::ende_web::BlobStore;
use ende_runtime::permissions::RuntimePermissionDescriptorParser;
use sys_traits::EnvCurrentDir;

use crate::binary::EndeCompileModuleSource;
use crate::binary::StandaloneData;
use crate::binary::StandaloneModules;
use crate::binary::transpile_runtime_module;
use crate::code_cache::EndeCompileCodeCache;
use crate::file_system::EndeRtSys;

struct SharedModuleLoaderState {
  blob_store: Arc<BlobStore>,
  code_cache: Option<Arc<EndeCompileCodeCache>>,
  modules: Arc<StandaloneModules>,
  workspace_resolver: WorkspaceResolver<EndeRtSys>,
}

impl SharedModuleLoaderState {
  fn get_code_cache(
    &self,
    specifier: &Url,
    source: &[u8],
  ) -> Option<SourceCodeCacheInfo> {
    let Some(code_cache) = &self.code_cache else {
      return None;
    };
    if !code_cache.enabled() {
      return None;
    }
    // ende version is already included in the root cache key
    let hash = FastInsecureHasher::new_without_ende_version()
      .write_hashable(source)
      .finish();
    let data = code_cache.get_sync(
      specifier,
      ende_runtime::code_cache::CodeCacheType::EsModule,
      hash,
    );
    Some(SourceCodeCacheInfo {
      hash,
      data: data.map(Cow::Owned),
    })
  }
}

#[derive(Clone)]
struct EmbeddedModuleLoader {
  shared: Arc<SharedModuleLoaderState>,
  sys: EndeRtSys,
  /// For blob/object-URL module workers, the captured root blob and its
  /// specifier. Used so the worker's root module load resolves from the
  /// synchronously-captured blob even if `URL.revokeObjectURL` has since
  /// removed it from the blob store.
  maybe_main_module_blob: Option<(ModuleSpecifier, Arc<Blob>)>,
}

impl std::fmt::Debug for EmbeddedModuleLoader {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("EmbeddedModuleLoader").finish()
  }
}

impl EmbeddedModuleLoader {
  fn resolve_inner(
    &self,
    raw_specifier: &str,
    referrer: &str,
    _kind: ResolutionKind,
  ) -> Result<Url, ModuleLoaderError> {
    let referrer = if referrer == "." {
      #[allow(
        clippy::disallowed_methods,
        reason = "ok to use current_dir here"
      )]
      let current_dir =
        self.sys.env_current_dir().map_err(JsErrorBox::from_err)?;
      ende_core::resolve_path(".", &current_dir)
        .map_err(JsErrorBox::from_err)?
    } else {
      Url::parse(referrer).map_err(|err| {
        JsErrorBox::type_error(format!(
          "Referrer uses invalid specifier: {}",
          err
        ))
      })?
    };
    let mapped_resolution = self.shared.workspace_resolver.resolve(
      raw_specifier,
      &referrer,
      ende_resolver::workspace::ResolutionKind::Execution,
    );

    match mapped_resolution {
      Ok(MappedResolution::WorkspaceJsrPackage { specifier, .. }) => {
        Ok(specifier)
      }
      Ok(MappedResolution::Normal { specifier, .. }) => {
        ende_resolver::check_specifier_supported(&specifier)
          .map_err(JsErrorBox::from_err)?;

        if specifier.scheme() == "jsr"
          && let Some(specifier) = self
            .shared
            .modules
            .resolve_specifier(&specifier)
            .map_err(JsErrorBox::from_err)?
        {
          return Ok(specifier.clone());
        }

        Ok(specifier)
      }
      Err(_) if ende_resolver::is_node_builtin_module_name(raw_specifier) => {
        Err(JsErrorBox::from_err(
          ende_resolver::UnsupportedSpecifierError::NodeBuiltin(
            raw_specifier.to_string(),
          ),
        ))
      }
      Err(err) => Err(JsErrorBox::from_err(err)),
    }
  }

  /// Load a module using the default embedded/filesystem logic. Used by
  /// `ModuleLoader::load` after data/blob URL routing.
  fn load_default(
    &self,
    original_specifier: &Url,
    options: ModuleLoadOptions,
  ) -> ende_core::ModuleLoadResponse {
    match self.shared.modules.read(original_specifier) {
      Ok(Some(module)) => {
        match options.requested_module_type {
          RequestedModuleType::Text | RequestedModuleType::Bytes => {
            let module_source = EndeCompileModuleSource::Bytes(module.data);
            return ende_core::ModuleLoadResponse::Sync(Ok(
              ende_core::ModuleSource::new_with_redirect(
                match options.requested_module_type {
                  RequestedModuleType::Text => ModuleType::Text,
                  RequestedModuleType::Bytes => ModuleType::Bytes,
                  _ => unreachable!(),
                },
                match options.requested_module_type {
                  RequestedModuleType::Text => module_source.into_for_v8(),
                  RequestedModuleType::Bytes => {
                    ModuleSourceCode::Bytes(module_source.into_bytes_for_v8())
                  }
                  _ => unreachable!(),
                },
                original_specifier,
                module.specifier,
                None,
              ),
            ));
          }
          RequestedModuleType::Other(_)
          | RequestedModuleType::None
          | RequestedModuleType::Json => {
            // ignore
          }
        }

        let (module_specifier, module_type, module_source) =
          module.into_parts();
        let module_source = module_source.into_for_v8();
        // Only JavaScript modules produce a V8 code cache. Requesting the
        // code cache for other module types (JSON, Wasm, etc.) would still
        // bump the `FirstRun` strategy's pending counter via `get_sync`
        // without a matching `set_sync`, preventing the cache from ever
        // being serialized. See https://github.com/denoland/deno/issues/31766
        let code_cache_entry = if module_type == ModuleType::JavaScript {
          self
            .shared
            .get_code_cache(module_specifier, module_source.as_bytes())
        } else {
          None
        };
        ende_core::ModuleLoadResponse::Sync(Ok(
          ende_core::ModuleSource::new_with_redirect(
            module_type,
            module_source,
            original_specifier,
            module_specifier,
            code_cache_entry,
          ),
        ))
      }
      Ok(None) => {
        // Fall back to reading from disk (used by dev entrypoint in HMR mode).
        if original_specifier.scheme() == "file"
          && let Ok(path) = original_specifier.to_file_path()
          && let Ok(source) = std::fs::read_to_string(&path)
        {
          let media_type = MediaType::from_specifier(original_specifier);
          let (module_type, should_transpile) = match media_type {
            MediaType::JavaScript | MediaType::Mjs | MediaType::Cjs => {
              (ModuleType::JavaScript, false)
            }
            MediaType::TypeScript
            | MediaType::Mts
            | MediaType::Cts
            | MediaType::Jsx
            | MediaType::Tsx => (ModuleType::JavaScript, true),
            MediaType::Json => (ModuleType::Json, false),
            _ => (ModuleType::JavaScript, false),
          };
          let code = if should_transpile {
            match deno_ast::parse_module(deno_ast::ParseParams {
              specifier: original_specifier.clone(),
              text: source.into(),
              media_type,
              capture_tokens: false,
              scope_analysis: false,
              maybe_syntax: None,
            }) {
              Ok(parsed) => match parsed.transpile(
                &deno_ast::TranspileOptions::default(),
                &deno_ast::TranspileModuleOptions::default(),
                &deno_ast::EmitOptions::default(),
              ) {
                Ok(transpiled) => {
                  ModuleSourceCode::String(transpiled.into_source().text.into())
                }
                Err(e) => {
                  return ende_core::ModuleLoadResponse::Sync(Err(
                    JsErrorBox::type_error(format!("Transpile error: {e}")),
                  ));
                }
              },
              Err(e) => {
                return ende_core::ModuleLoadResponse::Sync(Err(
                  JsErrorBox::type_error(format!("Parse error: {e}")),
                ));
              }
            }
          } else {
            ModuleSourceCode::String(source.into())
          };
          return ende_core::ModuleLoadResponse::Sync(Ok(
            ende_core::ModuleSource::new(
              module_type,
              code,
              original_specifier,
              None,
            ),
          ));
        }
        ende_core::ModuleLoadResponse::Sync(Err(JsErrorBox::type_error(
          format!("Module not found: {}", original_specifier),
        )))
      }
      Err(err) => ende_core::ModuleLoadResponse::Sync(Err(
        JsErrorBox::type_error(format!("{:?}", err)),
      )),
    }
  }
}

impl ModuleLoader for EmbeddedModuleLoader {
  fn resolve(
    &self,
    raw_specifier: &str,
    referrer: &str,
    kind: ResolutionKind,
  ) -> Result<ModuleSpecifier, JsErrorBox> {
    self.resolve_inner(raw_specifier, referrer, kind)
  }

  fn load(
    &self,
    original_specifier: &Url,
    _maybe_referrer: Option<&ModuleLoadReferrer>,
    options: ModuleLoadOptions,
  ) -> ende_core::ModuleLoadResponse {
    if original_specifier.scheme() == "data" {
      let raw_data_url = match deno_media_type::data_url::RawDataUrl::parse(
        original_specifier,
      ) {
        Ok(raw) => raw,
        Err(err) => {
          return ende_core::ModuleLoadResponse::Sync(Err(
            JsErrorBox::type_error(format!("{:#}", err)),
          ));
        }
      };
      let media_type = raw_data_url.media_type();
      let data_url_text = match raw_data_url.decode() {
        Ok(text) => text,
        Err(err) => {
          return ende_core::ModuleLoadResponse::Sync(Err(
            JsErrorBox::type_error(format!("{:#}", err)),
          ));
        }
      };
      // Transpile when the data URL carries TypeScript/JSX, so compiled
      // programs can dynamically import TypeScript built at runtime.
      let code = match transpile_runtime_module(
        original_specifier,
        media_type,
        data_url_text,
      ) {
        Ok(code) => code,
        Err(err) => {
          return ende_core::ModuleLoadResponse::Sync(Err(err));
        }
      };
      return ende_core::ModuleLoadResponse::Sync(Ok(
        ende_core::ModuleSource::new(
          ende_core::ModuleType::JavaScript,
          ModuleSourceCode::String(code.into()),
          original_specifier,
          None,
        ),
      ));
    }

    if original_specifier.scheme() == "blob" {
      let specifier = original_specifier.clone();
      // Prefer the synchronously-captured root blob (if this is the worker's
      // main module) so a racing `URL.revokeObjectURL` can't make the load
      // fail; otherwise look the object URL up in the blob store.
      let maybe_blob = self
        .maybe_main_module_blob
        .as_ref()
        .filter(|(blob_specifier, _)| *blob_specifier == specifier)
        .map(|(_, blob)| blob.clone())
        .or_else(|| self.shared.blob_store.get_object_url(specifier.clone()));
      let Some(blob) = maybe_blob else {
        return ende_core::ModuleLoadResponse::Sync(Err(
          JsErrorBox::type_error(format!("Blob URL not found: {specifier}")),
        ));
      };
      let requested_module_type = options.requested_module_type.clone();
      return ende_core::ModuleLoadResponse::Async(
        async move {
          let bytes = blob.read_all().await;
          let (media_type, maybe_charset) =
            deno_media_type::resolve_media_type_and_charset_from_content_type(
              &specifier,
              Some(&blob.media_type),
            );
          let module_type = module_type_from_media_and_requested_type(
            media_type,
            &requested_module_type,
          );
          let source = match module_type {
            ModuleType::Bytes | ModuleType::Wasm => {
              ModuleSourceCode::Bytes(bytes.into_boxed_slice().into())
            }
            _ => {
              // Mirror `ende run`'s charset-aware decoding (see
              // `TextDecodedFile::decode` in cli/file_fetcher.rs) so blob
              // sources honor the content-type charset and have a leading BOM
              // stripped, instead of blindly assuming UTF-8.
              let charset = maybe_charset.unwrap_or_else(|| {
                deno_media_type::encoding::detect_charset(&specifier, &bytes)
              });
              let text =
                deno_media_type::encoding::decode_owned_source(charset, bytes)
                  .map_err(|err| {
                    JsErrorBox::type_error(format!(
                      "Failed decoding \"{specifier}\": {err}"
                    ))
                  })?;
              // Transpile when the blob carries TypeScript/JSX, so compiled
              // programs can dynamically import TypeScript built at runtime.
              let text =
                transpile_runtime_module(&specifier, media_type, text)?;
              ModuleSourceCode::String(text.into())
            }
          };
          Ok(ende_core::ModuleSource::new(
            module_type,
            source,
            &specifier,
            None,
          ))
        }
        .boxed_local(),
      );
    }

    self.load_default(original_specifier, options)
  }

  fn get_code_cache(
    &self,
    specifier: &ModuleSpecifier,
    source: &str,
  ) -> Option<SourceCodeCacheInfo> {
    let code_cache = self.shared.code_cache.as_ref()?;
    if !code_cache.enabled() {
      return None;
    }
    // Residual ext-scripts don't go through
    // `load()`, so they can't ride their cache on a `ModuleSource`. Read the
    // shared on-disk cache directly here. The ende version is already part of
    // the root cache key.
    //
    // Hash the source *bytes*, not the `&str`: that's what
    // `SharedModuleLoaderState::get_code_cache` hashes for the same specifier,
    // and `&[u8]`/`&str` do not hash alike. Keep the two in sync or a module
    // reached through both paths silently misses the cache on every run.
    let hash = FastInsecureHasher::new_without_ende_version()
      .write_hashable(source.as_bytes())
      .finish();
    let data = code_cache.get_sync(
      specifier,
      ende_runtime::code_cache::CodeCacheType::EsModule,
      hash,
    );
    Some(SourceCodeCacheInfo {
      hash,
      data: data.map(Cow::Owned),
    })
  }

  fn code_cache_ready(
    &self,
    specifier: Url,
    source_hash: u64,
    code_cache_data: &[u8],
  ) -> LocalBoxFuture<'static, ()> {
    if let Some(code_cache) = &self.shared.code_cache {
      code_cache.set_sync(
        specifier,
        ende_runtime::code_cache::CodeCacheType::EsModule,
        source_hash,
        code_cache_data,
      );
    }
    std::future::ready(()).boxed_local()
  }

  fn get_source_map(&self, file_name: &str) -> Option<Cow<'_, [u8]>> {
    let url = Url::parse(file_name).ok()?;
    let data = self.shared.modules.read(&url).ok()??;
    data.source_map
  }

  fn load_external_source_map(
    &self,
    source_map_url: &str,
  ) -> Option<Cow<'_, [u8]>> {
    let url = Url::parse(source_map_url).ok()?;
    let data = self.shared.modules.read(&url).ok()??;
    Some(Cow::Owned(data.data.to_vec()))
  }

  fn source_map_source_exists(&self, _source_url: &str) -> Option<bool> {
    Some(true)
  }

  fn get_source_mapped_source_line(
    &self,
    file_name: &str,
    line_number: usize,
  ) -> Option<String> {
    let specifier = Url::parse(file_name).ok()?;
    let data = self.shared.modules.read(&specifier).ok()??;

    let source = String::from_utf8_lossy(&data.data);
    // Do NOT use .lines(): it skips the terminating empty line.
    // (due to internally using_terminator() instead of .split())
    let lines: Vec<&str> = source.split('\n').collect();
    if line_number >= lines.len() {
      Some(format!(
        "{} Couldn't format source line: Line {} is out of bounds (source may have changed at runtime)",
        crate::colors::yellow("Warning"),
        line_number + 1,
      ))
    } else {
      Some(lines[line_number].to_string())
    }
  }
}

struct StandaloneModuleLoaderFactory {
  shared: Arc<SharedModuleLoaderState>,
  sys: EndeRtSys,
}

impl StandaloneModuleLoaderFactory {
  pub fn create_result(
    &self,
    maybe_main_module_blob: Option<(ModuleSpecifier, Arc<Blob>)>,
  ) -> CreateModuleLoaderResult {
    let loader = Rc::new(EmbeddedModuleLoader {
      shared: self.shared.clone(),
      sys: self.sys.clone(),
      maybe_main_module_blob,
    });
    CreateModuleLoaderResult {
      module_loader: loader,
    }
  }
}

impl ModuleLoaderFactory for StandaloneModuleLoaderFactory {
  fn create_for_main(
    &self,
    _root_permissions: PermissionsContainer,
  ) -> CreateModuleLoaderResult {
    self.create_result(None)
  }

  fn create_for_worker(
    &self,
    _parent_permissions: PermissionsContainer,
    _permissions: PermissionsContainer,
    maybe_main_module_blob: Option<(ModuleSpecifier, Arc<Blob>)>,
  ) -> CreateModuleLoaderResult {
    self.create_result(maybe_main_module_blob)
  }
}

struct StandaloneRootCertStoreProvider {
  sys: EndeRtSys,
  ca_stores: Option<Vec<String>>,
  ca_data: Option<CaData>,
  cell: OnceLock<Result<RootCertStore, RootCertStoreLoadError>>,
}

impl RootCertStoreProvider for StandaloneRootCertStoreProvider {
  fn get_or_try_init(&self) -> Result<&RootCertStore, JsErrorBox> {
    self
      .cell
      // get_or_try_init was not stable yet when this was written
      .get_or_init(|| {
        get_root_cert_store(
          &self.sys,
          None,
          self.ca_stores.clone(),
          self.ca_data.clone(),
        )
      })
      .as_ref()
      .map_err(|err| JsErrorBox::from_err(err.clone()))
  }
}

/// Callback to initialize additional `OpState` during worker creation.
pub type OpStateInitFn = Box<dyn FnOnce(&mut ende_core::OpState) + Send>;

/// Error raised while a compiled desktop app's main module is still loading or
/// first evaluating (e.g. a failed import, a link error, or a top-level
/// throw/await rejection during evaluation).
///
/// Such failures happen before the app's own `error` / `unhandledrejection`
/// listeners can observe them (the event loop hasn't started yet), so the
/// desktop shell can't rely on the app to report them. It carries a
/// pre-formatted, user-facing message so the shell can surface it in a native
/// dialog: a GUI-launched app (Finder/Dock) has no visible stderr, so an
/// unsurfaced startup error otherwise presents as a window that blinks open and
/// immediately closes (deno#35544).
///
/// The original error is retained as the [`std::error::Error::source`] so the
/// `{:?}` debug logs in the desktop shell still print the full cause chain.
#[derive(Debug)]
pub struct DesktopStartupError {
  /// Pre-formatted, user-facing error message.
  pub message: String,
  /// The original error, kept so the cause chain survives in debug logs.
  source: AnyError,
}

impl std::fmt::Display for DesktopStartupError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(&self.message)
  }
}

impl std::error::Error for DesktopStartupError {
  fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
    Some(self.source.as_ref())
  }
}

/// Wrap a desktop main-module load/evaluation failure as a
/// [`DesktopStartupError`], formatting JS errors the same way the runtime would
/// print them to stderr while retaining the original error as the source.
fn desktop_startup_error<E: Into<AnyError>>(err: E) -> AnyError {
  let err: AnyError = err.into();
  let message = match ende_lib::util::result::js_error_downcast_ref(&err) {
    Some(js_error) => ende_runtime::fmt_errors::format_js_error(js_error, None),
    None => format!("{err:?}"),
  };
  AnyError::new(DesktopStartupError {
    message,
    source: err,
  })
}

/// Options to override default standalone runtime behavior.
/// Used by the desktop runtime to enable auto_serve and set the port.
#[derive(Default)]
pub struct RunOptions {
  pub auto_serve: bool,
  pub serve_port: Option<u16>,
  pub serve_host: Option<String>,
  /// Enable HMR file watching from this directory.
  pub hmr_watch_dir: Option<PathBuf>,
  /// Callback invoked after each successful HMR replacement.
  pub hmr_on_reload: Option<crate::hmr::HmrReloadCallback>,
  /// Callback to initialize additional OpState (e.g. desktop APIs).
  /// Called during worker creation to inject state without adding extensions.
  pub op_state_init: Option<OpStateInitFn>,
  /// Version and rollback state for desktop auto-update JS initialization.
  pub auto_update_version: Option<String>,
  pub auto_update_rolled_back: bool,
  /// Error reporting URL from ende.json `desktop.errorReporting.url`.
  pub error_reporting_url: Option<String>,
  /// Auto-update release base URL from ende.json `desktop.release.baseUrl`.
  /// Used as the default `url` for `Ende.autoUpdate` when none is passed.
  pub release_base_url: Option<String>,
  /// Stop on the first line of user code. Mirrors `--inspect-brk`.
  pub inspect_brk: bool,
  /// Wait for a DevTools session to attach before running user code.
  /// Mirrors `--inspect-wait`.
  pub inspect_wait: bool,
  /// Enables inspector-related setup in the worker (registers the runtime
  /// with the global inspector server). Mirrors `--inspect`/`-brk`/`-wait`.
  pub is_inspecting: bool,
}

pub async fn run(
  fs: Arc<dyn FileSystem>,
  sys: EndeRtSys,
  data: StandaloneData,
) -> Result<i32, AnyError> {
  run_with_options(fs, sys, data, RunOptions::default()).await
}

pub async fn run_with_options(
  fs: Arc<dyn FileSystem>,
  sys: EndeRtSys,
  data: StandaloneData,
  options: RunOptions,
) -> Result<i32, AnyError> {
  let hmr_watch_dir = options.hmr_watch_dir;
  let hmr_on_reload = options.hmr_on_reload;
  let op_state_init = options.op_state_init;
  let StandaloneData {
    metadata,
    modules,
    root_path,
    vfs: _,
  } = data;

  let root_cert_store_provider = Arc::new(StandaloneRootCertStoreProvider {
    sys: sys.clone(),
    ca_stores: metadata.ca_stores,
    ca_data: metadata.ca_data.map(CaData::Bytes),
    cell: Default::default(),
  });
  let root_dir_url = Arc::new(Url::from_directory_path(&root_path).unwrap());
  let workspace_root_path =
    hmr_watch_dir.clone().unwrap_or_else(|| root_path.clone());
  let workspace_root_dir_url =
    Arc::new(Url::from_directory_path(&workspace_root_path).unwrap());
  let entrypoint = root_dir_url.join(&metadata.entrypoint_key).unwrap();
  let main_module = entrypoint;
  let workspace_resolver = {
    let import_map = match metadata.workspace_resolver.import_map {
      Some(import_map) => Some(
        import_map::parse_from_json_with_options(
          workspace_root_dir_url.join(&import_map.specifier).unwrap(),
          &import_map.json,
          import_map::ImportMapOptions {
            address_hook: None,
            expand_imports: true,
          },
        )?
        .import_map,
      ),
      None => None,
    };
    WorkspaceResolver::new_raw(
      import_map,
      metadata
        .workspace_resolver
        .jsr_pkgs
        .iter()
        .map(|pkg| ResolverWorkspaceJsrPackage {
          is_link: false, // only used for enhancing the diagnostic, which isn't shown in ende compile
          base: root_dir_url.join(&pkg.relative_base).unwrap(),
          name: pkg.name.clone(),
          version: pkg.version.clone(),
          exports: pkg.exports.clone(),
        })
        .collect(),
      if metadata.unstable_config.sloppy_imports {
        SloppyImportsOptions::Enabled
      } else {
        SloppyImportsOptions::Unspecified
      },
      Default::default(),
      sys.clone(),
    )
  };
  let code_cache = match metadata.code_cache_key {
    Some(code_cache_key) => Some(Arc::new(EndeCompileCodeCache::new(
      root_path.with_file_name(format!(
        "{}.cache",
        root_path.file_name().unwrap().to_string_lossy()
      )),
      code_cache_key,
    ))),
    None => {
      log::debug!("Code cache disabled.");
      None
    }
  };
  let blob_store = Arc::new(BlobStore::default());
  let module_loader_factory = StandaloneModuleLoaderFactory {
    shared: Arc::new(SharedModuleLoaderState {
      blob_store: blob_store.clone(),
      code_cache: code_cache.clone(),
      modules,
      workspace_resolver,
    }),
    sys: sys.clone(),
  };

  // Desktop runtimes always need a baseline set of permissions to function:
  // the renderer is served over loopback HTTP, so net access to 127.0.0.1
  // is mandatory; the desktop init JS reads `ENDE_DESKTOP_*` env vars to
  // discover the inspector mux, HMR watch dir, etc. Without these, the
  // very first `Ende.serve(...)` or `Ende.env.get(...)` throws NotCapable
  // and aborts script execution before the event-loop IIFE registers —
  // which manifests as a blank window where nothing works (no input,
  // no auto-tests, no bindings). Granting them up front avoids forcing
  // every `ende desktop` user to remember `-A`.
  let has_desktop = op_state_init.is_some();
  let permissions = {
    let mut permissions = metadata.permissions;
    // grant read access to the vfs
    grant_vfs_read_access(&mut permissions, &root_path);

    if has_desktop {
      apply_desktop_permission_defaults(&mut permissions);
    }

    let desc_parser =
      Arc::new(RuntimePermissionDescriptorParser::new(sys.clone()));
    let permissions =
      Permissions::from_options(desc_parser.as_ref(), &permissions)?;
    PermissionsContainer::new(desc_parser, permissions)
  };
  let feature_checker = Arc::new({
    let mut checker = FeatureChecker::default();
    checker.set_exit_cb(Box::new(crate::unstable_exit_cb));
    for feature in metadata.unstable_config.features {
      // `metadata` is valid for the whole lifetime of the program, so we
      // can leak the string here.
      checker.enable_feature(feature.leak());
    }
    // Mirror cli/factory.rs: force-enable the unstable KV feature when
    // ENDE_KV_REQUIRES_DISTRIBUTED_DATABASE=error so Ende.openKv() is exposed
    // and surfaces a clear "no database attached" error.
    if ende_env::var("ENDE_KV_REQUIRES_DISTRIBUTED_DATABASE").as_deref()
      == Ok("error")
      && !checker.check("kv")
    {
      checker.enable_feature("kv");
    }
    checker
  });
  // Resolve a persistent, per-app data directory so origin-bound storage such
  // as the default `Ende.openKv()`, `localStorage` and `caches` persists to
  // disk instead of silently falling back to an in-memory database. A compiled
  // binary is a standalone app, so we store under the platform's app data
  // directory (`%LOCALAPPDATA%`, `~/Library/Application Support`,
  // `$XDG_DATA_HOME`) keyed by the app name, rather than polluting `ENDE_DIR`.
  //
  // The app name is baked in at compile time (`--app-name`, otherwise the
  // output file name), so it is stable across runs and even if the binary is
  // later renamed. Recompiling with a different `--output`/`--app-name` starts
  // a fresh store. If we can't resolve a data directory we leave both pieces
  // unset and keep the in-memory fallback.
  //
  // `metadata.app_name` is always set by binaries this version produces; the
  // fallback to the executable file stem only matters for binaries compiled
  // before the field existed.
  let app_name = metadata.app_name.unwrap_or_else(|| {
    std::env::current_exe()
      .ok()
      .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
      .unwrap_or_else(|| "ende-compile".to_string())
  });
  // `--app-name` is validated at compile time (see `validate_app_name`), so it
  // is safe to use directly as a single directory component here.
  let origin_data_folder_path =
    crate::binary::get_data_local_dir().map(|dir| dir.join(&app_name));
  // Only enable persistent storage when we resolved a data directory. The
  // storage key resolver must stay empty otherwise, because the worker unwraps
  // `origin_data_folder_path` whenever the key resolves to a value.
  let storage_key_resolver = if origin_data_folder_path.is_some() {
    StorageKeyResolver::from_compile_app_name(&app_name)
  } else {
    StorageKeyResolver::empty()
  };
  let lib_main_worker_options = LibMainWorkerOptions {
    argv: metadata.argv,
    log_level: WorkerLogLevel::Info,
    enable_testing_features: false,
    inspect_brk: options.inspect_brk,
    inspect_wait: options.inspect_wait,
    trace_ops: None,
    is_inspecting: options.is_inspecting,
    is_standalone: true,
    auto_serve: options.auto_serve,
    skip_op_registration: true,
    location: metadata.location,
    origin_data_folder_path,
    seed: metadata.seed,
    unsafely_ignore_certificate_errors: metadata
      .unsafely_ignore_certificate_errors,
    serve_port: options.serve_port,
    serve_host: options.serve_host,
    otel_config: metadata.otel_config,
    no_legacy_abort: false,
    startup_snapshot: ende_snapshots::CLI_SNAPSHOT,
    residual_lazy_js_sources: ende_snapshots::RESIDUAL_LAZY_JS,
    residual_lazy_esm_sources: ende_snapshots::RESIDUAL_LAZY_ESM,
    enable_raw_imports: metadata.unstable_config.raw_imports,
    close_on_idle: !options.auto_serve,
    maybe_initial_cwd: None,
    disable_offscreen_canvas: matches!(
      ende_env::var("ENDE_DISABLE_OFFSCREEN_CANVAS").as_deref(),
      Ok("1") | Ok("true")
    ),
  };
  let worker_factory = LibMainWorkerFactory::new(
    blob_store,
    code_cache.map(|c| c.for_ende_core()),
    sys.maybe_native_addon_loader(),
    feature_checker,
    fs,
    None, // maybe_coverage_dir
    None, // maybe_cpu_prof_config
    Box::new(module_loader_factory),
    root_cert_store_provider,
    storage_key_resolver,
    sys.clone(),
    lib_main_worker_options,
    Default::default(),
  );

  // Initialize v8 once from the main thread.
  v8_set_flags(construct_v8_flags(&[], &metadata.v8_flags, vec![]));
  let is_single_threaded = metadata
    .v8_flags
    .contains(&String::from("--single-threaded"));
  let v8_platform = if is_single_threaded {
    Some(::ende_core::v8::Platform::new_single_threaded(true).make_shared())
  } else {
    None
  };
  ende_core::JsRuntime::init_platform(v8_platform);

  let preload_modules = metadata
    .preload_modules
    .iter()
    .map(|key| root_dir_url.join(key).unwrap())
    .collect::<Vec<_>>();

  let mut worker = worker_factory.create_custom_worker(
    WorkerExecutionMode::Run,
    main_module,
    preload_modules,
    permissions,
    vec![],
    Default::default(),
    None,
  )?;

  // Inject desktop API state into OpState after worker creation.
  if let Some(init_fn) = op_state_init {
    let op_state = worker.js_runtime().op_state();
    init_fn(&mut op_state.borrow_mut());
  }

  // Initialize desktop APIs (Ende.desktop.*).
  if has_desktop {
    worker
      .js_runtime()
      .execute_script("ext:ende_desktop/init", crate::desktop::DESKTOP_JS)?;

    // Initialize auto-update JS (DesktopUpdater cppgc object is already
    // registered via the desktop extension's objects).
    let js = crate::desktop::desktop_auto_update_js(
      options.auto_update_version.as_deref(),
      options.auto_update_rolled_back,
      options.release_base_url.as_deref(),
    );
    worker
      .js_runtime()
      .execute_script("ext:ende_desktop/auto_update", js)?;

    // Make the operator-configured reporting URL available to the native
    // op (and the panic hook) via `ERROR_REPORT_CONFIG`. The op reads the
    // destination from here rather than trusting a JS-supplied URL, so an
    // untrusted caller can't retarget it. `OnceLock::set` is a no-op if a
    // path that already knows the URL (e.g. `run_desktop`) set it first.
    if let Some(url) = options.error_reporting_url.as_deref() {
      ende_runtime::ops::desktop::set_error_report_config(
        url.to_string(),
        options.auto_update_version.clone(),
      );
    }

    let js = crate::desktop::desktop_error_reporting_js(
      options.error_reporting_url.as_deref(),
      options.auto_update_version.as_deref(),
    );
    worker
      .js_runtime()
      .execute_script("ext:ende_desktop/error_reporting", js)?;
  }

  if let Some(watch_dir) = hmr_watch_dir {
    let mut hmr_runner =
      crate::hmr::setup_desktop_hmr(&mut worker, watch_dir, root_path.clone())?;
    if let Some(cb) = hmr_on_reload {
      hmr_runner.set_on_reload(cb);
    }

    // Run one event loop tick so the inspector processes the
    // Debugger.enable / Runtime.enable messages before we load modules.
    // This ensures scriptParsed notifications are emitted during module load.
    worker.run_event_loop(false).await?;

    worker.execute_load_phase().await?;

    loop {
      let hmr_fut = hmr_runner.run();
      let event_loop_fut = worker.run_event_loop(false);

      tokio::select! {
        hmr_result = hmr_fut => {
          if let Err(e) = hmr_result {
            log::error!("HMR error: {:?}", e);
          }
        }
        event_loop_result = event_loop_fut => {
          event_loop_result?;
          let web_continue = worker.dispatch_beforeunload_event()?;
          if !web_continue {
            break;
          }
        }
      }
    }

    worker.dispatch_unload_event()?;
    Ok(worker.exit_code())
  } else if has_desktop {
    // Same as `LibMainWorker::run()`, but tag any failure from the load phase
    // (main module still loading or first evaluating) as a
    // `DesktopStartupError` (see its docs for why these otherwise vanish). The
    // tag lets the desktop shell surface them in a native dialog. Errors raised
    // later, during the steady-state event loop, are left untagged: those are
    // already reported by the app's JS listeners, and tagging them would
    // produce a duplicate dialog.
    worker
      .execute_load_phase()
      .await
      .map_err(desktop_startup_error)?;
    Ok(worker.run_event_loop_to_completion().await?)
  } else {
    let exit_code = worker.run().await?;
    Ok(exit_code)
  }
}

/// Grant read access to the embedded VFS root so the compiled binary
/// can read the files it shipped with. Preserves the `Some(vec![])`
/// allow-all sentinel and avoids duplicating an already-present entry.
pub(crate) fn grant_vfs_read_access(
  permissions: &mut ende_runtime::ende_permissions::PermissionsOptions,
  root_path: &std::path::Path,
) {
  let entry = root_path.to_string_lossy().into_owned();
  match &mut permissions.allow_read {
    Some(vec) if vec.is_empty() => {
      // already wide-open (--allow-read with no args / -A); nothing to
      // extend.
    }
    Some(vec) => {
      if !vec.contains(&entry) {
        vec.push(entry);
      }
    }
    None => {
      permissions.allow_read = Some(vec![entry]);
    }
  }
}

/// Loopback hosts a desktop runtime must be able to reach. The compiled
/// app's renderer talks to its `Ende.serve()` over loopback HTTP, so
/// net access here is structural, not optional.
const DESKTOP_LOOPBACK_HOSTS: &[&str] = &["127.0.0.1", "localhost", "[::1]"];

/// Env vars the DESKTOP_JS init script reads. Granting only these
/// (instead of blanket `allow-env`) keeps the user's shell environment
/// off-limits to arbitrary `Ende.env.get` calls in their app code while
/// still letting the framework boot.
const DESKTOP_ENV_KEYS: &[&str] = &[
  "ENDE_DESKTOP_MUX_WS",
  "ENDE_DESKTOP_HMR",
  "ENDE_DESKTOP_INSPECT_INTERNAL_PORT",
  "ENDE_DESKTOP_INSPECT_BRK",
  "ENDE_DESKTOP_INSPECT_WAIT",
  "LAUFEY_RUNTIME_PATH",
  "LAUFEY_REMOTE_DEBUGGING_PORT",
];

/// Grant the baseline net (loopback) and env (DESKTOP_JS read keys)
/// access a desktop runtime needs to boot. Idempotent: re-applying
/// preserves any wider grants the user already had.
///
/// `Some(vec)` with `vec.is_empty()` represents allow-all (e.g. `-A`)
/// and is left untouched. `Some(vec)` with entries gets the missing
/// loopback/key entries appended. `None` is replaced with the baseline
/// vec.
pub(crate) fn apply_desktop_permission_defaults(
  permissions: &mut ende_runtime::ende_permissions::PermissionsOptions,
) {
  fn extend(slot: &mut Option<Vec<String>>, defaults: &[&str]) {
    match slot {
      Some(vec) if vec.is_empty() => {
        // already allow-all — wider than our baseline; don't narrow.
      }
      Some(vec) => {
        for entry in defaults {
          let s = (*entry).to_string();
          if !vec.contains(&s) {
            vec.push(s);
          }
        }
      }
      None => {
        *slot = Some(defaults.iter().map(|s| (*s).to_string()).collect());
      }
    }
  }

  extend(&mut permissions.allow_net, DESKTOP_LOOPBACK_HOSTS);
  extend(&mut permissions.allow_env, DESKTOP_ENV_KEYS);
}

#[cfg(test)]
mod tests {
  use ende_runtime::ende_permissions::PermissionsOptions;

  use super::DESKTOP_ENV_KEYS;
  use super::DESKTOP_LOOPBACK_HOSTS;
  use super::DesktopStartupError;
  use super::apply_desktop_permission_defaults;
  use super::desktop_startup_error;
  use super::grant_vfs_read_access;

  fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
  }

  #[test]
  fn desktop_startup_error_tags_and_carries_message() {
    // A desktop main-module load/eval failure is wrapped so the shell can
    // recognize it (downcast) and show the carried message in a dialog,
    // instead of letting it exit silently (deno#35544).
    let original =
      deno_error::JsErrorBox::generic("could not load workspace member");
    let wrapped = desktop_startup_error(original);
    let startup = wrapped
      .downcast_ref::<DesktopStartupError>()
      .expect("must be tagged as a startup error");
    assert!(startup.message.contains("could not load workspace member"));
    // Display delegates to the carried message.
    assert_eq!(startup.to_string(), startup.message);
    // The original error is retained as the source so `{:?}` debug logs keep
    // the full cause chain.
    let source =
      std::error::Error::source(startup).expect("source must be retained");
    assert!(
      source
        .to_string()
        .contains("could not load workspace member")
    );
  }

  #[test]
  fn defaults_populate_empty_options() {
    let mut p = PermissionsOptions::default();
    apply_desktop_permission_defaults(&mut p);
    assert_eq!(p.allow_net, Some(strs(DESKTOP_LOOPBACK_HOSTS)));
    assert_eq!(p.allow_env, Some(strs(DESKTOP_ENV_KEYS)));
    // Other permission categories must remain untouched.
    assert_eq!(p.allow_read, None);
    assert_eq!(p.allow_write, None);
  }

  #[test]
  fn defaults_extend_existing_allowlist_without_duplicating() {
    let mut p = PermissionsOptions {
      allow_net: Some(vec![
        "example.com".to_string(),
        "127.0.0.1".to_string(), // already present — must not duplicate
      ]),
      allow_env: Some(vec!["MY_VAR".to_string()]),
      ..Default::default()
    };
    apply_desktop_permission_defaults(&mut p);

    let net = p.allow_net.unwrap();
    // Original entries preserved.
    assert!(net.contains(&"example.com".to_string()));
    // 127.0.0.1 still appears exactly once.
    assert_eq!(net.iter().filter(|s| *s == "127.0.0.1").count(), 1);
    // Missing loopback entries appended.
    assert!(net.contains(&"localhost".to_string()));
    assert!(net.contains(&"[::1]".to_string()));

    let env = p.allow_env.unwrap();
    assert!(env.contains(&"MY_VAR".to_string()));
    for key in DESKTOP_ENV_KEYS {
      assert!(env.contains(&key.to_string()), "missing {key}");
    }
  }

  #[test]
  fn defaults_preserve_allow_all_sentinel() {
    // `Some(vec![])` is the allow-all sentinel (-A / --allow-net with
    // no args). Narrowing it down to a specific allowlist would silently
    // break user code that depends on broader access — leave it alone.
    let mut p = PermissionsOptions {
      allow_net: Some(vec![]),
      allow_env: Some(vec![]),
      ..Default::default()
    };
    apply_desktop_permission_defaults(&mut p);
    assert_eq!(p.allow_net, Some(vec![]));
    assert_eq!(p.allow_env, Some(vec![]));
  }

  #[test]
  fn defaults_are_idempotent() {
    let mut p = PermissionsOptions::default();
    apply_desktop_permission_defaults(&mut p);
    let after_once = p.clone();
    apply_desktop_permission_defaults(&mut p);
    assert_eq!(p, after_once);
  }

  // --- grant_vfs_read_access ---

  #[test]
  fn vfs_grant_populates_empty_options() {
    let mut p = PermissionsOptions::default();
    grant_vfs_read_access(&mut p, std::path::Path::new("/data/vfs"));
    assert_eq!(p.allow_read, Some(vec!["/data/vfs".to_string()]));
    // Net / env / write must remain None.
    assert_eq!(p.allow_net, None);
    assert_eq!(p.allow_env, None);
    assert_eq!(p.allow_write, None);
  }

  #[test]
  fn vfs_grant_preserves_allow_all_sentinel() {
    // `Some(vec![])` means --allow-read with no args (allow-all). The
    // compiled binary should keep its broader privilege; narrowing it
    // to a single path here would silently break user code that reads
    // outside the VFS.
    let mut p = PermissionsOptions {
      allow_read: Some(vec![]),
      ..Default::default()
    };
    grant_vfs_read_access(&mut p, std::path::Path::new("/data/vfs"));
    assert_eq!(p.allow_read, Some(vec![]));
  }

  #[test]
  fn vfs_grant_extends_existing_allowlist() {
    let mut p = PermissionsOptions {
      allow_read: Some(vec!["/etc".to_string()]),
      ..Default::default()
    };
    grant_vfs_read_access(&mut p, std::path::Path::new("/data/vfs"));
    let v = p.allow_read.unwrap();
    assert!(
      v.contains(&"/etc".to_string()),
      "original entries preserved"
    );
    assert!(v.contains(&"/data/vfs".to_string()));
  }

  #[test]
  fn vfs_grant_is_idempotent() {
    let mut p = PermissionsOptions::default();
    grant_vfs_read_access(&mut p, std::path::Path::new("/data/vfs"));
    grant_vfs_read_access(&mut p, std::path::Path::new("/data/vfs"));
    // Same path twice must not duplicate.
    assert_eq!(p.allow_read, Some(vec!["/data/vfs".to_string()]));
  }

  #[test]
  fn vfs_grant_handles_path_with_unicode() {
    let mut p = PermissionsOptions::default();
    grant_vfs_read_access(&mut p, std::path::Path::new("/Users/café/My App"));
    assert_eq!(p.allow_read, Some(vec!["/Users/café/My App".to_string()]));
  }
}
