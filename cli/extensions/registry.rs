// Copyright 2018-2026 the Ende authors. MIT license.

//! Loading the extensions an app declares in ende.json.
//!
//! An app imports `extension:@scope/name` and gets a generated proxy module
//! whose exports call into the extension's own worker (`runtime/js/11_workers.js`).
//! Preparing an extension fetches its static module graph, reads its
//! manifest and works out its permissions; the worker is then started from
//! that, by name only (`op_create_extension_worker`).

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno_error::JsErrorBox;
use deno_graph::ModuleGraph;
use ende_config::ende_json::ExtensionConfig;
use ende_config::ende_json::ExtensionsConfig;
use ende_core::ModuleSpecifier;
use ende_core::error::AnyError;
use ende_core::parking_lot::Mutex;
use ende_core::serde_json;
use ende_lockfile::ExtensionLockEntry;
use ende_runtime::ende_permissions::Permissions;
use ende_runtime::ende_permissions::PermissionsContainer;
use ende_runtime::ende_permissions::PermissionsOptions;
use ende_runtime::ende_permissions::RuntimePermissionDescriptorParser;
use ende_runtime::ops::worker_host::ExtensionWorkerProvider;
use ende_runtime::ops::worker_host::ExtensionWorkerSpec;

use super::extension_permissions_options;
use super::manifest::ExtensionManifest;
use super::manifest::extract_manifest;
use crate::args::CliLockfile;
use crate::module_loader::ModuleLoadPreparer;
use crate::resolver::CliResolver;
use crate::sys::CliSys;

/// The scheme of an import that names a declared extension.
pub const EXTENSION_SCHEME: &str = "extension";

/// An extension whose graph is fetched and whose manifest was read.
struct PreparedExtension {
  /// The extension's entry module, as declared.
  source: ModuleSpecifier,
  /// Its exported functions, which the proxy and the worker expose.
  exports: Vec<String>,
  /// Its effective permissions (grant ∩ manifest).
  options: PermissionsOptions,
  /// The static module graph its manifest was read from. The worker runs
  /// exactly this code: it starts from this graph instead of fetching the
  /// extension again, so a source changed on disk after the manifest was
  /// read can't take effect.
  graph: Arc<ModuleGraph>,
}

/// The extensions an app declares, and the ones loaded so far.
pub struct ExtensionRegistry {
  config: Result<ExtensionsConfig, String>,
  module_load_preparer: Arc<ModuleLoadPreparer>,
  resolver: Arc<CliResolver>,
  desc_parser: Arc<RuntimePermissionDescriptorParser<CliSys>>,
  /// The folder of the root ende.json, which declares the extensions.
  project_root: PathBuf,
  /// Pins each extension's source, manifest and grant once approved.
  lockfile: Option<Arc<CliLockfile>>,
  /// `--no-lock` was given on the command line: extensions run unpinned,
  /// with a warning. Without it, a missing lockfile (`"lock": false` in the
  /// config) is an error.
  no_lock_flag: bool,
  prepared: Mutex<HashMap<String, Arc<PreparedExtension>>>,
}

/// An extension whose graph was fetched and manifest read, and what the
/// lockfile would pin about it.
struct LoadedExtension {
  prepared: Arc<PreparedExtension>,
  pin: ExtensionLockEntry,
  warning: Option<String>,
}

impl ExtensionRegistry {
  pub fn new(
    config: Result<ExtensionsConfig, String>,
    module_load_preparer: Arc<ModuleLoadPreparer>,
    resolver: Arc<CliResolver>,
    desc_parser: Arc<RuntimePermissionDescriptorParser<CliSys>>,
    project_root: PathBuf,
    lockfile: Option<Arc<CliLockfile>>,
    no_lock_flag: bool,
  ) -> Self {
    // Pins of extensions no longer declared go on the next lockfile write.
    if let (Ok(config), Some(lockfile)) = (&config, &lockfile) {
      lockfile
        .lock()
        .retain_extensions(|name| config.extensions.contains_key(name));
    }
    Self {
      config,
      module_load_preparer,
      resolver,
      desc_parser,
      project_root,
      lockfile,
      no_lock_flag,
      prepared: Default::default(),
    }
  }

  /// The static graph extension `name` was prepared from, for its worker.
  pub fn prepared_graph(&self, name: &str) -> Option<Arc<ModuleGraph>> {
    self
      .prepared
      .lock()
      .get(name)
      .map(|prepared| prepared.graph.clone())
  }

  /// The declared source of extension `name`.
  pub fn source(&self, name: &str) -> Result<ModuleSpecifier, JsErrorBox> {
    Ok(self.declared(name)?.source.clone())
  }

  /// Errors unless `raw` (the extension's source URL, imported from
  /// `referrer`) resolves to itself. The app's import map applies to every
  /// import, absolute URLs included, so it could otherwise swap the module
  /// that runs for one whose manifest was never read.
  pub fn check_source_resolution(
    &self,
    name: &str,
    source: &ModuleSpecifier,
    referrer: &ModuleSpecifier,
  ) -> Result<(), JsErrorBox> {
    let resolved = self
      .resolver
      .resolve(
        source.as_str(),
        referrer,
        deno_graph::Position::zeroed(),
        ende_resolver::workspace::ResolutionKind::Execution,
      )
      .map_err(JsErrorBox::from_err)?;
    if &resolved != source {
      return Err(JsErrorBox::type_error(format!(
        "Extension \"{name}\": the app's import map maps its source \"{source}\" to \"{resolved}\". Extensions resolve without the app's import map: remove that entry"
      )));
    }
    Ok(())
  }

  /// The configuration of the declared extension `name`, or an error naming
  /// the declared ones.
  pub fn declared(&self, name: &str) -> Result<&ExtensionConfig, JsErrorBox> {
    let config = self
      .config
      .as_ref()
      .map_err(|err| JsErrorBox::type_error(err.clone()))?;
    config.extensions.get(name).ok_or_else(|| {
      let declared = config
        .extensions
        .keys()
        .map(|name| name.as_str())
        .collect::<Vec<_>>();
      JsErrorBox::type_error(if declared.is_empty() {
        format!(
          "Unknown extension \"{name}\". ende.json declares no extensions"
        )
      } else {
        format!(
          "Unknown extension \"{name}\". ende.json declares {}",
          declared.join(", ")
        )
      })
    })
  }

  /// The proxy module the app gets for `import … from "extension:<name>"`.
  /// The first one fetches the extension's graph, reads its manifest and
  /// prints the warning for permissions it asks for but isn't granted.
  pub async fn proxy_module(&self, name: &str) -> Result<String, JsErrorBox> {
    let prepared = self.prepare(name).await?;
    Ok(proxy_module_code(name, &prepared.exports))
  }

  async fn prepare(
    &self,
    name: &str,
  ) -> Result<Arc<PreparedExtension>, JsErrorBox> {
    if let Some(prepared) = self.prepared.lock().get(name) {
      return Ok(prepared.clone());
    }
    let loaded = self.load(name).await?;
    self.check_pin(name, &loaded.pin, &loaded.prepared.options)?;
    let mut map = self.prepared.lock();
    if let Some(existing) = map.get(name) {
      return Ok(existing.clone());
    }
    map.insert(name.to_string(), loaded.prepared.clone());
    drop(map);
    if let Some(warning) = loaded.warning {
      log::warn!("{} {}", crate::colors::yellow("Warning"), warning);
    }
    Ok(loaded.prepared)
  }

  /// Fetches the graph of extension `name`, reads its manifest and works out
  /// its permissions and what the lockfile pins about it, without checking
  /// the lockfile.
  async fn load(&self, name: &str) -> Result<LoadedExtension, JsErrorBox> {
    let config = self.declared(name)?.clone();
    check_extension_directory(name, &config.source, &self.project_root)?;
    self.check_source_resolution(name, &config.source, &config.source)?;
    let mut graph = ModuleGraph::new(deno_graph::GraphKind::CodeOnly);
    self
      .module_load_preparer
      .build_extension_graph(
        &mut graph,
        config.source.clone(),
        self.loader_permissions(&config.source)?,
      )
      .await
      .map_err(|err| {
        JsErrorBox::type_error(format!("Extension \"{name}\": {err}"))
      })?;
    check_extension_graph_resolution(name, &graph)?;
    let Some(deno_graph::Module::Js(module)) = graph.get(&config.source) else {
      return Err(JsErrorBox::type_error(format!(
        "Extension \"{name}\": {} is not a JavaScript or TypeScript module",
        config.source
      )));
    };
    let manifest = extract_manifest(
      name,
      &module.specifier,
      module.media_type,
      &module.source.text,
    )
    .map_err(JsErrorBox::from_err)?;
    let effective = extension_permissions_options(
      name,
      &config.permissions,
      &manifest,
      self.desc_parser.as_ref(),
    )
    .map_err(to_js_error)?;
    // Fail now rather than when the worker starts.
    Permissions::from_options(self.desc_parser.as_ref(), &effective.options)
      .map_err(to_js_error)?;
    let pin = extension_pin(name, &config, &manifest, &graph)?;
    Ok(LoadedExtension {
      prepared: Arc::new(PreparedExtension {
        source: config.source,
        exports: manifest.exports,
        options: effective.options,
        graph: Arc::new(graph),
      }),
      pin,
      warning: effective.warning,
    })
  }

  /// Errors if the lockfile approved extension `name` with a different
  /// source, manifest or grant. A first load records the pin and says so.
  fn check_pin(
    &self,
    name: &str,
    pin: &ExtensionLockEntry,
    options: &PermissionsOptions,
  ) -> Result<(), JsErrorBox> {
    let Some(lockfile) = &self.lockfile else {
      if self.no_lock_flag {
        // The user's own opt-out, so it runs; shown even with --quiet.
        #[allow(clippy::print_stderr, reason = "must show even with --quiet")]
        {
          eprintln!(
            "{} Extension \"{name}\" runs unpinned: --no-lock turned off the lockfile, so changes to its code, manifest or grant aren't checked.",
            crate::colors::yellow("Warning")
          );
        }
        return Ok(());
      }
      return Err(JsErrorBox::type_error(format!(
        "Extension \"{name}\" needs a lockfile, which pins its code and permissions, but the lockfile is turned off (\"lock\": false in the config). Turn it back on, or pass --no-lock to run extensions unpinned."
      )));
    };
    let approved = lockfile.lock().extension(name).cloned();
    match approved {
      Some(approved) if approved == *pin => Ok(()),
      Some(approved) => Err(JsErrorBox::type_error(format!(
        "Extension \"{name}\" changed since {} approved it: {}. Run `ende install` to review the change and approve it.",
        lockfile_name(lockfile),
        changed_parts(&approved, pin).join(", "),
      ))),
      None => {
        lockfile
          .lock()
          .insert_extension(name.to_string(), pin.clone());
        lockfile.write_if_changed().map_err(JsErrorBox::from_err)?;
        print_recorded_notice(name, lockfile, options);
        Ok(())
      }
    }
  }

  /// `ende install`: loads every declared extension, records new ones and
  /// asks before recording changes to approved ones. `yes` approves them
  /// without asking; without a terminal, changes need `yes`.
  pub async fn approve_changes(&self, yes: bool) -> Result<(), AnyError> {
    let Some(lockfile) = &self.lockfile else {
      return Ok(());
    };
    let names = match &self.config {
      Ok(config) => config.extensions.keys().cloned().collect::<Vec<_>>(),
      Err(err) => return Err(JsErrorBox::type_error(err.clone()).into()),
    };
    let mut changed = Vec::new();
    let mut recorded = Vec::new();
    for name in names {
      let loaded = self.load(&name).await?;
      let approved = lockfile.lock().extension(&name).cloned();
      match approved {
        Some(approved) if approved == loaded.pin => {}
        Some(approved) => changed.push((name, approved, loaded)),
        None => {
          lockfile
            .lock()
            .insert_extension(name.clone(), loaded.pin.clone());
          recorded.push((name, loaded));
        }
      }
    }
    if changed.is_empty() {
      return write_and_report(lockfile, &[], &recorded);
    }
    for (name, approved, loaded) in &changed {
      log::info!(
        "{}",
        describe_change(name, approved, &loaded.pin, &loaded.prepared.options)
      );
    }
    let approve = yes
      || match crate::util::console::confirm(
        crate::util::console::ConfirmOptions {
          message: "Approve these extension changes?".to_string(),
          default: false,
        },
      ) {
        Some(answer) => answer,
        None => {
          return Err(
            JsErrorBox::type_error(format!(
              "Extension changes need approval: run `ende install` in a terminal to review them, or `ende install --yes` to approve them. {} was not changed.",
              lockfile_name(lockfile),
            ))
            .into(),
          );
        }
      };
    if !approve {
      return Err(
        JsErrorBox::type_error(format!(
          "Extension changes not approved; {} was not changed.",
          lockfile_name(lockfile),
        ))
        .into(),
      );
    }
    let mut lock = lockfile.lock();
    for (name, _, loaded) in &changed {
      lock.insert_extension(name.clone(), loaded.pin.clone());
    }
    drop(lock);
    let approved = changed
      .iter()
      .map(|(name, _, _)| name.clone())
      .collect::<Vec<_>>();
    write_and_report(lockfile, &approved, &recorded)
  }

  /// What an extension's static module graph is fetched with: remote
  /// imports, and reading files only inside the extension's own directory.
  ///
  /// The app declared the extension's source in ende.json (and the lockfile
  /// pins what it fetched), so its static graph doesn't need the app's
  /// `--allow-import`. Its static imports are checked like an ordinary
  /// worker's, so a `file:` import (code, JSON, text or bytes) needs read
  /// access: a local extension may import files in its directory (resolved
  /// through symlinks), a remote or `jsr:` one no local files at all. What
  /// the extension does once it runs, including a dynamic `import()`, is
  /// checked against its own permissions.
  fn loader_permissions(
    &self,
    source: &ModuleSpecifier,
  ) -> Result<PermissionsContainer, JsErrorBox> {
    extension_loader_permissions(self.desc_parser.clone(), source)
  }
}

/// The permissions an extension's static graph is fetched with (see
/// [`ExtensionRegistry::loader_permissions`]). Shared with `ende check`,
/// which fetches the same graph for types.
pub fn extension_loader_permissions(
  desc_parser: Arc<RuntimePermissionDescriptorParser<CliSys>>,
  source: &ModuleSpecifier,
) -> Result<PermissionsContainer, JsErrorBox> {
  let allow_read = extension_directory(source)
    .map(|dir| vec![dir.to_string_lossy().into_owned()]);
  let permissions = Permissions::from_options(
    desc_parser.as_ref(),
    &PermissionsOptions {
      allow_import: Some(vec![]),
      allow_read,
      prompt: false,
      ..Default::default()
    },
  )
  .map_err(to_js_error)?;
  Ok(
    PermissionsContainer::new(desc_parser, permissions)
      .with_import_symlinks_resolved(),
  )
}

/// Starts extension workers for the main thread (see
/// [`ExtensionWorkerProvider`]).
pub struct CliExtensionWorkerProvider(pub Arc<ExtensionRegistry>);

impl ExtensionWorkerProvider for CliExtensionWorkerProvider {
  fn worker_spec(&self, name: &str) -> Result<ExtensionWorkerSpec, JsErrorBox> {
    let registry = &self.0;
    let Some(prepared) = registry.prepared.lock().get(name).cloned() else {
      // The proxy module prepares the extension before it starts the worker.
      return Err(JsErrorBox::type_error(format!(
        "Extension \"{name}\" isn't loaded: import it as \"{EXTENSION_SCHEME}:{name}\""
      )));
    };
    let permissions = Permissions::from_options(
      registry.desc_parser.as_ref(),
      &prepared.options,
    )
    .map_err(to_js_error)?;
    Ok(ExtensionWorkerSpec {
      main_module: harness_module_url(&prepared.source, &prepared.exports)?,
      permissions: PermissionsContainer::new(
        registry.desc_parser.clone(),
        permissions,
      ),
      loader_permissions: registry.loader_permissions(&prepared.source)?,
    })
  }
}

/// The directory a local (`file:`) extension lives in: its source's
/// directory with symlinks resolved, so a link inside it can't reach
/// outside. `None` for a remote or `jsr:` extension.
pub fn extension_directory(source: &ModuleSpecifier) -> Option<PathBuf> {
  if source.scheme() != "file" {
    return None;
  }
  let path = deno_path_util::url_to_file_path(source).ok()?;
  #[allow(clippy::disallowed_methods, reason = "resolving the real directory")]
  let path = std::fs::canonicalize(&path).unwrap_or(path);
  path.parent().map(|dir| dir.to_path_buf())
}

/// Errors if a local extension's folder, with symlinks resolved, is the
/// project folder (`project_root`, which holds ende.json) or above it. The
/// config check compares the URLs as written; this one catches a folder that
/// reaches the project through a symlink, like `./ext` pointing at `.`.
pub fn check_extension_directory(
  name: &str,
  source: &ModuleSpecifier,
  project_root: &Path,
) -> Result<(), JsErrorBox> {
  let Some(directory) = extension_directory(source) else {
    return Ok(());
  };
  #[allow(clippy::disallowed_methods, reason = "resolving the real directory")]
  let project_root = std::fs::canonicalize(project_root)
    .unwrap_or_else(|_| project_root.to_path_buf());
  if !project_root.starts_with(&directory) {
    return Ok(());
  }
  let package = name.split_once('/').map(|(_, p)| p).unwrap_or(name);
  Err(JsErrorBox::type_error(format!(
    "Extension \"{name}\": its source's folder resolves to \"{}\", which holds ende.json. A local extension can read its whole folder, and the app can't write to it, so give it a folder of its own, like \"./ext/{package}/mod.ts\".",
    directory.display()
  )))
}

/// Errors if the app's import map decided how anything in an extension's
/// graph resolves: an extension is self-contained, so each import must
/// resolve as written (a URL, or a path relative to the importing module).
/// A bare specifier only the app's import map defines, or an entry that
/// redirects one of the extension's imports, would let the app (or whoever
/// edits its config) run code the extension's manifest doesn't describe.
pub fn check_extension_graph_resolution(
  name: &str,
  graph: &ModuleGraph,
) -> Result<(), JsErrorBox> {
  for module in graph.modules() {
    let deno_graph::Module::Js(module) = module else {
      continue;
    };
    for (raw, dependency) in &module.dependencies {
      for resolution in [&dependency.maybe_code, &dependency.maybe_type] {
        let Some(resolved) = resolution.ok() else {
          continue;
        };
        let as_written = resolve_as_written(raw, &module.specifier);
        if as_written.as_ref() == Some(&resolved.specifier) {
          continue;
        }
        return Err(JsErrorBox::type_error(match as_written {
          None => format!(
            "Extension \"{name}\": \"{raw}\" (imported from {}) can only be resolved through the app's import map. Extensions resolve without it: import a relative path, https: URL or jsr: package instead",
            module.specifier
          ),
          Some(as_written) => format!(
            "Extension \"{name}\": the app's import map maps \"{raw}\" (imported from {}) to \"{}\" instead of \"{as_written}\". Extensions resolve without the app's import map: remove that entry",
            module.specifier, resolved.specifier
          ),
        }));
      }
    }
  }
  Ok(())
}

/// How `raw`, imported from `referrer`, resolves without an import map:
/// a URL as is, a relative or absolute path against the referrer. `None`
/// for a bare specifier.
fn resolve_as_written(
  raw: &str,
  referrer: &ModuleSpecifier,
) -> Option<ModuleSpecifier> {
  if let Ok(url) = ModuleSpecifier::parse(raw) {
    return Some(url);
  }
  if raw.starts_with("./") || raw.starts_with("../") || raw.starts_with('/') {
    return referrer.join(raw).ok();
  }
  None
}

/// What the lockfile pins about an extension: its source, its manifest and
/// its grant as written, and every file it can run (see
/// [`extension_files`]) with an integrity over them.
fn extension_pin(
  name: &str,
  config: &ExtensionConfig,
  manifest: &ExtensionManifest,
  graph: &ModuleGraph,
) -> Result<ExtensionLockEntry, JsErrorBox> {
  let files = extension_files(name, &config.source, graph)?;
  Ok(ExtensionLockEntry {
    source: config.source_specifier.clone(),
    integrity: Some(files_integrity(&files)),
    files,
    manifest: manifest.permissions_json.clone(),
    grant: config.permissions_json.clone(),
  })
}

/// The most files, and bytes, a local extension's folder may hold. Pinning
/// reads all of them on every run, and an extension's folder is meant to
/// hold only the extension.
const MAX_FOLDER_FILES: usize = 5_000;
const MAX_FOLDER_BYTES: u64 = 100 * 1024 * 1024;

/// Every file extension `name` can run, each with a SHA-256 of its bytes:
/// - for a local source, every entry in its folder, recursively, by path
///   relative to the folder. That is everything the extension may read,
///   which covers text and bytes imports and imports of computed paths that
///   the module graph never sees;
/// - every module in its graph that isn't a local file, by specifier, with
///   the bytes that were loaded (from the cache, the vendor folder or the
///   network), so vendored or re-resolved remote code is pinned too. A jsr
///   module's specifier holds its resolved version.
fn extension_files(
  name: &str,
  source: &ModuleSpecifier,
  graph: &ModuleGraph,
) -> Result<std::collections::BTreeMap<String, String>, JsErrorBox> {
  let mut files = std::collections::BTreeMap::new();
  if let Some(folder) = extension_directory(source) {
    folder_files(name, &folder, &mut files)?;
  }
  for module in graph.modules() {
    let specifier = module.specifier();
    if specifier.scheme() == "file" {
      continue;
    }
    let value = match module {
      deno_graph::Module::Js(module) => sha256(module.source.text.as_bytes()),
      deno_graph::Module::Json(module) => sha256(module.source.text.as_bytes()),
      deno_graph::Module::Wasm(module) => sha256(&module.source),
      _ => "external".to_string(),
    };
    files.insert(specifier.to_string(), value);
  }
  Ok(files)
}

/// Adds every entry under `folder` to `files`. A symlink's value is a hash
/// of where it points and, when that is a file inside the folder, of that
/// file's bytes, so retargeting a link changes the pin. A link that points
/// outside the folder is pinned by its target text alone: the extension
/// can't read through it.
fn folder_files(
  name: &str,
  folder: &Path,
  files: &mut std::collections::BTreeMap<String, String>,
) -> Result<(), JsErrorBox> {
  let too_big = || {
    JsErrorBox::type_error(format!(
      "Extension \"{name}\": its folder {} holds more than {MAX_FOLDER_FILES} files or {} MB. An extension's folder must hold only the extension, since all of it is pinned.",
      folder.display(),
      MAX_FOLDER_BYTES / (1024 * 1024),
    ))
  };
  let io_error = |path: &Path, err: std::io::Error| {
    JsErrorBox::type_error(format!(
      "Extension \"{name}\": couldn't read {} to pin it: {err}",
      path.display()
    ))
  };
  let mut total_bytes = 0u64;
  let mut pending = vec![(folder.to_path_buf(), String::new())];
  while let Some((dir, prefix)) = pending.pop() {
    #[allow(clippy::disallowed_methods, reason = "pinning reads the folder")]
    let entries = std::fs::read_dir(&dir).map_err(|err| io_error(&dir, err))?;
    for entry in entries {
      let entry = entry.map_err(|err| io_error(&dir, err))?;
      let path = entry.path();
      let key = format!("{prefix}{}", entry.file_name().to_string_lossy());
      let file_type = entry.file_type().map_err(|err| io_error(&path, err))?;
      if file_type.is_dir() {
        pending.push((path, format!("{key}/")));
        continue;
      }
      if files.len() >= MAX_FOLDER_FILES {
        return Err(too_big());
      }
      let value = if file_type.is_symlink() {
        #[allow(
          clippy::disallowed_methods,
          reason = "pinning reads the folder"
        )]
        let target = std::fs::read_link(&path)
          .map_err(|err| io_error(&path, err))?
          .to_string_lossy()
          .into_owned();
        #[allow(
          clippy::disallowed_methods,
          reason = "pinning reads the folder"
        )]
        let content = match std::fs::canonicalize(&path) {
          Ok(real) if real.starts_with(folder) && real.is_file() => {
            #[allow(
              clippy::disallowed_methods,
              reason = "pinning reads the folder"
            )]
            let bytes =
              std::fs::read(&real).map_err(|err| io_error(&real, err))?;
            total_bytes += bytes.len() as u64;
            Some(sha256(&bytes))
          }
          _ => None,
        };
        format!(
          "symlink:{}",
          sha256(&serde_json::to_vec(&(target, content)).unwrap())
        )
      } else if file_type.is_file() {
        #[allow(
          clippy::disallowed_methods,
          reason = "pinning reads the folder"
        )]
        let bytes = std::fs::read(&path).map_err(|err| io_error(&path, err))?;
        total_bytes += bytes.len() as u64;
        sha256(&bytes)
      } else {
        // a socket, FIFO or device: pinned by its name and kind
        "special".to_string()
      };
      if total_bytes > MAX_FOLDER_BYTES {
        return Err(too_big());
      }
      files.insert(key, value);
    }
  }
  Ok(())
}

/// The integrity over an extension's files: a SHA-256 of the map as JSON.
/// JSON encodes each key and value with its own delimiters and escapes, so
/// two different maps never hash the same input.
fn files_integrity(
  files: &std::collections::BTreeMap<String, String>,
) -> String {
  sha256(&serde_json::to_vec(files).unwrap())
}

fn sha256(bytes: &[u8]) -> String {
  ende_lib::util::checksum::r#gen(&[bytes])
}

/// The parts of an extension's pin that differ from the approved one.
fn changed_parts(
  approved: &ExtensionLockEntry,
  pin: &ExtensionLockEntry,
) -> Vec<&'static str> {
  let mut parts = Vec::new();
  if approved.source != pin.source || approved.integrity != pin.integrity {
    parts.push("its source");
  }
  if approved.manifest != pin.manifest {
    parts.push("its manifest (the permissions it asks for)");
  }
  if approved.grant != pin.grant {
    parts.push("its grant in ende.json");
  }
  parts
}

/// The change to extension `name` that `ende install` asks about: each part
/// that changed, old and new, and what it will run with.
fn describe_change(
  name: &str,
  approved: &ExtensionLockEntry,
  pin: &ExtensionLockEntry,
  options: &PermissionsOptions,
) -> String {
  let mut text = format!("Extension \"{name}\" changed:");
  if approved.source != pin.source {
    text.push_str(&format!(
      "\n  source: {} -> {}",
      approved.source, pin.source
    ));
  } else if approved.integrity != pin.integrity {
    text.push_str(&format!("\n  source: {}", pin.source));
    text.push_str(&format!(
      "\n  code: {}",
      describe_file_changes(approved, pin)
    ));
  }
  for (label, old, new) in [
    ("manifest (asks for)", &approved.manifest, &pin.manifest),
    ("grant (ende.json)", &approved.grant, &pin.grant),
  ] {
    if old != new {
      text.push_str(&format!("\n  {label}:\n    - {old}\n    + {new}"));
    }
  }
  text.push_str(&format!(
    "\n  it will run with: {}",
    describe_permissions(options)
  ));
  text
}

/// Which of an extension's files changed, were added or were removed since
/// the approved pin, for `ende install` to show.
fn describe_file_changes(
  approved: &ExtensionLockEntry,
  pin: &ExtensionLockEntry,
) -> String {
  if approved.files.is_empty() {
    // pinned before files were listed one by one
    return "changed".to_string();
  }
  let mut changed = Vec::new();
  let mut added = Vec::new();
  let mut removed = Vec::new();
  for (file, hash) in &pin.files {
    match approved.files.get(file) {
      Some(old) if old == hash => {}
      Some(_) => changed.push(file.as_str()),
      None => added.push(file.as_str()),
    }
  }
  for file in approved.files.keys() {
    if !pin.files.contains_key(file) {
      removed.push(file.as_str());
    }
  }
  let parts = [("changed", changed), ("added", added), ("removed", removed)]
    .into_iter()
    .filter(|(_, files)| !files.is_empty())
    .map(|(label, files)| format!("{label} {}", files.join(", ")))
    .collect::<Vec<_>>();
  if parts.is_empty() {
    "changed".to_string()
  } else {
    parts.join("; ")
  }
}

/// Writes what `ende install` recorded, then says so: one line per approved
/// change and per new extension. Nothing is said if the write fails (for
/// example with `--frozen`).
fn write_and_report(
  lockfile: &CliLockfile,
  approved: &[String],
  recorded: &[(String, LoadedExtension)],
) -> Result<(), AnyError> {
  lockfile.write_if_changed()?;
  for name in approved {
    log::info!(
      "Recorded the approved changes to extension \"{name}\" in {}.",
      lockfile_name(lockfile)
    );
  }
  for (name, loaded) in recorded {
    print_recorded_notice(name, lockfile, &loaded.prepared.options);
  }
  Ok(())
}

/// Prints the line for an extension first recorded in the lockfile (or
/// recorded again after its pin was lost). It is shown even with --quiet:
/// a record is the visible sign that a pin was reset, so it is never silent.
fn print_recorded_notice(
  name: &str,
  lockfile: &CliLockfile,
  options: &PermissionsOptions,
) {
  #[allow(clippy::print_stderr, reason = "must show even with --quiet")]
  {
    eprintln!("{}", recorded_notice(name, lockfile, options));
  }
}

/// The line printed when an extension is first recorded in the lockfile
/// (or recorded again after its pin was lost), so a record is never silent.
fn recorded_notice(
  name: &str,
  lockfile: &CliLockfile,
  options: &PermissionsOptions,
) -> String {
  format!(
    "Recorded extension \"{name}\" in {}; it will run with: {}",
    lockfile_name(lockfile),
    describe_permissions(options)
  )
}

/// A one-line summary of effective permissions, like
/// `read /data, net api.example.com`.
fn describe_permissions(options: &PermissionsOptions) -> String {
  let kinds = [
    ("read", &options.allow_read),
    ("write", &options.allow_write),
    ("net", &options.allow_net),
    ("env", &options.allow_env),
    ("run", &options.allow_run),
    ("ffi", &options.allow_ffi),
    ("sys", &options.allow_sys),
    ("import", &options.allow_import),
  ];
  let parts = kinds
    .iter()
    .filter_map(|(kind, allow)| {
      let allow = allow.as_ref()?;
      Some(if allow.is_empty() && *kind == "write" {
        // Ende fences scoped writes off from the app's code (see
        // `ende_permissions::check_code_load`); everything can't be.
        "write (all; can escape: it can change the app's code)".to_string()
      } else if allow.is_empty() {
        format!("{kind} (all)")
      } else {
        format!("{kind} {}", allow.join(", "))
      })
    })
    .collect::<Vec<_>>();
  if parts.is_empty() {
    "no permissions".to_string()
  } else {
    parts.join("; ")
  }
}

fn lockfile_name(lockfile: &CliLockfile) -> String {
  lockfile
    .filename
    .file_name()
    .map(|name| name.to_string_lossy().into_owned())
    .unwrap_or_else(|| "the lockfile".to_string())
}

fn to_js_error(err: impl std::fmt::Display) -> JsErrorBox {
  JsErrorBox::type_error(err.to_string())
}

fn js_string(value: &str) -> String {
  ende_core::serde_json::to_string(value).unwrap()
}

/// The module `import … from "extension:<name>"` loads in the app: one
/// async function per export, each a call into the extension's worker.
fn proxy_module_code(name: &str, exports: &[String]) -> String {
  let name = js_string(name);
  let mut code = format!(
    "const {{ start, call }} = globalThis.Ende[globalThis.Ende.internal].extensions;\nawait start({name});\n"
  );
  for (index, export) in exports.iter().enumerate() {
    let export = js_string(export);
    code.push_str(&format!(
      "const e{index} = (...args) => call({name}, {export}, args);\nexport {{ e{index} as {export} }};\n"
    ));
  }
  code
}

/// The main module of an extension's worker, as a `data:` URL: it imports
/// the extension and serves the app's calls to `exports`.
fn harness_module_url(
  source: &ModuleSpecifier,
  exports: &[String],
) -> Result<ModuleSpecifier, JsErrorBox> {
  let exports = ende_core::serde_json::to_string(exports).unwrap();
  let code = HARNESS
    .replace("__ENDE_EXTENSION_EXPORTS__", &exports)
    .replace("__ENDE_EXTENSION_SOURCE__", &js_string(source.as_str()));
  let url = format!(
    "data:application/javascript;base64,{}",
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, code)
  );
  ModuleSpecifier::parse(&url).map_err(JsErrorBox::from_err)
}

const HARNESS: &str = r#"import * as extension from __ENDE_EXTENSION_SOURCE__;
const exportNames = new Set(__ENDE_EXTENSION_EXPORTS__);
const hostObjectBrand = Symbol.for("Deno.core.hostObject");
function roots(value, found = [], seen = new Set()) {
  if (value === null || typeof value !== "object" || seen.has(value)) {
    return found;
  }
  seen.add(value);
  if (value[hostObjectBrand] === "EndeRoot") {
    found.push(value);
  } else if (Array.isArray(value)) {
    for (const item of value) roots(item, found, seen);
  } else if (Object.getPrototypeOf(value) === Object.prototype) {
    for (const item of Object.values(value)) roots(item, found, seen);
  }
  return found;
}
function describe(error) {
  if (error instanceof DOMException) {
    return { name: error.name, message: error.message, stack: error.stack, isDOMException: true };
  }
  if (error instanceof Error) {
    return { name: error.name, message: error.message, stack: error.stack };
  }
  return undefined;
}
addEventListener("message", async ({ data: { id, name, args } }) => {
  let reply;
  try {
    if (!exportNames.has(name) || typeof extension[name] !== "function") {
      throw new TypeError(`"${name}" is not a function the extension exports`);
    }
    reply = { id, ok: true, value: await extension[name](...args) };
  } catch (thrown) {
    const error = describe(thrown);
    reply = error === undefined ? { id, ok: false, thrown } : { id, ok: false, error };
  }
  try {
    postMessage(reply, roots(reply.value));
  } catch (error) {
    postMessage({ id, ok: false, error: describe(error) });
  }
});
postMessage({ ready: true });
"#;

#[cfg(test)]
#[allow(clippy::disallowed_methods, reason = "test fixtures on disk")]
mod tests {
  use std::collections::BTreeMap;

  use super::*;

  fn files_of(folder: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    folder_files("@acme/fmt", folder, &mut files).unwrap();
    files
  }

  #[test]
  fn integrity_tells_apart_files_split_differently() {
    // One file holding "a\0b.ts\0b" vs two files "a" and "b": the old
    // `key \0 bytes \0` framing hashed both the same.
    let one = BTreeMap::from([("a.ts".to_string(), sha256(b"a\0b.ts\0b"))]);
    let two = BTreeMap::from([
      ("a.ts".to_string(), sha256(b"a")),
      ("b.ts".to_string(), sha256(b"b")),
    ]);
    assert_ne!(files_integrity(&one), files_integrity(&two));
  }

  #[test]
  fn folder_files_cover_every_file() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().canonicalize().unwrap();
    std::fs::write(folder.join("mod.ts"), "export const a = 1;").unwrap();
    std::fs::write(folder.join("payload.txt"), "v1").unwrap();
    std::fs::create_dir(folder.join("sub")).unwrap();
    std::fs::write(folder.join("sub").join("x.ts"), "x").unwrap();
    let files = files_of(&folder);
    assert_eq!(
      files.keys().collect::<Vec<_>>(),
      ["mod.ts", "payload.txt", "sub/x.ts"]
    );
    assert_eq!(files["payload.txt"], sha256(b"v1"));
    std::fs::write(folder.join("payload.txt"), "v2").unwrap();
    assert_ne!(files_of(&folder)["payload.txt"], files["payload.txt"]);
  }

  #[cfg(unix)]
  #[test]
  fn folder_files_pin_where_symlinks_point() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let folder = root.join("ext");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.ts"), "a").unwrap();
    std::fs::write(folder.join("b.ts"), "b").unwrap();
    std::fs::write(root.join("outside.ts"), "o").unwrap();
    std::os::unix::fs::symlink("a.ts", folder.join("link.ts")).unwrap();
    std::os::unix::fs::symlink("../outside.ts", folder.join("out.ts")).unwrap();
    let before = files_of(&folder);
    assert!(before["link.ts"].starts_with("symlink:"));
    assert!(before["out.ts"].starts_with("symlink:"));
    // retargeting a link inside the folder changes its pin
    std::fs::remove_file(folder.join("link.ts")).unwrap();
    std::os::unix::fs::symlink("b.ts", folder.join("link.ts")).unwrap();
    let after = files_of(&folder);
    assert_ne!(after["link.ts"], before["link.ts"]);
    // so does a change to the file a link points at
    std::fs::write(folder.join("b.ts"), "b2").unwrap();
    assert_ne!(files_of(&folder)["link.ts"], after["link.ts"]);
    // a link out of the folder is pinned by its target text only
    std::fs::write(root.join("outside.ts"), "o2").unwrap();
    assert_eq!(files_of(&folder)["out.ts"], before["out.ts"]);
  }

  #[test]
  fn folder_files_refuse_a_folder_that_is_too_big() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().canonicalize().unwrap();
    for i in 0..=MAX_FOLDER_FILES {
      std::fs::write(folder.join(format!("f{i}.ts")), "").unwrap();
    }
    let mut files = BTreeMap::new();
    let err = folder_files("@acme/fmt", &folder, &mut files).unwrap_err();
    assert!(err.to_string().contains("more than 5000 files"), "{err}");
  }
}
