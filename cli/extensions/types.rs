// Copyright 2018-2026 the Ende authors. MIT license.

//! Types of `extension:@scope/name` imports for the type checker.
//!
//! At run time an `extension:` import is a generated proxy (see `registry`).
//! For type checking, the module graph gets a TypeScript stub in its place
//! that imports the extension's source for types only and declares each
//! export as the proxy presents it: the same parameters, the result wrapped
//! in a `Promise`, since every call crosses to the extension's worker.
//!
//! The stub's exports are the names the manifest reader finds, which needs
//! the source module, so the graph is built twice: first with a stub that
//! only imports the source (which brings it into the graph), then again with
//! the full stub.
//!
//! `extension:@scope/name` redirects to the stub at
//! `extension:///@scope/name.ts`: TypeScript reads a scheme without `//` as
//! a relative path, so the stub needs the rooted form to resolve its own
//! imports.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;

use deno_ast::ModuleSpecifier;
use deno_graph::Module;
use deno_graph::ModuleGraph;
use deno_graph::source::CacheInfo;
use deno_graph::source::LoadFuture;
use deno_graph::source::LoadOptions;
use deno_graph::source::LoadResponse;
use deno_graph::source::Loader;
use ende_config::ende_json::ExtensionsConfig;
use ende_core::futures::FutureExt;

use super::EXTENSION_SCHEME;
use super::manifest::extract_manifest;

/// What is known about an extension's exports.
#[derive(Debug, Clone)]
enum Exports {
  /// Not read yet: the stub only brings the source into the graph.
  Pending,
  /// The manifest reader's export names.
  Known(Vec<String>),
  /// The source couldn't be read as an extension; the import stays untyped
  /// (`any`), and running reports the real error.
  Untyped,
}

/// Stubs for every declared extension, for one graph build.
pub struct ExtensionTypes {
  config: ExtensionsConfig,
  exports: RefCell<HashMap<String, Exports>>,
}

impl ExtensionTypes {
  /// `None` when no extension is declared.
  pub fn new(config: ExtensionsConfig) -> Option<Self> {
    if config.extensions.is_empty() {
      return None;
    }
    Some(Self {
      config,
      exports: Default::default(),
    })
  }

  pub fn loader<'a>(
    &'a self,
    inner: &'a dyn Loader,
  ) -> ExtensionTypesLoader<'a> {
    ExtensionTypesLoader { types: self, inner }
  }

  /// After a build, reads the exports of each stub still pending from its
  /// source module in `graph`, and returns the stubs to load again.
  pub fn resolve_exports(&self, graph: &ModuleGraph) -> Vec<ModuleSpecifier> {
    let mut reload = Vec::new();
    for (name, extension) in &self.config.extensions {
      let Some(stub) = stub_specifier(name) else {
        continue;
      };
      if !matches!(graph.get(&stub), Some(Module::Js(_))) {
        continue;
      }
      if !matches!(
        self.exports.borrow().get(name.as_str()),
        None | Some(Exports::Pending)
      ) {
        continue;
      }
      let exports = graph
        .resolve_dependency(extension.source.as_str(), &stub, true)
        .and_then(|source| graph.get(source))
        .and_then(|module| match module {
          Module::Js(module) => extract_manifest(
            name,
            &module.specifier,
            module.media_type,
            &module.source.text,
          )
          .ok(),
          _ => None,
        })
        .map(|manifest| Exports::Known(manifest.exports))
        .unwrap_or(Exports::Untyped);
      self.exports.borrow_mut().insert(name.clone(), exports);
      reload.push(stub);
    }
    reload
  }

  fn load_stub(&self, specifier: &ModuleSpecifier) -> Option<LoadResponse> {
    if !specifier.path().starts_with('/') {
      // `extension:@scope/name`, as imported.
      let name = specifier.path();
      self.config.extensions.get(name)?;
      return Some(LoadResponse::Redirect {
        specifier: stub_specifier(name)?,
      });
    }
    let name = specifier
      .path()
      .strip_prefix('/')
      .and_then(|path| path.strip_suffix(".ts"))?;
    let extension = self.config.extensions.get(name)?;
    let exports = self
      .exports
      .borrow()
      .get(name)
      .cloned()
      .unwrap_or(Exports::Pending);
    let content = match exports {
      Exports::Untyped => {
        return Some(LoadResponse::External {
          specifier: specifier.clone(),
        });
      }
      Exports::Pending => stub_source(name, extension.source.as_str(), &[]),
      Exports::Known(exports) => {
        stub_source(name, extension.source.as_str(), &exports)
      }
    };
    Some(LoadResponse::Module {
      specifier: specifier.clone(),
      content: content.into_bytes().into(),
      mtime: None,
      maybe_headers: Some(HashMap::from([(
        "content-type".to_string(),
        "application/typescript".to_string(),
      )])),
    })
  }
}

/// Serves declared `extension:` stubs; everything else goes to `inner`.
pub struct ExtensionTypesLoader<'a> {
  types: &'a ExtensionTypes,
  inner: &'a dyn Loader,
}

impl Loader for ExtensionTypesLoader<'_> {
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
    if specifier.scheme() == EXTENSION_SCHEME
      && let Some(response) = self.types.load_stub(specifier)
    {
      return std::future::ready(Ok(Some(response))).boxed_local();
    }
    self.inner.load(specifier, options)
  }
}

/// `extension:@scope/name` for the type stub `extension:///@scope/name.ts`,
/// which a type-checked graph redirects to: running loads the proxy, never
/// the stub.
pub fn runtime_specifier(
  specifier: &ModuleSpecifier,
) -> Option<ModuleSpecifier> {
  if specifier.scheme() != EXTENSION_SCHEME {
    return None;
  }
  let name = specifier.path().strip_prefix('/')?.strip_suffix(".ts")?;
  ModuleSpecifier::parse(&format!("{EXTENSION_SCHEME}:{name}")).ok()
}

/// The type stub of extension `name` from its entry module's text, or
/// `None` when the text isn't a valid extension entry.
pub fn stub_for_source(
  name: &str,
  source: &ModuleSpecifier,
  text: &str,
) -> Option<String> {
  let manifest = extract_manifest(
    name,
    source,
    deno_ast::MediaType::from_specifier(source),
    text,
  )
  .ok()?;
  Some(stub_source(name, source.as_str(), &manifest.exports))
}

/// Where the stub of extension `name` lives in the graph.
fn stub_specifier(name: &str) -> Option<ModuleSpecifier> {
  ModuleSpecifier::parse(&format!("{EXTENSION_SCHEME}:///{name}.ts")).ok()
}

/// The stub's TypeScript. Arguments and results cross by structured clone,
/// which the types don't model: a function argument type-checks but fails
/// when called.
fn stub_source(name: &str, source: &str, exports: &[String]) -> String {
  let source = ende_core::serde_json::to_string(source).unwrap();
  let mut text = format!(
    "// Types of extension \"{name}\" as its proxy presents them: every call\n\
     // crosses to the extension's worker, so every result is a Promise.\n\
     import type * as __extension from {source};\n\
     type __Proxied<F> = F extends (...args: infer A) => infer R\n  \
     ? (...args: A) => Promise<Awaited<R>>\n  : never;\n"
  );
  for export in exports {
    text.push_str(&format!(
      "export declare const {export}: __Proxied<typeof __extension.{export}>;\n"
    ));
  }
  if exports.is_empty() {
    text.push_str("export {};\n");
  }
  text
}

/// Graph modules that only extensions use, as the URL strings diagnostics
/// name: what's reachable from an extension stub but not from the app's
/// roots without passing through one. An extension's own type errors are
/// its author's to fix, so they aren't reported when the app is checked.
pub fn extension_only_modules(graph: &ModuleGraph) -> HashSet<String> {
  let stubs = graph
    .modules()
    .filter(|m| m.specifier().scheme() == EXTENSION_SCHEME)
    .map(|m| m.specifier().clone())
    .collect::<Vec<_>>();
  if stubs.is_empty() {
    return HashSet::new();
  }
  let app = reachable(graph, graph.roots.iter().cloned(), false);
  let mut only = HashSet::new();
  for specifier in reachable(graph, stubs.into_iter(), true) {
    if specifier.scheme() != EXTENSION_SCHEME && !app.contains(&specifier) {
      only.insert(specifier.to_string());
    }
  }
  only
}

/// Modules reachable from `roots` through code and type dependencies;
/// `extension:` stubs are entered only when `into_extensions` is set.
fn reachable(
  graph: &ModuleGraph,
  roots: impl Iterator<Item = ModuleSpecifier>,
  into_extensions: bool,
) -> HashSet<ModuleSpecifier> {
  let mut seen = HashSet::new();
  let mut queue = roots.collect::<VecDeque<_>>();
  while let Some(specifier) = queue.pop_front() {
    let specifier = graph.resolve(&specifier).clone();
    if !seen.insert(specifier.clone()) {
      continue;
    }
    let Some(module) = graph.get(&specifier) else {
      continue;
    };
    let Module::Js(module) = module else {
      continue;
    };
    let mut push = |dep: Option<&ModuleSpecifier>| {
      if let Some(dep) = dep {
        if !into_extensions && dep.scheme() == EXTENSION_SCHEME {
          return;
        }
        queue.push_back(dep.clone());
      }
    };
    for dependency in module.dependencies.values() {
      push(dependency.get_code());
      push(dependency.get_type());
    }
    if let Some(types) = &module.maybe_types_dependency {
      push(types.dependency.maybe_specifier());
    }
  }
  seen
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn stub_wraps_each_export_in_a_promise() {
    let text = stub_source(
      "@acme/fmt",
      "file:///app/ext/fmt/mod.ts",
      &["shout".to_string(), "len".to_string()],
    );
    assert!(text.contains(
      "import type * as __extension from \"file:///app/ext/fmt/mod.ts\";"
    ));
    assert!(text.contains(
      "export declare const shout: __Proxied<typeof __extension.shout>;"
    ));
    assert!(text.contains(
      "export declare const len: __Proxied<typeof __extension.len>;"
    ));
    assert!(text.contains("Promise<Awaited<R>>"));
    assert!(!text.contains("export {};"));
  }

  #[test]
  fn runtime_specifier_maps_the_stub_back() {
    let stub = stub_specifier("@acme/fmt").unwrap();
    assert_eq!(stub.as_str(), "extension:///@acme/fmt.ts");
    assert_eq!(
      runtime_specifier(&stub).unwrap().as_str(),
      "extension:@acme/fmt"
    );
    let imported = ModuleSpecifier::parse("extension:@acme/fmt").unwrap();
    assert_eq!(runtime_specifier(&imported), None);
    let file = ModuleSpecifier::parse("file:///a/b.ts").unwrap();
    assert_eq!(runtime_specifier(&file), None);
  }

  #[test]
  fn pending_stub_only_imports_the_source() {
    let text = stub_source("@acme/fmt", "jsr:@acme/fmt@1", &[]);
    assert!(
      text.contains("import type * as __extension from \"jsr:@acme/fmt@1\";")
    );
    assert!(text.contains("export {};"));
    assert!(!text.contains("export declare"));
  }
}
