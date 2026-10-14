// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::cell::Cell;
use std::env;
use std::ffi::OsString;
use std::fs::File;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use capacity_builder::BytesAppendable;
use deno_ast::MediaType;
use deno_ast::ModuleKind;
use deno_ast::ModuleSpecifier;
use deno_graph::ModuleGraph;
use deno_path_util::fs::atomic_write_file_with_retries;
use deno_path_util::url_from_directory_path;
use deno_path_util::url_to_file_path;
use ende_cache_dir::CACHE_PERM;
use ende_core::anyhow::Context;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::serde_json;
use ende_core::url::Url;
use ende_lib::args::CaData;
use ende_lib::args::UnstableConfig;
use ende_lib::shared::ReleaseChannel;
use ende_lib::standalone::binary::MAGIC_BYTES;
use ende_lib::standalone::binary::Metadata;
use ende_lib::standalone::binary::RemoteModuleEntry;
use ende_lib::standalone::binary::SerializedResolverWorkspaceJsrPackage;
use ende_lib::standalone::binary::SerializedWorkspaceResolver;
use ende_lib::standalone::binary::SerializedWorkspaceResolverImportMap;
use ende_lib::standalone::binary::SpecifierDataStore;
use ende_lib::standalone::binary::SpecifierId;
use ende_lib::standalone::virtual_fs::BuiltVfs;
use ende_lib::standalone::virtual_fs::VfsBuilder;
use ende_lib::standalone::virtual_fs::WindowsSystemRootablePath;
use ende_lib::util::hash::FastInsecureHasher;
use ende_lib::util::text_encoding::is_valid_utf8;
use ende_lib::util::v8::construct_v8_flags;
use ende_lib::version::ENDE_VERSION_INFO;
use ende_resolver::file_fetcher::FetchLocalOptions;
use ende_resolver::file_fetcher::FetchOptions;
use ende_resolver::file_fetcher::FetchPermissionsOptionRef;
use ende_resolver::workspace::WorkspaceResolver;
use indexmap::IndexMap;

use super::virtual_fs::output_vfs;
use crate::args::CliOptions;
use crate::args::CompileFlags;
use crate::args::JavaScriptEngine;
use crate::args::get_default_v8_flags;
use crate::args::resolve_compile_target;
use crate::cache::EndeDir;
use crate::file_fetcher::CliFileFetcher;
use crate::http_util::HttpClientProvider;
use crate::module_loader::CliEmitter;
use crate::sys::CliSys;
use crate::util::archive;
use crate::util::env::handle_dotenv_error;
use crate::util::env::handle_dotenv_io_error;
use crate::util::env::handle_dotenv_not_found;
use crate::util::progress_bar::ProgressBar;
use crate::util::progress_bar::ProgressBarStyle;
use crate::util::progress_bar::ProgressMessagePrompt;

/// A URL that can be designated as the base for relative URLs.
///
/// After creation, this URL may be used to get the key for a
/// module in the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StandaloneRelativeFileBaseUrl<'a> {
  WindowsSystemRoot,
  Path(&'a Url),
}

impl StandaloneRelativeFileBaseUrl<'_> {
  /// Gets the module map key of the provided specifier.
  ///
  /// * Descendant file specifiers will be made relative to the base.
  /// * Non-descendant file specifiers will stay as-is (absolute).
  /// * Non-file specifiers will stay as-is.
  pub fn specifier_key<'b>(&self, target: &'b Url) -> Cow<'b, str> {
    if target.scheme() != "file" {
      return Cow::Borrowed(target.as_str());
    }
    let base = match self {
      Self::Path(base) => base,
      Self::WindowsSystemRoot => return Cow::Borrowed(target.path()),
    };

    match base.make_relative(target) {
      Some(relative) => {
        // This is not a great scenario to have because it means that the
        // specifier is outside the vfs and could cause the binary to act
        // strangely. If you encounter this, the fix is to add more paths
        // to the vfs builder by calling `add_possible_min_root_dir`.
        debug_assert!(
          !relative.starts_with("../"),
          "{} -> {} ({})",
          base.as_str(),
          target.as_str(),
          relative,
        );
        Cow::Owned(relative)
      }
      None => Cow::Borrowed(target.as_str()),
    }
  }
}

struct SpecifierStore<'a> {
  data: IndexMap<&'a Url, SpecifierId>,
}

impl<'a> SpecifierStore<'a> {
  pub fn with_capacity(capacity: usize) -> Self {
    Self {
      data: IndexMap::with_capacity(capacity),
    }
  }

  pub fn get_or_add(&mut self, specifier: &'a Url) -> SpecifierId {
    let len = self.data.len();
    let entry = self.data.entry(specifier);
    match entry {
      indexmap::map::Entry::Occupied(occupied_entry) => *occupied_entry.get(),
      indexmap::map::Entry::Vacant(vacant_entry) => {
        let new_id = SpecifierId::new(len as u32);
        vacant_entry.insert(new_id);
        new_id
      }
    }
  }

  pub fn for_serialization(
    self,
    base_url: &StandaloneRelativeFileBaseUrl<'a>,
  ) -> SpecifierStoreForSerialization<'a> {
    SpecifierStoreForSerialization {
      data: self
        .data
        .into_iter()
        .map(|(specifier, id)| (base_url.specifier_key(specifier), id))
        .collect(),
    }
  }
}

struct SpecifierStoreForSerialization<'a> {
  data: Vec<(Cow<'a, str>, SpecifierId)>,
}

impl<'a> BytesAppendable<'a> for &'a SpecifierStoreForSerialization<'a> {
  fn append_to_builder<TBytes: capacity_builder::BytesType>(
    self,
    builder: &mut capacity_builder::BytesBuilder<'a, TBytes>,
  ) {
    builder.append_le(self.data.len() as u32);
    for (specifier_str, id) in &self.data {
      builder.append_le(specifier_str.len() as u32);
      builder.append(specifier_str.as_ref());
      builder.append(*id);
    }
  }
}

pub fn is_standalone_binary(exe_path: &Path) -> bool {
  let Ok(data) = std::fs::read(exe_path) else {
    return false;
  };
  libsui::utils::is_elf(&data)
    || libsui::utils::is_pe(&data)
    || libsui::utils::is_macho(&data)
}

/// Validate a user-provided `--app-name`. The name becomes a single directory
/// component under the platform's app data directory at runtime. Because a
/// binary can be cross-compiled, validate against the union of what every
/// target OS allows so the baked identity resolves to one unambiguous
/// directory component everywhere, rather than escaping the directory or
/// failing on the target's filesystem. Done here so the user gets a clear
/// compile-time error instead of a surprising (or unusable) store location.
fn validate_app_name(app_name: &str) -> Result<(), AnyError> {
  const RESERVED_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6",
    "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6",
    "lpt7", "lpt8", "lpt9",
  ];

  // Windows reserved device names match case-insensitively against the portion
  // before the first `.` (so `nul`, `NUL`, and `nul.txt` all match).
  let stem = app_name.split('.').next().unwrap_or(app_name);
  let is_reserved_name =
    RESERVED_NAMES.iter().any(|n| stem.eq_ignore_ascii_case(n));

  let reason = if app_name.is_empty() {
    Some("must not be empty")
  } else if app_name == "." || app_name == ".." {
    Some("must not be `.` or `..`")
  } else if app_name
    // `/` and `\` are path separators; `<>:"|?*` are reserved on Windows;
    // control characters are rejected by the filesystem.
    .contains(|c: char| {
      matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')
        || c.is_control()
    })
  {
    Some("must not contain path separators or any of `<>:\"|?*`")
  } else if app_name.ends_with('.')
    || app_name.ends_with(' ')
    || app_name.starts_with(' ')
  {
    // Windows silently strips trailing dots and spaces, which would change the
    // identity out from under the user; a leading space is an error-prone
    // directory name everywhere, so reject it too.
    Some("must not start or end with a space, or end with a `.`")
  } else if is_reserved_name {
    Some("must not be a reserved device name (e.g. `CON`, `NUL`, `COM1`)")
  } else {
    None
  };

  if let Some(reason) = reason {
    bail!("Invalid `--app-name` value {:?}: {}.", app_name, reason);
  }
  Ok(())
}

fn default_app_name(display_output_filename: &str, is_desktop: bool) -> String {
  if is_desktop {
    // A desktop build's compile output is an intermediate shared library. The
    // final app is packaged without this platform-specific extension, so keep
    // its baked identity (and default window title) in sync with that name.
    Path::new(display_output_filename)
      .file_stem()
      .and_then(|stem| stem.to_str())
      .unwrap_or(display_output_filename)
      .to_string()
  } else {
    display_output_filename
      .strip_suffix(".exe")
      .unwrap_or(display_output_filename)
      .to_string()
  }
}

/// Resolve the stable app identity baked into a compiled binary: an explicit
/// `--app-name`, otherwise the output file name (minus the executable extension
/// added by Ende). The derived default is held to the same rules as an explicit
/// flag, since it becomes a single directory component at runtime (possibly on
/// a different target OS when cross-compiling); otherwise an output name like
/// `aux` or one with a trailing dot would silently break persistent storage on
/// the target.
fn resolve_app_name(
  compile_flags: &CompileFlags,
  display_output_filename: &str,
  is_desktop: bool,
) -> Result<String, AnyError> {
  let app_name = compile_flags
    .app_name
    .clone()
    .unwrap_or_else(|| default_app_name(display_output_filename, is_desktop));
  validate_app_name(&app_name)?;
  Ok(app_name)
}

pub struct WriteBinOptions<'a> {
  pub writer: File,
  pub display_output_filename: &'a str,
  pub graph: &'a ModuleGraph,
  pub entrypoint: &'a ModuleSpecifier,
  pub include_paths: &'a [ModuleSpecifier],
  pub exclude_paths: Vec<PathBuf>,
  pub compile_flags: &'a CompileFlags,
}

pub struct EndeCompileBinaryWriter<'a> {
  cli_options: &'a CliOptions,
  ende_dir: &'a EndeDir,
  emitter: &'a CliEmitter,
  file_fetcher: &'a CliFileFetcher,
  http_client_provider: &'a HttpClientProvider,
  workspace_resolver: &'a WorkspaceResolver<CliSys>,
  is_desktop: bool,
}

impl<'a> EndeCompileBinaryWriter<'a> {
  pub fn new(
    cli_options: &'a CliOptions,
    ende_dir: &'a EndeDir,
    emitter: &'a CliEmitter,
    file_fetcher: &'a CliFileFetcher,
    http_client_provider: &'a HttpClientProvider,
    workspace_resolver: &'a WorkspaceResolver<CliSys>,
    is_desktop: bool,
  ) -> Self {
    Self {
      cli_options,
      ende_dir,
      emitter,
      file_fetcher,
      http_client_provider,
      workspace_resolver,
      is_desktop,
    }
  }

  pub async fn write_bin(
    &self,
    options: WriteBinOptions<'_>,
  ) -> Result<(), AnyError> {
    // Select base binary based on target
    let mut original_binary =
      self.get_base_binary(options.compile_flags).await?;

    if options.compile_flags.no_terminal {
      let target = resolve_compile_target(options.compile_flags);
      if !target.contains("windows") {
        bail!(
          "The `--no-terminal` flag is only available when targeting Windows (current: {})",
          target,
        )
      }
      set_windows_binary_to_gui(&mut original_binary)
        .context("Setting windows binary to GUI.")?;
    }
    if options.compile_flags.icon.is_some() {
      let target = resolve_compile_target(options.compile_flags);
      // Desktop builds handle icons during app bundle packaging.
      if !target.contains("windows") && !self.is_desktop {
        bail!(
          "The `--icon` flag is only available when targeting Windows (current: {})",
          target,
        );
      }
    }
    // Validate the resolved app name (explicit `--app-name` or the default
    // derived from the output file name) up front, so an invalid name fails
    // before we do any work to write the binary. The returned name is discarded
    // here; the value actually baked into the metadata is resolved again at the
    // write site below.
    resolve_app_name(
      options.compile_flags,
      options.display_output_filename,
      self.is_desktop,
    )?;
    self.write_standalone_binary(options, original_binary).await
  }

  async fn get_base_binary(
    &self,
    compile_flags: &CompileFlags,
  ) -> Result<Vec<u8>, AnyError> {
    if self.is_desktop {
      return self.get_desktop_base_binary(compile_flags).await;
    }

    // Used for testing.
    //
    // Phase 2 of the 'min sized' ende compile RFC talks
    // about adding this as a flag.
    if let Some(path) = get_dev_binary_path() {
      if compile_flags.engine == JavaScriptEngine::QuickJs {
        log::warn!(
          "--engine quickjs is ignored when using the development endert at {}",
          path.to_string_lossy()
        );
      }
      log::debug!("Resolved endert: {}", path.to_string_lossy());
      return std::fs::read(&path).with_context(|| {
        format!("Could not find endert at '{}'", path.to_string_lossy())
      });
    }

    let target = resolve_compile_target(compile_flags);
    let binary_name =
      runtime_archive_name("endert", &compile_flags.engine, &target);

    let binary_path_suffix = match ENDE_VERSION_INFO.release_channel {
      ReleaseChannel::Canary => {
        format!("canary/{}/{}", ENDE_VERSION_INFO.git_hash, binary_name)
      }
      _ => {
        format!("release/v{}/{}", ENDE_VERSION_INFO.ende, binary_name)
      }
    };

    let download_directory = self.ende_dir.dl_folder_path();
    let binary_path = download_directory.join(&binary_path_suffix);
    log::debug!("Resolved endert: {}", binary_path.display());

    let read_file = |path: &Path| -> Result<Vec<u8>, AnyError> {
      std::fs::read(path).with_context(|| format!("Reading {}", path.display()))
    };
    let archive_data = if binary_path.exists() {
      read_file(&binary_path)?
    } else {
      self
        .download_base_binary(&binary_path, &binary_path_suffix)
        .await
        .context("Setting up base binary.")?
    };
    let temp_dir = tempfile::TempDir::new()?;
    let base_binary_path = archive::unpack_into_dir(archive::UnpackArgs {
      exe_name: if target.contains("windows") {
        "endert.exe"
      } else {
        "endert"
      },
      archive_name: &binary_name,
      archive_data: &archive_data,
      dest_path: temp_dir.path(),
    })?;
    let base_binary = read_file(&base_binary_path)?;
    drop(temp_dir); // delete the temp dir
    Ok(base_binary)
  }

  async fn get_desktop_base_binary(
    &self,
    compile_flags: &CompileFlags,
  ) -> Result<Vec<u8>, AnyError> {
    // For development: check ENDERT_DESKTOP_BIN env var or look
    // for libendert next to the ende executable.
    if let Some(path) = get_dev_desktop_binary_path() {
      if compile_flags.engine == JavaScriptEngine::QuickJs {
        log::warn!(
          "--engine quickjs is ignored when using the development libendert at {}",
          path.to_string_lossy()
        );
      }
      log::debug!("Resolved libendert: {}", path.to_string_lossy());
      return std::fs::read(&path).with_context(|| {
        format!("Could not find libendert at '{}'", path.to_string_lossy())
      });
    }

    let target = resolve_compile_target(compile_flags);
    let lib_ext = if target.contains("darwin") {
      "dylib"
    } else if target.contains("windows") {
      "dll"
    } else {
      "so"
    };
    let lib_name = if target.contains("windows") {
      format!("endert.{lib_ext}")
    } else {
      format!("libendert.{lib_ext}")
    };
    let binary_name =
      runtime_archive_name("libendert", &compile_flags.engine, &target);

    let binary_path_suffix = match ENDE_VERSION_INFO.release_channel {
      ReleaseChannel::Canary => {
        format!("canary/{}/{}", ENDE_VERSION_INFO.git_hash, binary_name)
      }
      _ => {
        format!("release/v{}/{}", ENDE_VERSION_INFO.ende, binary_name)
      }
    };

    let download_directory = self.ende_dir.dl_folder_path();
    let binary_path = download_directory.join(&binary_path_suffix);
    log::debug!("Resolved libendert: {}", binary_path.display());

    let read_file = |path: &Path| -> Result<Vec<u8>, AnyError> {
      std::fs::read(path).with_context(|| format!("Reading {}", path.display()))
    };
    let archive_data = if binary_path.exists() {
      read_file(&binary_path)?
    } else {
      self
        .download_base_binary(&binary_path, &binary_path_suffix)
        .await
        .context("Setting up desktop base binary.")?
    };
    let temp_dir = tempfile::TempDir::new()?;
    let base_binary_path = archive::unpack_into_dir(archive::UnpackArgs {
      exe_name: &lib_name,
      archive_name: &binary_name,
      archive_data: &archive_data,
      dest_path: temp_dir.path(),
    })?;
    let base_binary = read_file(&base_binary_path)?;
    drop(temp_dir);
    Ok(base_binary)
  }

  async fn download_base_binary(
    &self,
    output_path: &Path,
    binary_path_suffix: &str,
  ) -> Result<Vec<u8>, AnyError> {
    let download_url = runtime_download_url(binary_path_suffix);
    let response = {
      let progress_bars = ProgressBar::new(ProgressBarStyle::DownloadBars);
      let progress = progress_bars.update(&download_url);

      self
        .http_client_provider
        .get_or_create()?
        .download_with_progress_and_retries(
          download_url.parse()?,
          &Default::default(),
          &progress,
        )
        .await?
    };
    let bytes = response
      .into_bytes()
      .with_context(|| format!("Failed downloading '{}'", download_url))?;

    let create_dir_all = |dir: &Path| {
      std::fs::create_dir_all(dir)
        .with_context(|| format!("Creating {}", dir.display()))
    };
    create_dir_all(output_path.parent().unwrap())?;
    atomic_write_file_with_retries(
      &CliSys::default(),
      output_path,
      &bytes,
      CACHE_PERM,
    )
    .with_context(|| format!("Writing {}", output_path.display()))?;
    Ok(bytes)
  }

  /// This functions creates a standalone ende binary by appending a bundle
  /// and magic trailer to the currently executing binary.
  async fn write_standalone_binary(
    &self,
    options: WriteBinOptions<'_>,
    original_bin: Vec<u8>,
  ) -> Result<(), AnyError> {
    let WriteBinOptions {
      writer,
      display_output_filename,
      graph,
      entrypoint,
      include_paths,
      exclude_paths,
      compile_flags,
    } = options;
    let ca_data = match self.cli_options.ca_data() {
      Some(CaData::File(ca_file)) => Some(
        std::fs::read(ca_file).with_context(|| format!("Reading {ca_file}"))?,
      ),
      Some(CaData::Bytes(bytes)) => Some(bytes.clone()),
      None => None,
    };
    let mut vfs = VfsBuilder::new();
    for path in exclude_paths {
      vfs.add_exclude_path(path);
    }
    let progress_bar = ProgressBar::new(ProgressBarStyle::ProgressBars);
    for include_file in include_paths {
      let path = deno_path_util::url_to_file_path(include_file)?;
      vfs
        .add_path(&path)
        .with_context(|| format!("Including {}", path.display()))?;
    }
    let specifiers_count = graph.specifiers_count();
    let mut specifier_store = SpecifierStore::with_capacity(specifiers_count);
    let mut remote_modules_store =
      SpecifierDataStore::with_capacity(specifiers_count);
    let mut asset_module_urls = graph.asset_module_urls();
    let progress =
      progress_bar.update_with_prompt(ProgressMessagePrompt::Compile, "");
    progress.set_total_size(specifiers_count as u64);
    let mut modules_done: u64 = 0;
    // todo(dsherret): transpile in parallel
    for module in graph.modules() {
      if module.specifier().scheme() == "data" {
        continue; // don't store data urls as an entry as they're in the code
      }
      let mut maybe_source_map = None;
      let mut maybe_transpiled = None;
      let (maybe_original_source, media_type) = match module {
        deno_graph::Module::Js(m) => {
          let specifier = &m.specifier;
          let original_bytes = match m.source.try_get_original_bytes() {
            Some(bytes) => bytes,
            None => self.load_asset_bypass_permissions(specifier).await?.source,
          };
          if m.media_type.is_emittable() {
            let (source, source_map) =
              self.emitter.emit_source_for_ende_compile(
                &m.specifier,
                m.media_type,
                ModuleKind::Esm,
                &m.source.text,
              )?;
            if source != m.source.text.as_ref() {
              maybe_source_map = Some(source_map.into_bytes());
              maybe_transpiled = Some(source.into_bytes());
            }
          }
          (Some(original_bytes), m.media_type)
        }
        deno_graph::Module::Json(m) => {
          let original_bytes = match m.source.try_get_original_bytes() {
            Some(bytes) => bytes,
            None => {
              self
                .load_asset_bypass_permissions(&m.specifier)
                .await?
                .source
            }
          };
          (Some(original_bytes), m.media_type)
        }
        deno_graph::Module::Wasm(m) => {
          (Some(m.source.clone()), MediaType::Wasm)
        }
        deno_graph::Module::Npm(_)
        | deno_graph::Module::Node(_)
        | deno_graph::Module::External(_) => (None, MediaType::Unknown),
      };
      if let Some(original_source) = maybe_original_source {
        asset_module_urls.swap_remove(module.specifier());
        if module.specifier().scheme() == "file" {
          let file_path = deno_path_util::url_to_file_path(module.specifier())?;
          vfs
            .add_file_with_data(
              &file_path,
              ende_lib::standalone::virtual_fs::AddFileDataOptions {
                data: original_source.to_vec(),
                maybe_transpiled,
                maybe_source_map,
                mtime: file_path
                  .metadata()
                  .ok()
                  .and_then(|m| m.modified().ok()),
              },
            )
            .with_context(|| {
              format!("Failed adding '{}'", file_path.display())
            })?;
        } else {
          let specifier_id = specifier_store.get_or_add(module.specifier());
          remote_modules_store.add(
            specifier_id,
            RemoteModuleEntry {
              media_type,
              is_valid_utf8: is_valid_utf8(&original_source),
              data: Cow::Owned(original_source.to_vec()),
              maybe_transpiled: maybe_transpiled.map(Cow::Owned),
              maybe_source_map: maybe_source_map.map(Cow::Owned),
            },
          );
        }
      }
      modules_done += 1;
      progress.set_position(modules_done);
    }
    drop(progress);

    for url in asset_module_urls {
      if graph.try_get(url).is_err() {
        // skip because there was an error loading this module
        continue;
      }
      match url.scheme() {
        "file" => {
          let file_path = deno_path_util::url_to_file_path(url)?;
          vfs.add_path(&file_path)?;
        }
        "http" | "https" => {
          let specifier_id = specifier_store.get_or_add(url);
          if !remote_modules_store.contains(specifier_id) {
            // it's ok to bypass permissions here because we verified the module
            // loaded successfully in the graph
            let file = self.load_asset_bypass_permissions(url).await?;
            remote_modules_store.add(
              specifier_id,
              RemoteModuleEntry {
                media_type: MediaType::from_specifier_and_headers(
                  &file.url,
                  file.maybe_headers.as_ref(),
                ),
                is_valid_utf8: is_valid_utf8(&file.source),
                data: Cow::Owned(file.source.to_vec()),
                maybe_source_map: None,
                maybe_transpiled: None,
              },
            );
          }
        }
        _ => {}
      }
    }

    let mut redirects_store =
      SpecifierDataStore::with_capacity(graph.redirects.len());
    for (from, to) in &graph.redirects {
      redirects_store.add(
        specifier_store.get_or_add(from),
        specifier_store.get_or_add(to),
      );
    }

    if let Some(import_map) = self.workspace_resolver.maybe_import_map()
      && let Ok(file_path) = url_to_file_path(import_map.base_url())
      && let Some(import_map_parent_dir) = file_path.parent()
    {
      // tell the vfs about the import map's parent directory in case it
      // falls outside what the root of where the VFS will be based
      vfs.add_possible_min_root_dir(import_map_parent_dir);
    }
    let vfs = vfs.build();

    let root_dir_url = match &vfs.root_path {
      WindowsSystemRootablePath::Path(dir) => {
        Some(url_from_directory_path(dir)?)
      }
      WindowsSystemRootablePath::WindowSystemRoot => None,
    };
    let root_dir_url = match &root_dir_url {
      Some(url) => StandaloneRelativeFileBaseUrl::Path(url),
      None => StandaloneRelativeFileBaseUrl::WindowsSystemRoot,
    };

    let code_cache_key = if self.cli_options.code_cache_enabled() {
      let mut hasher = FastInsecureHasher::new_ende_versioned();
      for module in graph.modules() {
        if let Some(source) = module.source() {
          hasher
            .write(root_dir_url.specifier_key(module.specifier()).as_bytes());
          hasher.write(source.as_bytes());
        }
      }
      Some(hasher.finish())
    } else {
      None
    };

    let env_vars_from_env_file = {
      let mut aggregated_env_vars = IndexMap::new();
      for env_file_name in self.cli_options.env_file_names().rev() {
        match ende_dotenv::find_path_and_content(
          &CliSys::default(),
          self.cli_options.initial_cwd(),
          env_file_name,
        ) {
          Ok(Some((env_file_path, content))) => {
            match get_file_env_vars(&content) {
              Ok(env_vars) => {
                aggregated_env_vars.extend(env_vars);
                log::info!(
                  "{} Environment variables from the file \"{}\" were embedded in the generated executable file",
                  crate::colors::yellow("Warning"),
                  env_file_path.display()
                );
              }
              Err(e) => {
                handle_dotenv_error(
                  &e,
                  &env_file_path,
                  self.cli_options.log_level(),
                );
              }
            };
          }
          Ok(None) => {
            handle_dotenv_not_found(
              env_file_name,
              self.cli_options.log_level(),
            );
          }
          Err(e) => {
            handle_dotenv_io_error(&e, self.cli_options.log_level());
          }
        };
      }
      aggregated_env_vars
    };

    output_vfs(&vfs, display_output_filename);

    let preload_modules = self
      .cli_options
      .preload_modules()?
      .into_iter()
      .map(|s| root_dir_url.specifier_key(&s).into_owned())
      .collect::<Vec<_>>();

    let metadata = Metadata {
      argv: compile_flags.args.clone(),
      seed: self.cli_options.seed(),
      code_cache_key,
      location: self.cli_options.location_flag().clone(),
      permissions: self.cli_options.permissions_options()?,
      v8_flags: construct_v8_flags(
        &get_default_v8_flags(),
        self.cli_options.v8_flags(),
        vec![],
      ),
      unsafely_ignore_certificate_errors: self
        .cli_options
        .unsafely_ignore_certificate_errors()
        .clone(),
      log_level: self.cli_options.log_level(),
      ca_stores: self.cli_options.ca_stores().clone(),
      ca_data,
      env_vars_from_env_file,
      entrypoint_key: root_dir_url.specifier_key(entrypoint).into_owned(),
      preload_modules,
      workspace_resolver: SerializedWorkspaceResolver {
        import_map: self.workspace_resolver.maybe_import_map().map(|i| {
          SerializedWorkspaceResolverImportMap {
            specifier: if i.base_url().scheme() == "file" {
              root_dir_url.specifier_key(i.base_url()).into_owned()
            } else {
              // just make a remote url local
              ende_config::ende_json::DEFAULT_CONFIG_FILE_NAME.to_string()
            },
            json: i.to_json(),
          }
        }),
        jsr_pkgs: self
          .workspace_resolver
          .jsr_packages()
          .iter()
          .map(|pkg| SerializedResolverWorkspaceJsrPackage {
            relative_base: root_dir_url.specifier_key(&pkg.base).into_owned(),
            name: pkg.name.clone(),
            version: pkg.version.clone(),
            exports: pkg.exports.clone(),
          })
          .collect(),
      },
      unstable_config: UnstableConfig {
        legacy_flag_enabled: false,
        features: self
          .cli_options
          .unstable_features()
          .into_iter()
          .map(|s| s.to_string())
          .collect(),
        lazy_dynamic_imports: self.cli_options.unstable_lazy_dynamic_imports(),
        raw_imports: self.cli_options.unstable_raw_imports(),
        sloppy_imports: self.cli_options.unstable_sloppy_imports(),
        tsgo: self.cli_options.unstable_tsgo(),
      },
      otel_config: self.cli_options.otel_config(),
      vfs_case_sensitivity: vfs.case_sensitivity,
      self_extracting: if compile_flags.self_extracting {
        let mut hasher = FastInsecureHasher::new_ende_versioned();
        for file in &vfs.files {
          hasher.write_u64(file.len() as u64);
          hasher.write(file);
        }
        Some(format!("{:016x}", hasher.finish()))
      } else {
        None
      },
      // Bake in a stable app identity so origin-bound storage
      // (`localStorage`, `caches`) persists to a per-app
      // directory at runtime. Prefer an explicit `--app-name`, otherwise derive
      // it from the output file name (minus any executable extension Ende adds).
      // Resolving here keeps the identity stable even if the binary is later
      // renamed. The name is already validated in `write_bin` (via
      // `resolve_app_name`).
      app_name: Some(resolve_app_name(
        compile_flags,
        display_output_filename,
        self.is_desktop,
      )?),
      app_version: self
        .cli_options
        .workspace()
        .root_ende_json()
        .and_then(|c| c.json.version.clone()),
      error_reporting_url: self
        .cli_options
        .start_dir
        .to_desktop_config()
        .ok()
        .and_then(|c| c.error_reporting.as_ref()?.url.clone()),
      release_base_url: self
        .cli_options
        .start_dir
        .to_desktop_config()
        .ok()
        .and_then(|c| c.release.as_ref()?.base_url.clone()),
    };

    let (data_section_bytes, section_sizes) = serialize_binary_data_section(
      &metadata,
      &specifier_store.for_serialization(&root_dir_url),
      &redirects_store,
      &remote_modules_store,
      &vfs,
    )
    .context("Serializing binary data section.")?;

    log::info!(
      "\n{} {}",
      crate::colors::bold("Files:"),
      crate::util::display::human_size(section_sizes.vfs as f64)
    );
    log::info!(
      "{} {}",
      crate::colors::bold("Metadata:"),
      crate::util::display::human_size(section_sizes.metadata as f64)
    );
    log::info!(
      "{} {}\n",
      crate::colors::bold("Remote modules:"),
      crate::util::display::human_size(section_sizes.remote_modules as f64)
    );

    write_binary_bytes(writer, original_bin, data_section_bytes, compile_flags)
      .context("Writing binary bytes")
  }

  async fn load_asset_bypass_permissions(
    &self,
    specifier: &ModuleSpecifier,
  ) -> Result<
    ende_cache_dir::file_fetcher::File,
    ende_resolver::file_fetcher::FetchError,
  > {
    self
      .file_fetcher
      .fetch_with_options(
        specifier,
        FetchPermissionsOptionRef::AllowAll,
        FetchOptions {
          local: FetchLocalOptions {
            include_mtime: false,
          },
          maybe_auth: None,
          maybe_accept: None,
          maybe_cache_setting: Some(
            &ende_cache_dir::file_fetcher::CacheSetting::Use,
          ),
        },
      )
      .await
  }
}

#[allow(clippy::too_many_arguments, reason = "private code")]
fn write_binary_bytes(
  mut file_writer: File,
  original_bin: Vec<u8>,
  data_section_bytes: Vec<u8>,
  compile_flags: &CompileFlags,
) -> Result<(), AnyError> {
  let target = resolve_compile_target(compile_flags);
  if target.contains("linux") {
    libsui::Elf::new(&original_bin).append(
      "d3n0l4nd",
      &data_section_bytes,
      &mut file_writer,
    )?;
  } else if target.contains("windows") {
    let mut pe = libsui::PortableExecutable::from(&original_bin)?;
    if let Some(icon) = compile_flags.icon.as_ref() {
      let icon = std::fs::read(icon)?;
      pe = pe.set_icon(&icon)?;
    }

    pe.write_resource("d3n0l4nd", data_section_bytes)?
      .build(&mut file_writer)?;
  } else if target.contains("darwin") {
    libsui::Macho::from(original_bin)?
      .write_section("d3n0l4nd", data_section_bytes)?
      .build_and_sign(&mut file_writer)?;
  }
  Ok(())
}

struct BinaryDataSectionSizes {
  metadata: usize,
  remote_modules: usize,
  vfs: usize,
}

/// Binary format:
/// * d3n0l4nd
/// * <metadata_len><metadata>
/// * <specifiers>
/// * <redirects>
/// * <remote_modules>
/// * <vfs_headers_len><vfs_headers>
/// * <vfs_file_data_len><vfs_file_data>
/// * d3n0l4nd
#[allow(clippy::too_many_arguments, reason = "private code")]
fn serialize_binary_data_section(
  metadata: &Metadata,
  specifiers: &SpecifierStoreForSerialization,
  redirects: &SpecifierDataStore<SpecifierId>,
  remote_modules: &SpecifierDataStore<RemoteModuleEntry<'_>>,
  vfs: &BuiltVfs,
) -> Result<(Vec<u8>, BinaryDataSectionSizes), AnyError> {
  let metadata = serde_json::to_string(metadata)?;
  let serialized_vfs = serde_json::to_string(&vfs.entries)?;

  let remote_modules_len = Cell::new(0);
  let metadata_len = Cell::new(0);
  let vfs_len = Cell::new(0);

  let bytes = capacity_builder::BytesBuilder::build(|builder| {
    builder.append(MAGIC_BYTES);
    // 1. Metadata
    {
      builder.append_le(metadata.len() as u64);
      builder.append(&metadata);
    }
    metadata_len.set(builder.len());
    // 2. Specifiers
    builder.append(specifiers);
    // 3. Redirects
    redirects.serialize(builder);
    // 4. Remote modules
    remote_modules.serialize(builder);
    remote_modules_len.set(builder.len() - metadata_len.get());
    // 5. VFS
    {
      builder.append_le(serialized_vfs.len() as u64);
      builder.append(&serialized_vfs);
      let vfs_bytes_len = vfs.files.iter().map(|f| f.len() as u64).sum::<u64>();
      builder.append_le(vfs_bytes_len);
      for file in &vfs.files {
        builder.append(file);
      }
    }
    vfs_len.set(builder.len() - remote_modules_len.get());

    // write the magic bytes at the end so we can use it
    // to make sure we've deserialized correctly
    builder.append(MAGIC_BYTES);
  })?;

  Ok((
    bytes,
    BinaryDataSectionSizes {
      metadata: metadata_len.get(),
      remote_modules: remote_modules_len.get(),
      vfs: vfs_len.get(),
    },
  ))
}

fn get_endert_path(ende_exe: PathBuf) -> Option<OsString> {
  let mut endert = ende_exe;
  endert.set_file_name(if cfg!(windows) {
    "endert.exe"
  } else {
    "endert"
  });
  endert.exists().then(|| endert.into_os_string())
}

fn runtime_download_url(binary_path_suffix: &str) -> String {
  format!("https://dl.ende.sh/{binary_path_suffix}")
}

fn runtime_archive_name(
  prefix: &str,
  engine: &JavaScriptEngine,
  target: &str,
) -> String {
  format!("{prefix}{}-{target}.zip", engine.artifact_suffix())
}

/// Reads a development override env var, falling back to its deprecated
/// pre-rename name with a warning.
fn dev_env_var_os(name: &str, legacy_name: &str) -> Option<OsString> {
  env::var_os(name).or_else(|| {
    let value = env::var_os(legacy_name)?;
    log::warn!(
      "{} {legacy_name} is deprecated. Use {name} instead.",
      crate::colors::yellow("Warning")
    );
    Some(value)
  })
}

fn get_dev_binary_path() -> Option<OsString> {
  dev_env_var_os("ENDERT_BIN", "DENORT_BIN").or_else(|| {
    env::current_exe().ok().and_then(|exec_path| {
      if exec_path
        .components()
        .any(|component| component == Component::Normal("target".as_ref()))
      {
        get_endert_path(exec_path)
      } else {
        None
      }
    })
  })
}

fn get_libendert_path(ende_exe: PathBuf) -> Option<OsString> {
  let mut libendert = ende_exe;
  if cfg!(target_os = "macos") {
    libendert.set_file_name("libendert.dylib");
  } else if cfg!(windows) {
    libendert.set_file_name("endert.dll");
  } else {
    libendert.set_file_name("libendert.so");
  }
  libendert.exists().then(|| libendert.into_os_string())
}

fn get_dev_desktop_binary_path() -> Option<OsString> {
  dev_env_var_os("ENDERT_DESKTOP_BIN", "DENORT_DESKTOP_BIN").or_else(|| {
    env::current_exe().ok().and_then(|exec_path| {
      if exec_path
        .components()
        .any(|component| component == Component::Normal("target".as_ref()))
      {
        // Prefer release libendert (optimized) over debug.
        let target_dir = exec_path.parent().and_then(|p| p.parent());
        target_dir
          .and_then(|d| {
            get_libendert_path(d.join("release").join("libendert.dylib"))
          })
          .or_else(|| get_libendert_path(exec_path.clone()))
      } else {
        None
      }
    })
  })
}

/// This function returns the environment variables specified
/// in the passed environment file.
fn get_file_env_vars(
  content: &str,
) -> Result<IndexMap<String, String>, ende_dotenv::ParseError> {
  let mut file_env_vars = IndexMap::new();
  for item in ende_dotenv::from_content_sanitized_iter_with_substitution(
    &CliSys::default(),
    content,
  )? {
    let Ok((key, val)) = item else {
      continue; // this failure will be warned about on load
    };
    file_env_vars.insert(key, val);
  }
  Ok(file_env_vars)
}

/// This function sets the subsystem field in the PE header to 2 (GUI subsystem)
/// For more information about the PE header: https://learn.microsoft.com/en-us/windows/win32/debug/pe-format
fn set_windows_binary_to_gui(bin: &mut [u8]) -> Result<(), AnyError> {
  // Get the PE header offset located in an i32 found at offset 60
  // See: https://learn.microsoft.com/en-us/windows/win32/debug/pe-format#ms-dos-stub-image-only
  let start_pe = u32::from_le_bytes((bin[60..64]).try_into()?);

  // Get image type (PE32 or PE32+) indicates whether the binary is 32 or 64 bit
  // The used offset and size values can be found here:
  // https://learn.microsoft.com/en-us/windows/win32/debug/pe-format#optional-header-image-only
  let start_32 = start_pe as usize + 28;
  let magic_32 =
    u16::from_le_bytes(bin[(start_32)..(start_32 + 2)].try_into()?);

  let start_64 = start_pe as usize + 24;
  let magic_64 =
    u16::from_le_bytes(bin[(start_64)..(start_64 + 2)].try_into()?);

  // Take the standard fields size for the current architecture (32 or 64 bit)
  // This is the ofset for the Windows-Specific fields
  let standard_fields_size = if magic_32 == 0x10b {
    28
  } else if magic_64 == 0x20b {
    24
  } else {
    bail!("Could not find a matching magic field in the PE header")
  };

  // Set the subsystem field (offset 68) to 2 (GUI subsystem)
  // For all possible options, see: https://learn.microsoft.com/en-us/windows/win32/debug/pe-format#optional-header-windows-specific-fields-image-only
  let subsystem_offset = 68;
  let subsystem_start =
    start_pe as usize + standard_fields_size + subsystem_offset;
  let subsystem: u16 = 2;
  bin[(subsystem_start)..(subsystem_start + 2)]
    .copy_from_slice(&subsystem.to_le_bytes());
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::default_app_name;
  use super::runtime_archive_name;
  use super::runtime_download_url;
  use crate::args::JavaScriptEngine;

  #[test]
  fn runtime_archive_names_include_engine_suffix() {
    let target = "aarch64-apple-darwin";
    assert_eq!(
      runtime_archive_name("endert", &JavaScriptEngine::V8, target),
      "endert-aarch64-apple-darwin.zip"
    );
    assert_eq!(
      runtime_archive_name("endert", &JavaScriptEngine::QuickJs, target),
      "endert-quickjs-aarch64-apple-darwin.zip"
    );
    assert_eq!(
      runtime_archive_name("libendert", &JavaScriptEngine::QuickJs, target),
      "libendert-quickjs-aarch64-apple-darwin.zip"
    );
  }

  #[test]
  fn runtime_download_url_uses_ende_host() {
    assert_eq!(
      runtime_download_url("release/v2.0.0/endert-aarch64-apple-darwin.zip"),
      "https://dl.ende.sh/release/v2.0.0/endert-aarch64-apple-darwin.zip"
    );
  }

  #[test]
  fn default_app_name_strips_only_ende_added_extensions() {
    assert_eq!(default_app_name("speedgraph.exe", false), "speedgraph");
    assert_eq!(default_app_name("speedgraph.so", false), "speedgraph.so");

    assert_eq!(default_app_name("speedgraph.dll", true), "speedgraph");
    assert_eq!(default_app_name("speedgraph.dylib", true), "speedgraph");
    assert_eq!(default_app_name("speedgraph.so", true), "speedgraph");
    assert_eq!(default_app_name("speed.graph.so", true), "speed.graph");
  }
}
