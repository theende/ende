// Copyright 2018-2026 the Ende authors. MIT license.

//! Types of sandboxed imports for the type checker and the editor.
//!
//! At run time the app imports a sandboxed module through a generated proxy
//! (see `super::sandbox`). For type checking, an import from outside a
//! sandbox into a module it holds resolves to a TypeScript stub instead,
//! which presents the module as its proxy does:
//!
//! - each function export with the same parameters and its result wrapped
//!   in a `Promise` (`__Proxied`, as for extensions), each constant as it
//!   is;
//! - exports that can't cross (a class, a `let`/`var` binding, `then`) only
//!   as types, through `export type *`, so a type-only use still works.
//!   Importing one by value is an error at the import (see
//!   [`unsupported_imports`]), as it is when the app runs;
//! - the module's types (interfaces, type aliases) as they are.
//!
//! The stub's exports come from the module's source, which the first graph
//! build brings in (the stub imports it for types), so the graph is built
//! twice, as for extension stubs.
//!
//! A stub's first line holds what the editor and the type checker need to
//! report about it ([`StubInfo`]).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;

use base64::Engine;
use deno_ast::MediaType;
use deno_ast::ModuleSpecifier;
use deno_graph::ModuleGraph;
use deno_graph::source::CacheInfo;
use deno_graph::source::LoadFuture;
use deno_graph::source::LoadOptions;
use deno_graph::source::LoadResponse;
use deno_graph::source::Loader;
use deno_graph::source::ResolutionKind;
use deno_graph::source::ResolveError;
use deno_graph::source::Resolver;
use ende_config::ende_json::IsolateConfig;
use ende_core::futures::FutureExt;
use ende_core::serde_json;
use ende_core::url::Url;
use serde::Deserialize;
use serde::Serialize;

use super::Classifier;
use super::exports::ModuleExports;
use super::exports::value_imports;
use super::paths_of;
use crate::sys::CliSys;

/// The scheme of the type stubs of sandboxed modules.
pub const TYPES_SCHEME: &str = "isolate-types";

/// The stub that types `entry`, a sandboxed module as the app's import
/// resolved it. TypeScript reads a scheme without `//` as a relative path,
/// so the stub has the rooted form, and a `.ts` extension.
pub fn stub_specifier(entry: &ModuleSpecifier) -> ModuleSpecifier {
  let encoded =
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(entry.as_str());
  ModuleSpecifier::parse(&format!("{TYPES_SCHEME}:///{encoded}.ts")).unwrap()
}

/// The entry a stub types, if `specifier` is a stub.
pub fn stub_entry(specifier: &ModuleSpecifier) -> Option<ModuleSpecifier> {
  if specifier.scheme() != TYPES_SCHEME {
    return None;
  }
  let encoded = specifier.path().strip_prefix('/')?.strip_suffix(".ts")?;
  let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
    .decode(encoded)
    .ok()?;
  ModuleSpecifier::parse(std::str::from_utf8(&bytes).ok()?).ok()
}

/// What a stub says about the module it types, on its first line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StubInfo {
  /// The matcher of the rule whose sandbox holds the module.
  pub rule: String,
  /// The module, after redirects.
  pub module: String,
  /// The exports that can't cross, with why.
  pub unsupported: BTreeMap<String, String>,
  /// The `permissions` of the module's manifest, if it declares one: the
  /// editor's hover intersects every entry's (see
  /// [`SandboxRules::runs_with`]).
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub manifest: Option<serde_json::Value>,
  /// Why the module's manifest can't be read, if it can't: the sandbox
  /// won't start until it's fixed.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub manifest_error: Option<String>,
}

const INFO_PREFIX: &str = "// isolate-stub: ";

/// The [`StubInfo`] of a stub's text, if it is a stub.
pub fn stub_info(text: &str) -> Option<StubInfo> {
  let line = text.lines().next()?.strip_prefix(INFO_PREFIX)?;
  serde_json::from_str(line).ok()
}

/// The stub's TypeScript. `import` is the entry as the app's import
/// resolved it, which the stub imports for types; `exports` are its
/// exports, as its source shows them.
pub fn stub_source(
  info: &StubInfo,
  import: &str,
  exports: &ModuleExports,
) -> String {
  let import = serde_json::to_string(import).unwrap();
  let mut text = format!(
    "{INFO_PREFIX}{}\n\
     // Types of a module in the sandbox of isolate[{}] as its proxy\n\
     // presents them: every call crosses to the sandbox's worker, so every\n\
     // result is a Promise.\n\
     import {import};\n\
     import type * as __entry from {import};\n\
     export type * from {import};\n\
     type __Proxied<T> = T extends abstract new (...args: any) => any ? never\n  \
     : T extends (...args: infer A) => infer R\n    \
     ? (...args: A) => Promise<Awaited<R>>\n  : T;\n",
    serde_json::to_string(info).unwrap(),
    serde_json::to_string(&info.rule).unwrap(),
  );
  let mut index = 0;
  for name in exports.names.keys() {
    if info.unsupported.contains_key(name) {
      // Only its type crosses: a type-only use works, as when the app runs
      // (TypeScript drops the import), and a use as a value is an error
      // (see `unsupported_imports`).
      if is_identifier(name) || name == "default" {
        text.push_str(&format!("export type {{ {name} }} from {import};\n"));
      }
      continue;
    }
    let quoted = serde_json::to_string(name).unwrap();
    let exported = if is_identifier(name) {
      name.clone()
    } else {
      quoted.clone()
    };
    text.push_str(&format!(
      "declare const __e{index}: __Proxied<(typeof __entry)[{quoted}]>;\nexport {{ __e{index} as {exported} }};\n"
    ));
    index += 1;
  }
  text
}

/// The stub before the module's exports are known: it only brings the
/// module into the graph. Each stub imports its module as code too, so a
/// graph pruned of types for running (`ModuleGraph::prune_types`) keeps
/// it; the stub itself never runs.
fn pending_stub_source(import: &str) -> String {
  let import = serde_json::to_string(import).unwrap();
  format!(
    "import {import};\nimport type * as __entry from {import};\nexport {{}};\n"
  )
}

/// A sandboxed JSON module is data: the app imports it as is.
fn json_stub_source(import: &str) -> String {
  format!(
    "export {{ default }} from {} with {{ type: \"json\" }};\n",
    serde_json::to_string(import).unwrap()
  )
}

fn is_identifier(name: &str) -> bool {
  let mut chars = name.chars();
  chars
    .next()
    .is_some_and(|c| c == '_' || c == '$' || c.is_alphabetic())
    && chars.all(|c| c == '_' || c == '$' || c.is_alphanumeric())
}

/// Why export `name` of kind `kind` can't cross, if it can't.
fn unsupported_exports(exports: &ModuleExports) -> BTreeMap<String, String> {
  exports
    .names
    .iter()
    .filter_map(|(name, kind)| {
      Some((
        name.clone(),
        super::sandbox::unsupported_export(name, kind)?.to_string(),
      ))
    })
    .collect()
}

/// The stub of `entry` (an import's resolved specifier) whose module and
/// its re-exports `source` holds; `manifest` is what its manifest asks for,
/// for the editor. `None` if the module isn't a loaded JavaScript,
/// TypeScript or JSON module.
pub fn stub_for(
  source: &dyn super::exports::ExportsSource,
  rule: &str,
  entry: &ModuleSpecifier,
  manifest: Result<Option<serde_json::Value>, String>,
) -> Option<String> {
  match source.module(entry)? {
    super::exports::SourceModule::Json => {
      Some(json_stub_source(entry.as_str()))
    }
    super::exports::SourceModule::Js { specifier, .. } => {
      let exports = super::exports::module_exports_from(source, &specifier);
      let info = StubInfo {
        rule: rule.to_string(),
        module: specifier.to_string(),
        unsupported: unsupported_exports(&exports),
        manifest: manifest.clone().ok().flatten(),
        manifest_error: manifest.err(),
      };
      Some(stub_source(&info, entry.as_str(), &exports))
    }
  }
}

/// Whether an import resolved to `specifier` is of code, by its media
/// type: JSON and other data are imported as they are.
pub fn is_code(specifier: &ModuleSpecifier) -> bool {
  match specifier.scheme() {
    "jsr" => true,
    "file" | "http" | "https" => {
      matches!(
        MediaType::from_specifier(specifier),
        MediaType::JavaScript
          | MediaType::Jsx
          | MediaType::Mjs
          | MediaType::Cjs
          | MediaType::TypeScript
          | MediaType::Mts
          | MediaType::Cts
          | MediaType::Tsx
          | MediaType::Dts
          | MediaType::Dmts
          | MediaType::Dcts
      ) || (specifier.scheme() != "file"
        && MediaType::from_specifier(specifier) == MediaType::Unknown)
    }
    _ => false,
  }
}

/// The `isolate` rules, ready to tell which sandbox a module is in, for
/// the type checker's stubs and the editor's.
pub struct SandboxRules {
  config: IsolateConfig,
  classifier: Classifier,
  sys: CliSys,
}

impl std::fmt::Debug for SandboxRules {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("SandboxRules")
      .field("rules", &self.config.rules.keys().collect::<Vec<_>>())
      .finish()
  }
}

impl SandboxRules {
  /// `None` when there are no rules.
  pub fn new(config: IsolateConfig, jsr_url: &Url) -> Option<Self> {
    if config.rules.is_empty() {
      return None;
    }
    let sys = CliSys::default();
    Some(Self {
      classifier: Classifier::new(&config, jsr_url, &sys),
      config,
      sys,
    })
  }

  /// The matcher of the rule that holds `specifier`, by the specifier
  /// alone.
  pub fn rule_of(&self, specifier: &ModuleSpecifier) -> Option<&str> {
    let id = self
      .classifier
      .sandbox_of_specifier(specifier, &|url| paths_of(url, &self.sys))?;
    self
      .config
      .rules
      .get_index(id.0)
      .map(|(matcher, _)| matcher.as_str())
  }

  /// Whether an import in `referrer` that resolved to `resolved` is typed
  /// through a stub: it goes from outside every sandbox into a sandboxed
  /// module of code.
  ///
  /// `target` is what `resolved` redirects to, when known: a URL that
  /// redirects into a sandbox is sandboxed when the app runs, so it is
  /// typed through a stub too.
  pub fn is_proxied(
    &self,
    referrer: &ModuleSpecifier,
    resolved: &ModuleSpecifier,
    target: Option<&ModuleSpecifier>,
  ) -> bool {
    referrer.scheme() != TYPES_SCHEME
      && referrer.scheme() != "data"
      && (is_code(resolved) || target.is_some_and(is_code))
      && self.rule_of(referrer).is_none()
      && (self.rule_of(resolved).is_some()
        || target.is_some_and(|target| self.rule_of(target).is_some()))
  }

  /// The `permissions` of the manifest `module` (in the sandbox of `rule`,
  /// with `text` of `media_type`) declares, if any.
  pub fn manifest_of(
    &self,
    rule: &str,
    module: &ModuleSpecifier,
    media_type: MediaType,
    text: &str,
  ) -> Result<Option<serde_json::Value>, String> {
    crate::extensions::extract_package_manifest(rule, module, media_type, text)
      .map(|manifest| manifest.map(|manifest| manifest.permissions_json))
      .map_err(|err| err.to_string())
  }

  /// What the sandbox of `rule` runs with when its entries declare
  /// `manifests` (each manifest's `permissions`): the grant ∩ every
  /// manifest, as its worker gets it. The app's own permissions narrow it
  /// further when it runs.
  pub fn runs_with<'a>(
    &self,
    rule: &str,
    manifests: impl Iterator<Item = &'a serde_json::Value>,
  ) -> Option<String> {
    let config = self.config.rules.get(rule)?;
    let grant =
      crate::args::extension_grant_permissions_options(&config.permissions)
        .ok()?;
    // A manifest as declared is a literal, so it reads back as one.
    let specifier = ModuleSpecifier::parse("file:///manifest.ts").unwrap();
    let mut read = Vec::new();
    for permissions in manifests {
      let source =
        format!("export const manifest = {{ permissions: {permissions} }};");
      read.push(
        crate::extensions::extract_package_manifest(
          rule,
          &specifier,
          MediaType::TypeScript,
          &source,
        )
        .ok()??,
      );
    }
    let parser =
      ende_runtime::ende_permissions::RuntimePermissionDescriptorParser::new(
        self.sys.clone(),
      );
    let (options, _) = super::manifests::effective_options(
      rule,
      &grant,
      read.iter().map(|manifest| ("", manifest)),
      &parser,
    )
    .ok()?;
    Some(crate::pins::describe_permissions(&options))
  }
}

/// The type stubs of one graph build.
pub struct IsolateTypes {
  rules: SandboxRules,
  /// Each stub's text once its module's exports are known.
  stubs: RefCell<HashMap<ModuleSpecifier, String>>,
  /// URLs outside every sandbox that the graph found redirect into one,
  /// with where they redirect.
  redirects: RefCell<HashMap<ModuleSpecifier, ModuleSpecifier>>,
}

impl IsolateTypes {
  /// `None` when there are no rules.
  pub fn new(config: &IsolateConfig, jsr_url: &Url) -> Option<Self> {
    Some(Self {
      rules: SandboxRules::new(config.clone(), jsr_url)?,
      stubs: Default::default(),
      redirects: Default::default(),
    })
  }

  fn rule_of(&self, specifier: &ModuleSpecifier) -> Option<&str> {
    self.rules.rule_of(specifier)
  }

  /// A resolver that sends an import from outside every sandbox into a
  /// module a sandbox holds to its stub, and everything else to `inner`.
  pub fn resolver<'a>(
    &'a self,
    inner: &'a dyn Resolver,
  ) -> IsolateTypesResolver<'a> {
    IsolateTypesResolver { types: self, inner }
  }

  /// A loader serving the stubs; everything else goes to `inner`.
  pub fn loader<'a>(&'a self, inner: &'a dyn Loader) -> IsolateTypesLoader<'a> {
    IsolateTypesLoader { types: self, inner }
  }

  /// After the first build, the modules that import a URL which redirects
  /// into a sandbox, to load again so that the import gets its stub. The
  /// resolver can't see redirects; the graph has them now.
  pub fn redirected_importers(
    &self,
    graph: &ModuleGraph,
  ) -> Vec<ModuleSpecifier> {
    let mut found = HashMap::new();
    for from in graph.redirects.keys() {
      let target = graph.resolve(from);
      if self.rules.rule_of(from).is_none()
        && self.rules.rule_of(target).is_some()
        && is_code(target)
      {
        found.insert(from.clone(), target.clone());
      }
    }
    if found.is_empty() {
      return Vec::new();
    }
    let mut importers = Vec::new();
    for module in graph.modules() {
      let deno_graph::Module::Js(module) = module else {
        continue;
      };
      if stub_entry(&module.specifier).is_some()
        || self.rules.rule_of(&module.specifier).is_some()
      {
        continue;
      }
      if module.dependencies.values().any(|dependency| {
        dependency
          .get_code()
          .is_some_and(|code| found.contains_key(code))
      }) {
        importers.push(module.specifier.clone());
      }
    }
    self.redirects.borrow_mut().extend(found);
    importers
  }

  /// After the first build, works out each stub from its module in
  /// `graph`, and returns the stubs to load again.
  pub fn fill_stubs(&self, graph: &ModuleGraph) -> Vec<ModuleSpecifier> {
    let mut reload = Vec::new();
    let stubs = graph
      .specifiers()
      .map(|(specifier, _)| specifier.clone())
      .filter(|specifier| specifier.scheme() == TYPES_SCHEME)
      .collect::<Vec<_>>();
    for stub in stubs {
      if self.stubs.borrow().contains_key(&stub) {
        continue;
      }
      let Some(entry) = stub_entry(&stub) else {
        continue;
      };
      let target = graph.resolve(&entry).clone();
      let Some(rule) = self.rule_of(&target).or_else(|| self.rule_of(&entry))
      else {
        continue;
      };
      let Some(text) = stub_for(graph, rule, &entry, Ok(None)) else {
        continue;
      };
      self.stubs.borrow_mut().insert(stub.clone(), text);
      reload.push(stub);
    }
    reload
  }
}

/// See [`IsolateTypes::resolver`].
#[derive(Debug)]
pub struct IsolateTypesResolver<'a> {
  types: &'a IsolateTypes,
  inner: &'a dyn Resolver,
}

impl std::fmt::Debug for IsolateTypes {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("IsolateTypes")
      .field("rules", &self.rules)
      .finish()
  }
}

impl Resolver for IsolateTypesResolver<'_> {
  fn default_jsx_import_source(
    &self,
    referrer: &ModuleSpecifier,
  ) -> Option<String> {
    self.inner.default_jsx_import_source(referrer)
  }

  fn default_jsx_import_source_types(
    &self,
    referrer: &ModuleSpecifier,
  ) -> Option<String> {
    self.inner.default_jsx_import_source_types(referrer)
  }

  fn jsx_import_source_module(&self, referrer: &ModuleSpecifier) -> &str {
    self.inner.jsx_import_source_module(referrer)
  }

  fn resolve(
    &self,
    specifier_text: &str,
    referrer_range: &deno_graph::Range,
    kind: ResolutionKind,
  ) -> Result<ModuleSpecifier, ResolveError> {
    let referrer = &referrer_range.specifier;
    if referrer.scheme() == TYPES_SCHEME {
      // A stub imports its module by the specifier the app's import
      // resolved to; resolving it again could remap it.
      return Ok(deno_graph::resolve_import(specifier_text, referrer)?);
    }
    let resolved = self.inner.resolve(specifier_text, referrer_range, kind)?;
    let redirects = self.types.redirects.borrow();
    if self.types.rules.is_proxied(
      referrer,
      &resolved,
      redirects.get(&resolved),
    ) {
      Ok(stub_specifier(&resolved))
    } else {
      Ok(resolved)
    }
  }

  fn resolve_attribute_type_import(
    &self,
    specifier_text: &str,
    referrer_range: &deno_graph::Range,
    kind: ResolutionKind,
    attribute_type: &str,
  ) -> Option<Result<ModuleSpecifier, ResolveError>> {
    // JSON, text and bytes imports are data: never a stub.
    Some(
      self
        .inner
        .resolve_attribute_type_import(
          specifier_text,
          referrer_range,
          kind,
          attribute_type,
        )
        .unwrap_or_else(|| {
          self.inner.resolve(specifier_text, referrer_range, kind)
        }),
    )
  }

  fn resolve_types(
    &self,
    specifier: &ModuleSpecifier,
  ) -> Result<Option<(ModuleSpecifier, Option<deno_graph::Range>)>, ResolveError>
  {
    self.inner.resolve_types(specifier)
  }
}

/// See [`IsolateTypes::loader`].
pub struct IsolateTypesLoader<'a> {
  types: &'a IsolateTypes,
  inner: &'a dyn Loader,
}

impl Loader for IsolateTypesLoader<'_> {
  fn max_redirects(&self) -> usize {
    self.inner.max_redirects()
  }

  fn cache_info_enabled(&self) -> bool {
    self.inner.cache_info_enabled()
  }

  fn get_cache_info(&self, specifier: &ModuleSpecifier) -> Option<CacheInfo> {
    self.inner.get_cache_info(specifier)
  }

  fn load(
    &self,
    specifier: &ModuleSpecifier,
    options: LoadOptions,
  ) -> LoadFuture {
    if specifier.scheme() == TYPES_SCHEME
      && let Some(entry) = stub_entry(specifier)
    {
      let content = self
        .types
        .stubs
        .borrow()
        .get(specifier)
        .cloned()
        .unwrap_or_else(|| pending_stub_source(entry.as_str()));
      let response = LoadResponse::Module {
        specifier: specifier.clone(),
        content: content.into_bytes().into(),
        mtime: None,
        maybe_headers: Some(HashMap::from([(
          "content-type".to_string(),
          "application/typescript".to_string(),
        )])),
      };
      return std::future::ready(Ok(Some(response))).boxed_local();
    }
    self.inner.load(specifier, options)
  }
}

/// An import by value of an export that can't leave its sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedImport {
  /// Zero-based line and column of the imported name.
  pub start: (usize, usize),
  pub end: (usize, usize),
  pub message: String,
}

/// The imports by value, in the module `text` (`specifier`, of
/// `media_type`), of exports that can't leave a sandbox. `stub_of` gives
/// the [`StubInfo`] of the stub an import (by specifier as written)
/// resolves to, if it does.
pub fn unsupported_imports(
  specifier: &ModuleSpecifier,
  media_type: MediaType,
  text: &str,
  transpile: &deno_ast::TranspileOptions,
  stub_of: &dyn Fn(&str) -> Option<StubInfo>,
) -> Vec<UnsupportedImport> {
  let Some(imports) = value_imports(specifier, media_type, text) else {
    return Vec::new();
  };
  // Only what the code that runs still imports crosses: TypeScript drops
  // an import used only as a type, and so does running.
  let runs = code_that_runs(specifier, media_type, text, transpile)
    .and_then(|code| super::exports::requested_exports(specifier, &code));
  let mut found = Vec::new();
  let mut infos: HashMap<String, Option<StubInfo>> = HashMap::new();
  for import in imports {
    let info = infos
      .entry(import.src.clone())
      .or_insert_with(|| stub_of(&import.src));
    let Some(info) = info else {
      continue;
    };
    let Some(what) = info.unsupported.get(&import.name) else {
      continue;
    };
    if let Some(runs) = &runs
      && !runs
        .get(&import.src)
        .is_some_and(|requested| requested.names.contains(&import.name))
    {
      continue;
    }
    found.push(UnsupportedImport {
      start: import.start,
      end: import.end,
      message: format!(
        "{}. Only its type can be used outside the sandbox.",
        super::sandbox::cannot_import_message(
          &info.rule,
          &info.module,
          &import.name,
          what
        )
      ),
    });
  }
  found
}

/// The JavaScript that runs for the module `text`: TypeScript transpiled
/// with `transpile`, which drops imports used only as types.
fn code_that_runs(
  specifier: &ModuleSpecifier,
  media_type: MediaType,
  text: &str,
  transpile: &deno_ast::TranspileOptions,
) -> Option<String> {
  if !matches!(
    media_type,
    MediaType::TypeScript
      | MediaType::Mts
      | MediaType::Cts
      | MediaType::Tsx
      | MediaType::Jsx
  ) {
    return Some(text.to_string());
  }
  let parsed = deno_ast::parse_module(deno_ast::ParseParams {
    specifier: specifier.clone(),
    text: text.into(),
    media_type,
    capture_tokens: false,
    scope_analysis: false,
    maybe_syntax: None,
  })
  .ok()?;
  let emitted = parsed
    .transpile(
      transpile,
      &deno_ast::TranspileModuleOptions {
        module_kind: Some(deno_ast::ModuleKind::Esm),
      },
      &deno_ast::EmitOptions {
        source_map: deno_ast::SourceMapOption::None,
        ..Default::default()
      },
    )
    .ok()?;
  Some(emitted.into_source().text)
}

/// The editor's form of a stub: a `data:` module holding `text`.
pub fn data_stub(text: &str) -> Option<ModuleSpecifier> {
  ModuleSpecifier::parse(&format!(
    "{DATA_STUB_PREFIX}{}",
    base64::engine::general_purpose::STANDARD.encode(text)
  ))
  .ok()
}

/// The [`StubInfo`] of an editor stub (see [`data_stub`]), if `specifier`
/// is one.
pub fn data_stub_info(specifier: &ModuleSpecifier) -> Option<StubInfo> {
  let encoded = specifier.as_str().strip_prefix(DATA_STUB_PREFIX)?;
  let bytes = base64::engine::general_purpose::STANDARD
    .decode(encoded)
    .ok()?;
  stub_info(std::str::from_utf8(&bytes).ok()?)
}

const DATA_STUB_PREFIX: &str = "data:application/typescript;base64,";

/// Whether `specifier` is a sandboxed module's type stub.
pub fn is_stub(specifier: &ModuleSpecifier) -> bool {
  specifier.scheme() == TYPES_SCHEME
}

#[cfg(test)]
mod test {
  use pretty_assertions::assert_eq;

  use super::*;
  use crate::isolate::exports::ExportKind;

  #[test]
  fn stub_specifiers_round_trip() {
    let entry =
      ModuleSpecifier::parse("jsr:@endetest/multiple-exports@1/add").unwrap();
    let stub = stub_specifier(&entry);
    assert_eq!(stub.scheme(), TYPES_SCHEME);
    assert!(stub.path().ends_with(".ts"));
    assert_eq!(stub_entry(&stub), Some(entry));
    let file = ModuleSpecifier::parse("file:///a.ts").unwrap();
    assert_eq!(stub_entry(&file), None);
  }

  #[test]
  fn stub_proxies_functions_and_keeps_unsupported_exports_as_types() {
    let exports = ModuleExports {
      names: BTreeMap::from([
        ("parse".to_string(), ExportKind::Function),
        ("VERSION".to_string(), ExportKind::Value),
        ("Widget".to_string(), ExportKind::Class),
        ("a-b".to_string(), ExportKind::Unknown),
        ("default".to_string(), ExportKind::Function),
      ]),
      complete: true,
    };
    let info = StubInfo {
      rule: "./vendor/a/".to_string(),
      module: "file:///app/vendor/a/mod.ts".to_string(),
      unsupported: unsupported_exports(&exports),
      manifest: Some(serde_json::json!(["read"])),
      manifest_error: None,
    };
    assert_eq!(
      info.unsupported,
      BTreeMap::from([("Widget".to_string(), "a class".to_string())])
    );
    let text = stub_source(&info, "file:///app/vendor/a/mod.ts", &exports);
    assert_eq!(stub_info(&text), Some(info));
    assert!(text.contains(
      "import \"file:///app/vendor/a/mod.ts\";\nimport type * as __entry from \"file:///app/vendor/a/mod.ts\";\nexport type * from \"file:///app/vendor/a/mod.ts\";"
    ));
    assert!(text.contains(
      "__Proxied<(typeof __entry)[\"parse\"]>;\nexport { __e3 as parse };"
    ));
    assert!(text.contains("export { __e0 as VERSION };"));
    assert!(text.contains("export { __e1 as \"a-b\" };"));
    assert!(text.contains("export { __e2 as default };"));
    // A class crosses only as a type.
    assert!(!text.contains("as Widget"));
    assert!(text.contains(
      "export type { Widget } from \"file:///app/vendor/a/mod.ts\";"
    ));
  }

  #[test]
  fn unsupported_imports_name_the_export_and_the_rule() {
    let info = StubInfo {
      rule: "./vendor/a/".to_string(),
      module: "file:///app/vendor/a/mod.ts".to_string(),
      unsupported: BTreeMap::from([(
        "Widget".to_string(),
        "a class".to_string(),
      )]),
      manifest: None,
      manifest_error: None,
    };
    let found = |text: &str| {
      unsupported_imports(
        &ModuleSpecifier::parse("file:///app/main.ts").unwrap(),
        MediaType::TypeScript,
        text,
        &Default::default(),
        &|src| (src == "./vendor/a/mod.ts").then(|| info.clone()),
      )
    };
    // Used only as a type: TypeScript drops the import, so nothing
    // crosses, as when the app runs.
    assert_eq!(
      found(
        "import type { Widget as W } from \"./vendor/a/mod.ts\";\nimport { parse, Widget } from \"./vendor/a/mod.ts\";\nlet w: Widget | W | undefined;\nconsole.log(parse, w);\n"
      ),
      []
    );
    // Used as a value: an error at the import.
    let found = found(
      "import { parse, Widget } from \"./vendor/a/mod.ts\";\nconsole.log(parse, new Widget());\n",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].start, (0, 16));
    assert!(
      found[0].message.starts_with(
        "Cannot import \"Widget\" from \"file:///app/vendor/a/mod.ts\": it is a class, which can't leave the sandbox of isolate[\"./vendor/a/\"]."
      ),
      "{}",
      found[0].message
    );
  }
}
