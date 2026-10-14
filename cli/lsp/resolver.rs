// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;
use deno_graph::ModuleSpecifier;
use deno_graph::Range;
use deno_semver::jsr::JsrPackageReqReference;
use deno_semver::package::PackageNv;
use deno_semver::package::PackageReq;
use ende_cache_dir::HttpCache;
use ende_core::parking_lot::Mutex;
use ende_core::url::Url;
use ende_resolver::EndeResolverOptions;
use ende_resolver::ende_json::CompilerOptionsResolver;
use ende_resolver::ende_json::JsxImportSourceConfig;
use ende_resolver::workspace::CreateResolverOptions;
use ende_resolver::workspace::FsCacheOptions;
use ende_resolver::workspace::ResolutionKind;
use ende_resolver::workspace::SloppyImportsOptions;
use ende_resolver::workspace::WorkspaceResolver;
use indexmap::IndexSet;

use super::cache::LspCache;
use super::jsr::JsrCacheResolver;
use crate::args::CliLockfile;
use crate::factory::Deferred;
use crate::lsp::config::Config;
use crate::lsp::config::ConfigData;
use crate::lsp::logging::lsp_warn;
use crate::resolver::CliResolver;
use crate::resolver::on_resolve_diagnostic;
use crate::sys::CliSys;

/// Information about how a specifier was resolved through an import map,
/// surfaced in hover tooltips.
#[derive(Debug, Clone)]
pub struct ImportMapHover {
  /// The matched key as written in the import map.
  pub key: String,
  /// The value the key maps to, as written in the import map.
  pub value: String,
  /// The location of the import map (e.g. the `ende.json`).
  pub base_url: Url,
}

#[derive(Debug, Clone)]
pub struct LspScopedResolver {
  resolver: Arc<CliResolver>,
  workspace_resolver: Arc<WorkspaceResolver<CliSys>>,
  jsr_resolver: Option<Arc<JsrCacheResolver>>,
  redirect_resolver: Option<Arc<RedirectResolver>>,
  dep_info: Arc<Mutex<Arc<ScopeDepInfo>>>,
  config_data: Option<Arc<ConfigData>>,
}

impl Default for LspScopedResolver {
  fn default() -> Self {
    let factory = ResolverFactory::new(None);
    Self {
      resolver: factory.cli_resolver().clone(),
      workspace_resolver: factory.workspace_resolver().clone(),
      jsr_resolver: None,
      redirect_resolver: None,
      dep_info: Default::default(),
      config_data: None,
    }
  }
}

impl LspScopedResolver {
  fn from_config_data(
    config_data: Option<&Arc<ConfigData>>,
    cache: &LspCache,
  ) -> Self {
    let factory = ResolverFactory::new(config_data);
    let workspace_resolver = factory.workspace_resolver().clone();
    let jsr_resolver = Some(Arc::new(JsrCacheResolver::new(
      cache.for_specifier(config_data.map(|d| d.scope.as_ref())),
      config_data.map(|d| d.as_ref()),
      &workspace_resolver,
    )));
    let redirect_resolver = Some(Arc::new(RedirectResolver::new(
      cache.for_specifier(config_data.map(|d| d.scope.as_ref())),
      config_data.and_then(|d| d.lockfile.clone()),
    )));
    Self {
      resolver: factory.cli_resolver().clone(),
      workspace_resolver,
      jsr_resolver,
      redirect_resolver,
      dep_info: Default::default(),
      config_data: config_data.cloned(),
    }
  }

  pub fn as_cli_resolver(&self) -> &CliResolver {
    self.resolver.as_ref()
  }

  pub fn as_workspace_resolver(&self) -> &Arc<WorkspaceResolver<CliSys>> {
    &self.workspace_resolver
  }

  pub fn jsr_to_resource_url(
    &self,
    req_ref: &JsrPackageReqReference,
  ) -> Option<ModuleSpecifier> {
    self.jsr_resolver.as_ref()?.jsr_to_resource_url(req_ref)
  }

  /// If `specifier` (as written at `referrer`) was remapped by the import map
  /// to `code`, returns the import map entry that was responsible. Used to
  /// surface import map resolution details in hover tooltips.
  ///
  /// This mirrors [`import_map::ImportMap::lookup`]: an entry is responsible for
  /// a resolution when its address equals (literal mapping) or is a prefix of
  /// (`/`-suffixed mapping) the resolved specifier. We additionally require that
  /// the entry reconstructs the exact specifier that was written, so we don't
  /// report an import map for e.g. a relative import that merely happens to land
  /// in a mapped directory.
  pub fn import_map_hover(
    &self,
    specifier: &str,
    code: &Url,
    referrer: &Url,
  ) -> Option<ImportMapHover> {
    let import_map = self.workspace_resolver.maybe_import_map()?;
    let code_str = code.as_str();
    for entry in import_map.entries_for_referrer(referrer) {
      let Some(address) = entry.value else {
        continue;
      };
      let address_str = address.as_str();
      let reconstructed = if address_str == code_str {
        entry.raw_key.to_string()
      } else if address_str.ends_with('/') && code_str.starts_with(address_str)
      {
        code_str.replace(address_str, entry.raw_key)
      } else {
        continue;
      };
      if reconstructed != specifier {
        continue;
      }
      let value = entry.raw_value.unwrap_or(address_str).to_string();
      return Some(ImportMapHover {
        key: entry.raw_key.to_string(),
        value,
        base_url: import_map.base_url().clone(),
      });
    }
    None
  }

  pub fn configured_auto_import_roots(&self) -> Vec<ModuleSpecifier> {
    let mut roots = IndexSet::new();
    if let Some(import_map) = self.workspace_resolver.maybe_import_map() {
      for entry in import_map.imports().entries().chain(
        import_map
          .scopes()
          .flat_map(|scope| scope.imports.entries()),
      ) {
        let Some(value) = entry.value else {
          continue;
        };
        match value.scheme() {
          "jsr" => {
            if let Ok(req_ref) = JsrPackageReqReference::from_specifier(value) {
              roots.insert(value.clone());
              if let Some(jsr_resolver) = &self.jsr_resolver {
                roots.extend(jsr_resolver.auto_import_resource_urls(&req_ref));
              }
            }
          }
          "file" => {
            if entry.key.ends_with('/') && value.as_str().ends_with('/') {
              continue;
            }
            if let Ok(path) = value.to_file_path()
              && !path.is_file()
            {
              continue;
            }
            roots.insert(value.clone());
          }
          "http" | "https" => {
            if entry.key.ends_with('/') && value.as_str().ends_with('/') {
              continue;
            }
            roots.insert(value.clone());
          }
          _ => {}
        }
      }
    }
    roots.into_iter().collect()
  }

  pub fn jsr_lookup_bare_specifier_for_workspace_file(
    &self,
    specifier: &Url,
  ) -> Option<String> {
    self
      .jsr_resolver
      .as_ref()?
      .lookup_bare_specifier_for_workspace_file(specifier)
  }

  pub fn jsr_lookup_export_for_path(
    &self,
    nv: &PackageNv,
    path: &str,
  ) -> Option<String> {
    self.jsr_resolver.as_ref()?.lookup_export_for_path(nv, path)
  }

  pub fn jsr_lookup_req_for_nv(&self, nv: &PackageNv) -> Option<PackageReq> {
    self.jsr_resolver.as_ref()?.lookup_req_for_nv(nv)
  }

  pub fn ende_types_to_code_resolution(
    &self,
    specifier: &ModuleSpecifier,
  ) -> Option<ModuleSpecifier> {
    let dep_info = self.dep_info.lock();
    dep_info
      .ende_types_to_code_resolutions
      .get(specifier)
      .cloned()
  }

  pub fn in_node_modules(&self, specifier: &ModuleSpecifier) -> bool {
    // consider any /node_modules/ directory as being in the node_modules
    // folder for the LSP because it's pretty complicated to deal with multiple scopes
    specifier.scheme() == "file"
      && specifier
        .path()
        .to_ascii_lowercase()
        .contains("/node_modules/")
  }

  pub fn resolve_redirects(
    &self,
    specifier: &ModuleSpecifier,
  ) -> Option<ModuleSpecifier> {
    let Some(redirect_resolver) = self.redirect_resolver.as_ref() else {
      return Some(specifier.clone());
    };
    redirect_resolver.resolve(specifier)
  }

  pub fn redirect_chain_headers(
    &self,
    specifier: &ModuleSpecifier,
  ) -> Vec<(ModuleSpecifier, Arc<HashMap<String, String>>)> {
    let Some(redirect_resolver) = self.redirect_resolver.as_ref() else {
      return vec![];
    };
    redirect_resolver
      .chain(specifier)
      .into_iter()
      .map(|(s, e)| (s, e.headers.clone()))
      .collect()
  }
}

#[derive(Debug, Default, Clone)]
pub struct LspResolver {
  unscoped: Arc<LspScopedResolver>,
  by_scope: BTreeMap<Arc<Url>, Arc<LspScopedResolver>>,
}

impl LspResolver {
  pub fn from_config(config: &Config, cache: &LspCache) -> Self {
    let mut by_scope = BTreeMap::new();
    for (scope, config_data) in config.tree.data_by_scope().as_ref() {
      by_scope.insert(
        scope.clone(),
        Arc::new(LspScopedResolver::from_config_data(
          Some(config_data),
          cache,
        )),
      );
    }
    let unscoped = Arc::new(LspScopedResolver::from_config_data(None, cache));
    Self { unscoped, by_scope }
  }

  pub fn set_compiler_options_resolver(
    &self,
    value: &Arc<CompilerOptionsResolver>,
  ) {
    for resolver in
      std::iter::once(&self.unscoped).chain(self.by_scope.values())
    {
      resolver
        .workspace_resolver
        .set_compiler_options_resolver(value.clone());
    }
  }

  pub fn snapshot(&self) -> Arc<Self> {
    Arc::new(self.clone())
  }

  pub fn did_cache(&self) {
    for resolver in
      std::iter::once(&self.unscoped).chain(self.by_scope.values())
    {
      resolver.jsr_resolver.as_ref().inspect(|r| r.did_cache());
      resolver
        .redirect_resolver
        .as_ref()
        .inspect(|r| r.did_cache());
    }
  }

  pub fn set_dep_info_by_scope(
    &self,
    dep_info_by_scope: &Arc<BTreeMap<Option<Arc<Url>>, Arc<ScopeDepInfo>>>,
  ) {
    for (scope, resolver) in [(None, &self.unscoped)]
      .into_iter()
      .chain(self.by_scope.iter().map(|(s, r)| (Some(s), r)))
    {
      let dep_info = dep_info_by_scope
        .get(&scope.cloned())
        .cloned()
        .unwrap_or_default();
      {
        let mut resolver_dep_info = resolver.dep_info.lock();
        *resolver_dep_info = dep_info.clone();
      }
    }
  }

  pub fn in_node_modules(&self, specifier: &ModuleSpecifier) -> bool {
    self
      .get_scoped_resolver(Some(specifier))
      .in_node_modules(specifier)
  }

  pub fn get_scoped_resolver(
    &self,
    file_referrer: Option<&ModuleSpecifier>,
  ) -> &LspScopedResolver {
    let Some(file_referrer) = file_referrer else {
      return self.unscoped.as_ref();
    };
    self
      .by_scope
      .values()
      .rfind(|r| {
        r.config_data
          .as_ref()
          .map(|d| d.scope_contains_specifier(file_referrer))
          .unwrap_or(false)
      })
      .map(|r| r.as_ref())
      .unwrap_or(self.unscoped.as_ref())
  }
}

#[derive(Debug, Default, Clone)]
pub struct ScopeDepInfo {
  pub ende_types_to_code_resolutions: HashMap<ModuleSpecifier, ModuleSpecifier>,
}

#[derive(Default)]
struct ResolverFactoryServices {
  cli_resolver: Deferred<Arc<CliResolver>>,
  workspace_resolver: Deferred<Arc<WorkspaceResolver<CliSys>>>,
}

struct ResolverFactory<'a> {
  config_data: Option<&'a Arc<ConfigData>>,
  sys: CliSys,
  services: ResolverFactoryServices,
}

impl<'a> ResolverFactory<'a> {
  pub fn new(config_data: Option<&'a Arc<ConfigData>>) -> Self {
    Self {
      config_data,
      sys: CliSys::default(),
      services: Default::default(),
    }
  }

  pub fn cli_resolver(&self) -> &Arc<CliResolver> {
    self.services.cli_resolver.get_or_init(|| {
      let ende_resolver =
        Arc::new(ende_resolver::RawEndeResolver::new(EndeResolverOptions {
          workspace_resolver: self.workspace_resolver().clone(),
          maybe_vendor_dir: self
            .config_data
            .and_then(|d| d.vendor_dir.as_ref()),
        }));
      Arc::new(CliResolver::new(
        ende_resolver,
        Some(Arc::new(on_resolve_diagnostic)),
      ))
    })
  }

  pub fn workspace_resolver(&self) -> &Arc<WorkspaceResolver<CliSys>> {
    self.services.workspace_resolver.get_or_init(|| {
      let workspace_resolver = self
        .config_data
        .map(|d| {
          let unstable_sloppy_imports =
            ende_env::var("ENDE_UNSTABLE_SLOPPY_IMPORTS").is_ok()
              || d.unstable.contains("sloppy-imports");
          WorkspaceResolver::from_workspace(
            &d.member_dir.workspace,
            CliSys::default(),
            CreateResolverOptions {
              specified_import_map: d.specified_import_map.clone(),
              sloppy_imports_options: if unstable_sloppy_imports {
                SloppyImportsOptions::Enabled
              } else {
                SloppyImportsOptions::Unspecified
              },
              fs_cache_options: FsCacheOptions::Disabled,
            },
          )
          .inspect_err(|err| {
            lsp_warn!(
              "Failed to load resolver: {err}", // will contain the specifier
            );
          })
          .ok()
          .unwrap_or_else(|| {
            // create a dummy resolver
            WorkspaceResolver::new_raw(
              None,
              d.member_dir.workspace.resolver_jsr_pkgs().collect(),
              Default::default(),
              Default::default(),
              CliSys::default(),
            )
          })
        })
        .unwrap_or_else(|| {
          WorkspaceResolver::new_raw(
            None,
            Vec::new(),
            Default::default(),
            Default::default(),
            self.sys.clone(),
          )
        });
      let diagnostics = workspace_resolver.diagnostics();
      if !diagnostics.is_empty() {
        lsp_warn!(
          "Workspace resolver diagnostics ({}):\n{}",
          self
            .config_data
            .map(|d| d.scope.as_str())
            .unwrap_or("null scope"),
          diagnostics
            .iter()
            .map(|d| format!("  - {d}"))
            .collect::<Vec<_>>()
            .join("\n")
        );
      }
      Arc::new(workspace_resolver)
    })
  }
}

#[derive(Debug, Eq, PartialEq)]
struct RedirectEntry {
  headers: Arc<HashMap<String, String>>,
  target: Url,
  destination: Option<Url>,
}

type GetHeadersFn =
  Box<dyn Fn(&Url) -> Option<HashMap<String, String>> + Send + Sync>;

struct RedirectResolver {
  get_headers: GetHeadersFn,
  entries: DashMap<Url, Option<Arc<RedirectEntry>>>,
}

impl std::fmt::Debug for RedirectResolver {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("RedirectResolver")
      .field("get_headers", &"Box(|_| { ... })")
      .field("entries", &self.entries)
      .finish()
  }
}

#[derive(Debug)]
pub struct SingleReferrerGraphResolver<'a> {
  pub valid_referrer: &'a ModuleSpecifier,
  pub cli_resolver: &'a CliResolver,
  pub jsx_import_source_config: Option<&'a JsxImportSourceConfig>,
}

impl deno_graph::source::Resolver for SingleReferrerGraphResolver<'_> {
  fn default_jsx_import_source(
    &self,
    _referrer: &ModuleSpecifier,
  ) -> Option<String> {
    self
      .jsx_import_source_config
      .and_then(|c| c.import_source.as_ref().map(|s| s.specifier.clone()))
  }

  fn default_jsx_import_source_types(
    &self,
    _referrer: &ModuleSpecifier,
  ) -> Option<String> {
    self
      .jsx_import_source_config
      .and_then(|c| c.import_source_types.as_ref().map(|s| s.specifier.clone()))
  }

  fn jsx_import_source_module(&self, _referrer: &ModuleSpecifier) -> &str {
    self
      .jsx_import_source_config
      .map(|c| c.module.as_str())
      .unwrap_or(deno_graph::source::DEFAULT_JSX_IMPORT_SOURCE_MODULE)
  }

  fn resolve(
    &self,
    specifier_text: &str,
    referrer_range: &Range,
    resolution_kind: deno_graph::source::ResolutionKind,
  ) -> Result<ModuleSpecifier, deno_graph::source::ResolveError> {
    // this resolver assumes it will only be used with a single referrer
    // with the provided referrer kind
    debug_assert_eq!(referrer_range.specifier, *self.valid_referrer);
    self
      .cli_resolver
      .resolve(
        specifier_text,
        &referrer_range.specifier,
        referrer_range.range.start,
        ResolutionKind::from_deno_graph(resolution_kind),
      )
      .map_err(|err| err.into_deno_graph_error())
  }
}

impl RedirectResolver {
  fn new(
    cache: Arc<dyn HttpCache>,
    lockfile: Option<Arc<CliLockfile>>,
  ) -> Self {
    let entries = DashMap::new();
    if let Some(lockfile) = lockfile {
      for (source, destination) in &lockfile.lock().content.redirects {
        let Ok(source) = ModuleSpecifier::parse(source) else {
          continue;
        };
        let Ok(destination) = ModuleSpecifier::parse(destination) else {
          continue;
        };
        entries.insert(
          source,
          Some(Arc::new(RedirectEntry {
            headers: Default::default(),
            target: destination.clone(),
            destination: Some(destination.clone()),
          })),
        );
        entries.insert(destination, None);
      }
    }
    Self {
      get_headers: Box::new(move |specifier| {
        let cache_key = cache.cache_item_key(specifier).ok()?;
        cache.read_headers(&cache_key).ok().flatten()
      }),
      entries,
    }
  }

  #[cfg(test)]
  fn mock(get_headers: GetHeadersFn) -> Self {
    Self {
      get_headers,
      entries: Default::default(),
    }
  }

  fn resolve(&self, specifier: &Url) -> Option<Url> {
    if !matches!(specifier.scheme(), "http" | "https") {
      return Some(specifier.clone());
    }
    let mut current = specifier.clone();
    let mut chain = vec![];
    let destination = loop {
      if let Some(maybe_entry) = self.entries.get(&current) {
        break match maybe_entry.as_ref() {
          Some(entry) => entry.destination.clone(),
          None => Some(current),
        };
      }
      let Some(headers) = (self.get_headers)(&current) else {
        break None;
      };
      let headers = Arc::new(headers);
      if let Some(location) = headers.get("location") {
        if chain.len() > 10 {
          break None;
        }
        let Ok(target) = specifier.join(location) else {
          break None;
        };
        chain.push((
          current.clone(),
          RedirectEntry {
            headers,
            target: target.clone(),
            destination: None,
          },
        ));
        current = target;
      } else {
        self.entries.insert(current.clone(), None);
        break Some(current);
      }
    };
    for (specifier, mut entry) in chain {
      entry.destination.clone_from(&destination);
      self.entries.insert(specifier, Some(Arc::new(entry)));
    }
    destination
  }

  fn chain(&self, specifier: &Url) -> Vec<(Url, Arc<RedirectEntry>)> {
    self.resolve(specifier);
    let mut result = vec![];
    let mut seen = HashSet::new();
    let mut current = Cow::Borrowed(specifier);
    loop {
      let Some(maybe_entry) = self.entries.get(&current) else {
        break;
      };
      let Some(entry) = maybe_entry.as_ref() else {
        break;
      };
      result.push((current.as_ref().clone(), entry.clone()));
      seen.insert(current.as_ref().clone());
      if seen.contains(&entry.target) {
        break;
      }
      current = Cow::Owned(entry.target.clone())
    }
    result
  }

  fn did_cache(&self) {
    self.entries.retain(|_, entry| entry.is_some());
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_redirect_resolver() {
    let redirect_resolver =
      RedirectResolver::mock(Box::new(|specifier| match specifier.as_str() {
        "https://foo/redirect_2.js" => Some(
          [("location".to_string(), "./redirect_1.js".to_string())]
            .into_iter()
            .collect(),
        ),
        "https://foo/redirect_1.js" => Some(
          [("location".to_string(), "./file.js".to_string())]
            .into_iter()
            .collect(),
        ),
        "https://foo/file.js" => Some([].into_iter().collect()),
        _ => None,
      }));
    assert_eq!(
      redirect_resolver.resolve(&Url::parse("https://foo/file.js").unwrap()),
      Some(Url::parse("https://foo/file.js").unwrap())
    );
    assert_eq!(
      redirect_resolver
        .resolve(&Url::parse("https://foo/redirect_1.js").unwrap()),
      Some(Url::parse("https://foo/file.js").unwrap())
    );
    assert_eq!(
      redirect_resolver
        .resolve(&Url::parse("https://foo/redirect_2.js").unwrap()),
      Some(Url::parse("https://foo/file.js").unwrap())
    );
    assert_eq!(
      redirect_resolver.resolve(&Url::parse("https://foo/unknown").unwrap()),
      None
    );
    assert_eq!(
      redirect_resolver
        .chain(&Url::parse("https://foo/redirect_2.js").unwrap()),
      vec![
        (
          Url::parse("https://foo/redirect_2.js").unwrap(),
          Arc::new(RedirectEntry {
            headers: Arc::new(
              [("location".to_string(), "./redirect_1.js".to_string())]
                .into_iter()
                .collect()
            ),
            target: Url::parse("https://foo/redirect_1.js").unwrap(),
            destination: Some(Url::parse("https://foo/file.js").unwrap()),
          })
        ),
        (
          Url::parse("https://foo/redirect_1.js").unwrap(),
          Arc::new(RedirectEntry {
            headers: Arc::new(
              [("location".to_string(), "./file.js".to_string())]
                .into_iter()
                .collect()
            ),
            target: Url::parse("https://foo/file.js").unwrap(),
            destination: Some(Url::parse("https://foo/file.js").unwrap()),
          })
        ),
      ]
    );
  }
}
