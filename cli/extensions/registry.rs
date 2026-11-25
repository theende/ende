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
use ende_core::parking_lot::Mutex;
use ende_runtime::ende_permissions::Permissions;
use ende_runtime::ende_permissions::PermissionsContainer;
use ende_runtime::ende_permissions::PermissionsOptions;
use ende_runtime::ende_permissions::RuntimePermissionDescriptorParser;
use ende_runtime::ops::worker_host::ExtensionWorkerProvider;
use ende_runtime::ops::worker_host::ExtensionWorkerSpec;

use super::extension_permissions_options;
use super::manifest::extract_manifest;
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
  prepared: Mutex<HashMap<String, Arc<PreparedExtension>>>,
}

impl ExtensionRegistry {
  pub fn new(
    config: Result<ExtensionsConfig, String>,
    module_load_preparer: Arc<ModuleLoadPreparer>,
    resolver: Arc<CliResolver>,
    desc_parser: Arc<RuntimePermissionDescriptorParser<CliSys>>,
    project_root: PathBuf,
  ) -> Self {
    Self {
      config,
      module_load_preparer,
      resolver,
      desc_parser,
      project_root,
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
      .map_err(JsErrorBox::from_err)?;
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
    let prepared = Arc::new(PreparedExtension {
      source: config.source,
      exports: manifest.exports,
      options: effective.options,
      graph: Arc::new(graph),
    });
    let mut map = self.prepared.lock();
    if let Some(existing) = map.get(name) {
      return Ok(existing.clone());
    }
    map.insert(name.to_string(), prepared.clone());
    drop(map);
    if let Some(warning) = effective.warning {
      log::warn!("{} {}", crate::colors::yellow("Warning"), warning);
    }
    Ok(prepared)
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
    let allow_read = extension_directory(source)
      .map(|dir| vec![dir.to_string_lossy().into_owned()]);
    let permissions = Permissions::from_options(
      self.desc_parser.as_ref(),
      &PermissionsOptions {
        allow_import: Some(vec![]),
        allow_read,
        prompt: false,
        ..Default::default()
      },
    )
    .map_err(to_js_error)?;
    Ok(
      PermissionsContainer::new(self.desc_parser.clone(), permissions)
        .with_import_symlinks_resolved(),
    )
  }
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
