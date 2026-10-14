// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use deno_ast::ModuleSpecifier;
use deno_graph::JsrPackageReqNotFoundError;
use deno_graph::packages::JsrPackageVersionInfo;
use deno_semver::StackString;
use deno_semver::Version;
use deno_semver::VersionReq;
use deno_semver::jsr::JsrDepPackageReq;
use deno_semver::jsr::JsrPackageReqReference;
use deno_semver::package::PackageNv;
use deno_semver::package::PackageReq;
use ende_config::ende_json::ConfigFile;
use ende_config::ende_json::ConfigFileRc;
use ende_config::workspace::Workspace;
use ende_config::workspace::WorkspaceDirectory;
use ende_core::anyhow::Context;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::futures::FutureExt;
use ende_core::futures::StreamExt;
use ende_core::futures::future::try_join;
use ende_core::futures::stream::FuturesOrdered;
use ende_core::futures::stream::FuturesUnordered;
use ende_core::serde_json;
use ende_core::url::Url;
use ende_runtime::ende_permissions::PermissionsContainer;
use import_map::ImportMap;
use import_map::ImportMapWithDiagnostics;
use import_map::SpecifierMapEntry;
use tokio::sync::Semaphore;

use super::ConfigUpdater;
use crate::args::CliLockfile;
use crate::graph_container::MainModuleGraphContainer;
use crate::graph_container::ModuleGraphContainer;
use crate::graph_container::ModuleGraphUpdatePermit;
use crate::jsr::JsrFetchResolver;
use crate::jsr::PackageInfoLoadError;
use crate::module_loader::ModuleLoadPreparer;
use crate::util::progress_bar::ProgressBar;
use crate::util::sync::AtomicFlag;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportMapKind {
  Inline,
  Outline(PathBuf),
}

/// Where a dependency is declared: an ende.json import map entry.
#[derive(Clone)]
pub struct DepLocation {
  ende_json: ConfigFileRc,
  key_path: KeyPath,
  import_map_kind: ImportMapKind,
}

impl DepLocation {
  pub fn file_path(&self) -> Cow<'_, std::path::Path> {
    match &self.import_map_kind {
      ImportMapKind::Inline => {
        Cow::Owned(self.ende_json.specifier.to_file_path().unwrap())
      }
      ImportMapKind::Outline(path) => Cow::Borrowed(path.as_path()),
    }
  }
}

struct DebugAdapter<T>(T);

impl std::fmt::Debug for DebugAdapter<&ConfigFileRc> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ConfigFile")
      .field("specifier", &self.0.specifier)
      .finish()
  }
}
impl std::fmt::Debug for DepLocation {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("DepLocation")
      .field("ende_json", &DebugAdapter(&self.ende_json))
      .field("key_path", &self.key_path)
      .field("import_map_kind", &self.import_map_kind)
      .finish()
  }
}

#[derive(Clone, Debug)]
pub enum KeyPart {
  Imports,
  Scopes,
  String(StackString),
}

impl KeyPart {
  pub fn as_str(&self) -> &str {
    match self {
      KeyPart::Imports => "imports",
      KeyPart::Scopes => "scopes",
      KeyPart::String(s) => s.as_str(),
    }
  }
}

#[derive(Clone, Debug)]
pub struct KeyPath {
  pub parts: Vec<KeyPart>,
}

impl KeyPath {
  fn from_parts(parts: impl IntoIterator<Item = KeyPart>) -> Self {
    Self {
      parts: parts.into_iter().collect(),
    }
  }
  fn last(&self) -> Option<&KeyPart> {
    self.parts.last()
  }
  fn push(&mut self, part: KeyPart) {
    self.parts.push(part)
  }
}

#[derive(Clone, Debug)]
pub struct Dep {
  pub req: PackageReq,
  pub location: DepLocation,
  #[allow(dead_code, reason = "not used yet")]
  pub id: DepId,
  #[allow(dead_code, reason = "not used yet")]
  pub alias: Option<String>,
}

impl Dep {
  pub fn alias_or_name(&self) -> &str {
    self.alias.as_deref().unwrap_or_else(|| &self.req.name)
  }
}

fn import_map_entries(
  import_map: &ImportMap,
) -> impl Iterator<Item = (KeyPath, SpecifierMapEntry<'_>)> {
  import_map
    .imports()
    .entries()
    .map(|entry| {
      (
        KeyPath::from_parts([
          KeyPart::Imports,
          KeyPart::String(entry.raw_key.into()),
        ]),
        entry,
      )
    })
    .chain(import_map.scopes().flat_map(|scope| {
      let path = KeyPath::from_parts([
        KeyPart::Scopes,
        KeyPart::String(scope.raw_key.into()),
      ]);

      scope.imports.entries().map(move |entry| {
        let mut full_path = path.clone();
        full_path.push(KeyPart::String(entry.raw_key.into()));
        (full_path, entry)
      })
    }))
}

fn to_import_map_value_from_imports(
  ende_json: &ConfigFile,
) -> serde_json::Value {
  let mut value = serde_json::Map::with_capacity(2);
  if let Some(imports) = &ende_json.json.imports {
    value.insert("imports".to_string(), imports.clone());
  }
  if let Some(scopes) = &ende_json.json.scopes {
    value.insert("scopes".to_string(), scopes.clone());
  }
  serde_json::Value::Object(value)
}

fn ende_json_import_map(
  ende_json: &ConfigFile,
) -> Result<Option<(ImportMapWithDiagnostics, ImportMapKind)>, AnyError> {
  let (value, kind) = if ende_json.json.imports.is_some()
    || ende_json.json.scopes.is_some()
  {
    (
      to_import_map_value_from_imports(ende_json),
      ImportMapKind::Inline,
    )
  } else {
    match ende_json.to_import_map_path()? {
      Some(path) => {
        let err_context = || {
          format!(
            "loading import map at '{}' (from \"importMap\" field in '{}')",
            path.display(),
            ende_json.specifier
          )
        };
        let text = std::fs::read_to_string(&path).with_context(err_context)?;
        let value = serde_json::from_str(&text).with_context(err_context)?;
        (value, ImportMapKind::Outline(path))
      }
      None => return Ok(None),
    }
  };

  import_map::parse_from_value(ende_json.specifier.clone(), value)
    .map_err(Into::into)
    .map(|import_map| Some((import_map, kind)))
}

fn add_deps_from_ende_json(
  ende_json: &Arc<ConfigFile>,
  mut filter: impl DepFilter,
  deps: &mut Vec<Dep>,
) {
  let (import_map, import_map_kind) = match ende_json_import_map(ende_json) {
    Ok(Some((import_map, import_map_kind))) => (import_map, import_map_kind),
    Ok(None) => return,
    Err(e) => {
      log::warn!("failed to parse imports from {}: {e}", &ende_json.specifier);
      return;
    }
  };
  for (key_path, entry) in import_map_entries(&import_map.import_map) {
    let Some(value) = entry.value else { continue };
    // npm packages are not supported in Ende, so they're ignored
    if value.scheme() != "jsr" {
      continue;
    }
    let req = match JsrPackageReqReference::from_str(value.as_str()) {
      Ok(req) => req.into_inner().req,
      Err(err) => {
        log::warn!("failed to parse package req \"{}\": {err}", value.as_str());
        continue;
      }
    };
    let alias: &str = key_path.last().unwrap().as_str().trim_end_matches('/');
    let alias = (alias != req.name).then(|| alias.to_string());
    if !filter.should_include(alias.as_deref(), &req) {
      continue;
    }
    let id = DepId(deps.len());
    deps.push(Dep {
      location: DepLocation {
        ende_json: ende_json.clone(),
        key_path,
        import_map_kind: import_map_kind.clone(),
      },
      req,
      id,
      alias,
    });
  }
}

fn deps_from_workspace(
  workspace: &Arc<Workspace>,
  dep_filter: impl DepFilter,
) -> Result<Vec<Dep>, AnyError> {
  let mut deps = Vec::with_capacity(256);
  for ende_json in workspace.ende_jsons() {
    add_deps_from_ende_json(ende_json, dep_filter, &mut deps);
  }

  Ok(deps)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DepId(usize);

#[derive(Debug, Clone)]
pub enum Change {
  Update(DepId, VersionReq),
}

pub trait DepFilter: Copy {
  fn should_include(
    &mut self,
    alias: Option<&str>,
    package_req: &PackageReq,
  ) -> bool;
}

impl<T> DepFilter for T
where
  T: FnMut(Option<&str>, &PackageReq) -> bool + Copy,
{
  fn should_include<'a>(
    &mut self,
    alias: Option<&'a str>,
    package_req: &'a PackageReq,
  ) -> bool {
    (*self)(alias, package_req)
  }
}

#[derive(Clone, Debug)]
pub struct PackageLatestVersion {
  pub semver_compatible: Option<PackageNv>,
  pub latest: Option<PackageNv>,
  /// Set when fetching the package's metadata failed (e.g. an unreachable or
  /// unauthorized private registry). The package is dropped from the outdated
  /// table, so this is used to warn the user instead of skipping silently.
  pub fetch_error: Option<Arc<PackageInfoLoadError>>,
}

pub struct DepManager {
  deps: Vec<Dep>,
  resolved_versions: Vec<Option<PackageNv>>,
  latest_versions: Vec<PackageLatestVersion>,

  pending_changes: Vec<Change>,

  dependencies_resolved: AtomicFlag,
  module_load_preparer: Arc<ModuleLoadPreparer>,
  // TODO(nathanwhit): probably shouldn't be pub
  pub(crate) jsr_fetch_resolver: Arc<JsrFetchResolver>,
  permissions_container: PermissionsContainer,
  progress_bar: ProgressBar,
  main_module_graph_container: Arc<MainModuleGraphContainer>,
  lockfile: Option<Arc<CliLockfile>>,
}

pub struct DepManagerArgs {
  pub module_load_preparer: Arc<ModuleLoadPreparer>,
  pub jsr_fetch_resolver: Arc<JsrFetchResolver>,
  pub permissions_container: PermissionsContainer,
  pub progress_bar: ProgressBar,
  pub main_module_graph_container: Arc<MainModuleGraphContainer>,
  pub lockfile: Option<Arc<CliLockfile>>,
}

impl DepManager {
  pub fn reloaded_after_modification(self, args: DepManagerArgs) -> Self {
    let mut new = Self::with_deps_args(self.deps, args);
    new.latest_versions = self.latest_versions;
    new
  }

  fn with_deps_args(deps: Vec<Dep>, args: DepManagerArgs) -> Self {
    let DepManagerArgs {
      module_load_preparer,
      jsr_fetch_resolver,
      progress_bar,
      permissions_container,
      main_module_graph_container,
      lockfile,
    } = args;
    Self {
      deps,
      resolved_versions: Vec::new(),
      latest_versions: Vec::new(),
      jsr_fetch_resolver,
      dependencies_resolved: AtomicFlag::lowered(),
      module_load_preparer,
      progress_bar,
      permissions_container,
      main_module_graph_container,
      lockfile,
      pending_changes: Vec::new(),
    }
  }

  pub fn from_workspace_dir(
    workspace_dir: &Arc<WorkspaceDirectory>,
    dep_filter: impl DepFilter,
    args: DepManagerArgs,
  ) -> Result<Self, AnyError> {
    let mut deps = Vec::with_capacity(256);
    if let Some(ende_json) = workspace_dir.member_ende_json() {
      if ende_json.specifier.scheme() != "file" {
        bail!("remote ende.json files are not supported");
      }
      let path = ende_json.specifier.to_file_path().unwrap();
      if path.parent().unwrap() == workspace_dir.dir_path() {
        add_deps_from_ende_json(ende_json, dep_filter, &mut deps);
      }
    }

    Ok(Self::with_deps_args(deps, args))
  }

  pub fn from_workspace(
    workspace: &Arc<Workspace>,
    dep_filter: impl DepFilter,
    args: DepManagerArgs,
  ) -> Result<Self, AnyError> {
    let deps = deps_from_workspace(workspace, dep_filter)?;
    Ok(Self::with_deps_args(deps, args))
  }

  async fn run_dependency_resolution(&self) -> Result<(), AnyError> {
    if self.dependencies_resolved.is_raised() {
      return Ok(());
    }
    let _clear_guard = self.progress_bar.deferred_keep_initialize_alive();

    let mut graph_permit = self
      .main_module_graph_container
      .acquire_update_permit()
      .await;
    let graph = graph_permit.graph_mut();
    // populate the information from the lockfile
    if let Some(lockfile) = &self.lockfile {
      lockfile.fill_graph(graph);
    }

    if self
      .deps
      .iter()
      .all(|dep| graph.packages.mappings().contains_key(&dep.req))
    {
      self.dependencies_resolved.raise();
      graph_permit.commit();
      return Ok(());
    }

    let mut roots = Vec::new();
    let mut info_futures = FuturesUnordered::new();
    for dep in &self.deps {
      // Note: a tagged req (e.g. `@latest`) never appears in the graph
      // mappings because the graph build rejects jsr version tags, so
      // the VersionReq::matches call below (which panics on tags)
      // cannot be reached with one.
      let resolved_nv = graph.packages.mappings().get(&dep.req);
      let resolved_nv = resolved_nv
        .and_then(|nv| {
          let versions = graph.packages.versions_by_name(&dep.req.name)?;
          let mut best = nv;
          for version in versions {
            if version.version > best.version
              && dep.req.version_req.matches(&version.version)
            {
              best = version;
            }
          }
          Some(best)
        })
        .cloned();
      info_futures.push(async {
        let nv = if let Some(nv) = resolved_nv {
          nv
        } else if let Some(nv) =
          self.jsr_fetch_resolver.req_to_nv(&dep.req).await?
        {
          nv
        } else {
          return Result::<
            Option<(Url, Arc<JsrPackageVersionInfo>)>,
            JsrPackageReqNotFoundError,
          >::Ok(None);
        };
        if let Some(info) =
          self.jsr_fetch_resolver.package_version_info(&nv).await
        {
          let specifier =
            ModuleSpecifier::parse(&format!("jsr:/{}/", &dep.req)).unwrap();
          return Ok(Some((specifier, info)));
        }
        Ok(None)
      });
    }

    while let Some(info_future) = info_futures.next().await {
      if let Some((specifier, info)) = info_future? {
        let exports = info.exports();
        for (k, _) in exports {
          if let Ok(spec) = specifier.join(k) {
            roots.push(spec);
          }
        }
      }
    }

    self
      .module_load_preparer
      .prepare_module_load(
        graph,
        &roots,
        crate::module_loader::PrepareModuleLoadOptions {
          is_dynamic: false,
          lib: ende_config::workspace::TsTypeLib::EndeWindow,
          permissions: self.permissions_container.clone(),
          file_permission_api_name: None,
          ext_overwrite: None,
          allow_unknown_media_types: true,
          skip_graph_roots_validation: false,
          file_content_overrides: Default::default(),
          file_header_overrides: Default::default(),
        },
      )
      .await?;

    self.dependencies_resolved.raise();
    graph_permit.commit();

    Ok(())
  }

  pub fn resolved_version(&self, id: DepId) -> Option<&PackageNv> {
    self.resolved_versions[id.0].as_ref()
  }

  pub async fn resolve_current_versions(&mut self) -> Result<(), AnyError> {
    self.run_dependency_resolution().await?;

    let graph = self.main_module_graph_container.graph();

    let resolved_jsr = graph.packages.mappings();
    self.resolved_versions = self
      .deps
      .iter()
      .map(|dep| resolved_jsr.get(&dep.req).cloned())
      .collect();

    Ok(())
  }

  async fn load_latest_versions(
    &self,
  ) -> Result<Vec<PackageLatestVersion>, AnyError> {
    if self.latest_versions.len() == self.deps.len() {
      return Ok(self.latest_versions.clone());
    }
    let mut latest_versions = Vec::with_capacity(self.deps.len());

    let jsr_sema = Semaphore::new(32);
    let mut futs = FuturesOrdered::new();

    for dep in &self.deps {
      futs.push_back(
        async {
          let semver_req = &dep.req;
          let _permit = jsr_sema.acquire().await;
          let semver_compatible = self
            .jsr_fetch_resolver
            .req_to_nv(semver_req)
            .await
            .ok()
            .flatten();
          let info_result = self
            .jsr_fetch_resolver
            .package_info_with_reason(&semver_req.name)
            .await;
          let fetch_error = info_result.as_ref().err().cloned();
          let latest = info_result
            .ok()
            .and_then(|info| {
              let version_resolver = self
                .jsr_fetch_resolver
                .version_resolver_for_package(&semver_req.name, &info);
              let lower_bound = &semver_compatible.as_ref()?.version;
              // Only consider pre-release versions when the currently
              // resolved version is itself a pre-release. Otherwise
              // `ende outdated --latest` reports a pre-release (e.g.
              // 2.0.3-alpha.0) as the latest version even though JSR
              // designates the newest stable release as latest. This mirrors
              // npm packages, which resolve latest from the `latest`
              // dist-tag. (#36320)
              //
              // JSR's `meta.json` also has a `latest` field (exposed as
              // `info.latest`), but it can't replace the scan below on its
              // own: it never points at a pre-release, so a project already
              // tracking a pre-release line would stop being offered newer
              // pre-releases, and it may fall outside the
              // `minimumDependencyAge` window, in which case we still want
              // the newest version that is old enough.
              let allow_pre_release = !lower_bound.pre.is_empty();
              {
                let mut best: Option<&Version> = Some(lower_bound);
                for version in
                  info.versions.iter().filter_map(|(version, version_info)| {
                    if !version_info.yanked
                      && (allow_pre_release || version.pre.is_empty())
                      && version_resolver
                        .matches_newest_dependency_date(version_info)
                    {
                      Some(version)
                    } else {
                      None
                    }
                  })
                {
                  if best.is_none_or(|b| version > b) {
                    best = Some(version);
                  }
                }
                best.cloned()
              }
            })
            .map(|version| PackageNv {
              name: semver_req.name.clone(),
              version,
            });
          PackageLatestVersion {
            latest,
            semver_compatible,
            fetch_error,
          }
        }
        .boxed_local(),
      );
    }
    while let Some(nv) = futs.next().await {
      latest_versions.push(nv);
    }

    Ok(latest_versions)
  }

  pub async fn resolve_versions(&mut self) -> Result<(), AnyError> {
    let (_, latest_versions) = try_join(
      self.run_dependency_resolution(),
      self.load_latest_versions(),
    )
    .await?;

    self.latest_versions = latest_versions;

    self.resolve_current_versions().await?;

    Ok(())
  }

  pub fn deps_with_resolved_latest_versions(
    &self,
  ) -> impl IntoIterator<Item = (DepId, Option<PackageNv>, PackageLatestVersion)> + '_
  {
    self
      .resolved_versions
      .iter()
      .zip(self.latest_versions.iter())
      .enumerate()
      .map(|(i, (resolved, latest))| {
        (DepId(i), resolved.clone(), latest.clone())
      })
  }

  pub fn get_dep(&self, id: DepId) -> &Dep {
    &self.deps[id.0]
  }

  pub fn deps_with_ids(&self) -> impl Iterator<Item = (DepId, &Dep)> {
    self.deps.iter().enumerate().map(|(i, dep)| (DepId(i), dep))
  }

  pub fn update_dep(&mut self, dep_id: DepId, new_version_req: VersionReq) {
    self
      .pending_changes
      .push(Change::Update(dep_id, new_version_req));
  }

  pub fn commit_changes(&mut self) -> Result<(), AnyError> {
    let changes = std::mem::take(&mut self.pending_changes);
    let mut config_updaters = HashMap::new();
    for change in changes {
      match change {
        Change::Update(dep_id, version_req) => {
          // TODO: move most of this to ConfigUpdater
          let dep = &mut self.deps[dep_id.0];
          dep.req.version_req = version_req.clone();
          let DepLocation {
            ende_json,
            key_path,
            ..
          } = &dep.location;
          let updater =
            get_or_create_updater(&mut config_updaters, &dep.location)?;

          let Some(property) = updater.get_property_for_mutation(key_path)
          else {
            log::warn!(
              "failed to find property at path {key_path:?} for file {}",
              ende_json.specifier
            );
            continue;
          };
          let Some(string_value) = cst_string_literal(&property) else {
            continue;
          };
          let mut req_reference =
            JsrPackageReqReference::from_str(&string_value)
              .unwrap()
              .into_inner();
          req_reference.req.version_req = version_req;
          let mut new_value = format!("jsr:{}", req_reference);
          if string_value.ends_with('/') && !new_value.ends_with('/') {
            // the display impl for PackageReqReference maps `/` to the root
            // subpath, but for the import map the trailing `/` is significant
            new_value.push('/');
          }
          if string_value.trim_start_matches("jsr:").starts_with('/') {
            // this is gross
            new_value = new_value.replace(':', ":/");
          }
          property
            .set_value(jsonc_parser::cst::CstInputValue::String(new_value));
        }
      }
    }

    for (_, updater) in config_updaters {
      updater.commit()?;
    }

    Ok(())
  }

  /// Removes the lockfile specifier entries for the given dependencies and
  /// writes the lockfile to disk. The next install will re-resolve the
  /// missing entries to the latest version that matches each dependency's
  /// existing version requirement, bumping the lockfile within ranges
  /// without modifying `ende.json`.
  pub fn invalidate_lockfile_for_update(
    &self,
    dep_ids: impl IntoIterator<Item = DepId>,
  ) -> Result<(), AnyError> {
    let Some(lockfile_lock) = &self.lockfile else {
      return Ok(());
    };
    {
      let mut lockfile = lockfile_lock.lock();
      for dep_id in dep_ids {
        let dep = &self.deps[dep_id.0];
        let req = JsrDepPackageReq::jsr(dep.req.clone());
        if lockfile.content.packages.specifiers.remove(&req).is_some() {
          lockfile.has_content_changed = true;
        }
      }
    }
    lockfile_lock.write_if_changed()?;
    Ok(())
  }
}

fn get_or_create_updater<'a>(
  config_updaters: &'a mut HashMap<std::path::PathBuf, ConfigUpdater>,
  location: &DepLocation,
) -> Result<&'a mut ConfigUpdater, AnyError> {
  match config_updaters.entry(location.file_path().into_owned()) {
    std::collections::hash_map::Entry::Occupied(occupied_entry) => {
      Ok(occupied_entry.into_mut())
    }
    std::collections::hash_map::Entry::Vacant(vacant_entry) => {
      let updater = ConfigUpdater::new(location.file_path().into_owned())?;
      Ok(vacant_entry.insert(updater))
    }
  }
}

fn cst_string_literal(
  property: &jsonc_parser::cst::CstObjectProp,
) -> Option<String> {
  // TODO(nathanwhit): ensure this unwrap is safe
  let value = property.value().unwrap();
  let Some(string) = value.as_string_lit() else {
    log::warn!("malformed entry");
    return None;
  };
  let Ok(string_value) = string.decoded_value() else {
    log::warn!("malformed string: {string:?}");
    return None;
  };
  Some(string_value)
}
