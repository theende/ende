// Copyright 2018-2026 the Ende authors. MIT license.

//! The sandboxes of `isolate` rules: each runs the modules its rule matches
//! in a worker of its own, with the rule's grant checked together with the
//! app's live permissions (see `PermissionsContainer::chained_to`).
//!
//! - An import from outside a sandbox into a module it holds is served a
//!   generated proxy module (`isolate:` specifiers, see [`proxy_specifier`]):
//!   one async function per exported function and a snapshot per constant,
//!   each a call into the sandbox's worker (`runtime/js/11_workers.js`).
//! - The worker starts from [`HARNESS`], which loads entries when the app
//!   asks and serves its calls. Its loader holds only the sandbox's own
//!   static graph (`CliModuleLoader`), so anything else is checked against
//!   the sandbox's permissions.
//! - Its permissions come from ende.json alone, through
//!   [`CliSandboxWorkerProvider`], never from JS.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use deno_error::JsErrorBox;
use deno_graph::ModuleGraph;
use ende_config::ende_json::IsolateConfig;
use ende_core::ModuleSpecifier;
use ende_core::parking_lot::Mutex;
use ende_core::serde_json;
use ende_core::url::Url;
use ende_runtime::ende_permissions::Permissions;
use ende_runtime::ende_permissions::PermissionsContainer;
use ende_runtime::ende_permissions::PermissionsOptions;
use ende_runtime::ende_permissions::RuntimePermissionDescriptorParser;
use ende_runtime::ops::worker_host::SandboxWorkerProvider;
use ende_runtime::ops::worker_host::SandboxWorkerSpec;

use super::Classifier;
use super::SandboxId;
use super::exports::Requested;
use super::exports::module_exports;
use super::exports::requested_exports;
use super::manifests;
use super::paths_of;
use super::unmatched_rule_warning;
use crate::args::CliLockfile;
use crate::args::extension_grant_permissions_options;
use crate::graph_container::MainModuleGraphContainer;
use crate::graph_container::ModuleGraphContainer;
use crate::sys::CliSys;

type RequestedByImporter = Option<Arc<HashMap<String, Requested>>>;

/// The scheme of the proxy modules the app imports sandboxed modules
/// through.
pub const ISOLATE_SCHEME: &str = "isolate";

/// The `isolate` rules of an app and what they've started.
pub struct SandboxRegistry {
  config: IsolateConfig,
  classifier: Classifier,
  sys: CliSys,
  desc_parser: Arc<RuntimePermissionDescriptorParser<CliSys>>,
  /// The app's own permissions as configured, to warn about grants wider
  /// than them.
  app_options: PermissionsOptions,
  /// What no sandbox may write: the app's config, import map, lockfile,
  /// vendor folder and module cache.
  protected_paths: Vec<PathBuf>,
  main_graph: Arc<MainModuleGraphContainer>,
  /// The paths of `file:` URLs, as written and with symlinks resolved.
  paths: Mutex<HashMap<Url, Vec<PathBuf>>>,
  /// What each importer asks each sandboxed module for (see
  /// [`Self::requested_by`]), with a hash of the code it was read from;
  /// `None` when it doesn't parse.
  requested: Mutex<HashMap<ModuleSpecifier, (u64, RequestedByImporter)>>,
  /// The rules whose too-wide grant was warned about.
  warned_grants: Mutex<HashSet<SandboxId>>,
  warned_unmatched: Mutex<bool>,
  /// Pins each rule's grant and its entries' manifests once approved.
  lockfile: Option<Arc<CliLockfile>>,
  /// `--no-lock` was given on the command line: sandboxes run unpinned,
  /// with a warning. Without it, a missing lockfile (`"lock": false` in the
  /// config) is an error.
  no_lock_flag: bool,
  /// The folder of the root ende.json, which the lockfile names local
  /// entries relative to.
  root_dir: Url,
  /// The sandboxes the app imported, with what they run with.
  prepared: Mutex<HashMap<SandboxId, Arc<PreparedSandbox>>>,
  /// The rules whose "Recorded" notice this run printed: one per rule.
  noticed: Mutex<HashSet<SandboxId>>,
  /// The folder commands run from, for the `ende install --entrypoint`
  /// command a block message prints.
  cwd: PathBuf,
}

/// A sandbox whose entries' manifests were read and whose pin was checked.
struct PreparedSandbox {
  /// Its grant ∩ its entries' manifests (the app's permissions apply on
  /// top, through the chain).
  options: PermissionsOptions,
  /// Its grant alone, and the grant ∩ each manifest by entry, to say which
  /// manifests withhold what a denial asks for. Empty without manifests.
  narrowing: Option<(PermissionsOptions, Vec<(String, PermissionsOptions)>)>,
  /// The entries counted, by lockfile key, with the `permissions` of the
  /// manifest each declares.
  entries: Mutex<BTreeMap<String, Option<serde_json::Value>>>,
  /// The modules those entries name. The app's loader may reach one by
  /// another specifier (a jsr import by its registry URL), which is the
  /// same entry.
  modules: Mutex<HashSet<ModuleSpecifier>>,
}

impl SandboxRegistry {
  #[allow(clippy::too_many_arguments, reason = "set up once, by the factory")]
  pub fn new(
    config: IsolateConfig,
    jsr_url: &Url,
    desc_parser: Arc<RuntimePermissionDescriptorParser<CliSys>>,
    app_options: PermissionsOptions,
    protected_paths: Vec<PathBuf>,
    main_graph: Arc<MainModuleGraphContainer>,
    lockfile: Option<Arc<CliLockfile>>,
    no_lock_flag: bool,
    root_dir: Url,
    cwd: PathBuf,
  ) -> Self {
    let sys = CliSys::default();
    Self {
      classifier: Classifier::new(&config, jsr_url, &sys),
      config,
      sys,
      desc_parser,
      app_options,
      protected_paths,
      main_graph,
      paths: Default::default(),
      requested: Default::default(),
      warned_grants: Default::default(),
      warned_unmatched: Default::default(),
      lockfile,
      no_lock_flag,
      root_dir,
      prepared: Default::default(),
      noticed: Default::default(),
      cwd,
    }
  }

  /// The sandbox the module `specifier` resolves to in `graph` belongs to.
  pub fn sandbox_of(
    &self,
    graph: &ModuleGraph,
    specifier: &ModuleSpecifier,
  ) -> Option<SandboxId> {
    self
      .classifier
      .sandbox_of(graph, specifier, &|url| self.cached_paths(url))
  }

  /// Whether `specifier` is one of sandbox `id`'s own modules. For a
  /// folder rule that is decided by where the file really is, with symlinks
  /// resolved, never by the path it is reached through: a link in the
  /// folder doesn't make what it points at the sandbox's. Other rules
  /// classify as the app does.
  pub fn is_own(
    &self,
    id: SandboxId,
    graph: &ModuleGraph,
    specifier: &ModuleSpecifier,
  ) -> bool {
    if !self.classifier.is_folder_rule(id) {
      return self.sandbox_of(graph, specifier) == Some(id);
    }
    let target = graph.resolve(specifier);
    if target.scheme() != "file" {
      return false;
    }
    let (Some(folder), Ok(path)) = (
      self.classifier.canonical_folder(id, &self.sys),
      deno_path_util::url_to_file_path(target),
    ) else {
      return false;
    };
    real_path(&path).is_some_and(|real| real.starts_with(&folder))
  }

  /// `paths_of(url)`, cached for the life of the registry.
  fn cached_paths(&self, url: &Url) -> Vec<PathBuf> {
    if let Some(paths) = self.paths.lock().get(url) {
      return paths.clone();
    }
    let paths = paths_of(url, &self.sys);
    self.paths.lock().insert(url.clone(), paths.clone());
    paths
  }

  /// The matcher of a rule, as written in ende.json.
  pub fn matcher(&self, id: SandboxId) -> &str {
    self
      .config
      .rules
      .get_index(id.0)
      .map(|(matcher, _)| matcher.as_str())
      .unwrap_or_default()
  }

  pub fn id_of(&self, matcher: &str) -> Result<SandboxId, JsErrorBox> {
    self
      .config
      .rules
      .get_index_of(matcher)
      .map(SandboxId)
      .ok_or_else(|| {
        JsErrorBox::type_error(format!(
          "ende.json has no isolate rule \"{matcher}\""
        ))
      })
  }

  /// The main module of the worker of sandbox `id`: [`HARNESS`] as a
  /// `data:` URL.
  pub fn harness_url(&self, id: SandboxId) -> ModuleSpecifier {
    let code = HARNESS.replace("__ENDE_RULE__", &js_string(self.matcher(id)));
    let url = format!(
      "data:application/javascript;base64,{}",
      base64::engine::general_purpose::STANDARD.encode(code)
    );
    ModuleSpecifier::parse(&url).unwrap()
  }

  /// Warns once for each rule that matches nothing in `graph`, the app's
  /// static graph.
  pub fn warn_unmatched(&self, graph: &ModuleGraph) {
    let mut warned = self.warned_unmatched.lock();
    if *warned {
      return;
    }
    *warned = true;
    let classification =
      super::classify(graph, &self.config, &self.classifier.jsr_url, &self.sys);
    for id in classification.unmatched {
      log::warn!(
        "{} {}",
        crate::colors::yellow("Warning"),
        unmatched_rule_warning(self.matcher(id))
      );
    }
  }

  /// What the import of `raw` in `referrer` asks for, read from `code`, the
  /// code that runs (parsed once per version of it). When that code can't
  /// be read, every export that can cross.
  pub fn requested_by(
    &self,
    referrer: &ModuleSpecifier,
    raw: &str,
    code: Option<String>,
  ) -> Requested {
    let requested = code.and_then(|code| {
      let hash = {
        use std::hash::Hash;
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        code.hash(&mut hasher);
        hasher.finish()
      };
      let cached = self.requested.lock().get(referrer).cloned();
      match cached {
        Some((cached_hash, requested)) if cached_hash == hash => requested,
        _ => {
          let requested = requested_exports(referrer, &code).map(Arc::new);
          self
            .requested
            .lock()
            .insert(referrer.clone(), (hash, requested.clone()));
          requested
        }
      }
    });
    match requested.as_ref().and_then(|requested| requested.get(raw)) {
      Some(requested) => requested.clone(),
      None => Requested {
        names: Default::default(),
        all: true,
      },
    }
  }

  /// The code of the proxy module for `entry`, a module of a sandbox, asked
  /// for `requested`. Importing an export that can't cross, as the source
  /// shows, is an error here; one whose kind shows only when the sandbox
  /// runs is an error when the proxy runs.
  pub fn proxy_module(
    &self,
    graph: &ModuleGraph,
    entry: &ModuleSpecifier,
    requested: &Requested,
  ) -> Result<String, JsErrorBox> {
    let target = graph.resolve(entry).clone();
    let Some(id) = self.sandbox_of(graph, &target) else {
      return Err(JsErrorBox::type_error(format!(
        "\"{target}\" isn't in a sandbox: no isolate rule in ende.json matches it"
      )));
    };
    let rule = self.matcher(id);
    match graph.get(&target) {
      Some(deno_graph::Module::Js(_)) => {}
      Some(_) => {
        return Err(JsErrorBox::type_error(format!(
          "\"{target}\" is in the sandbox of isolate[\"{rule}\"], and only its JavaScript and TypeScript modules can be imported from outside it"
        )));
      }
      None => {
        return Err(JsErrorBox::type_error(format!(
          "\"{target}\" (in the sandbox of isolate[\"{rule}\"]) wasn't loaded"
        )));
      }
    }
    let exports = module_exports(graph, &target);
    let mut named = Vec::new();
    for name in &requested.names {
      match exports.names.get(name) {
        Some(kind) => {
          if let Some(what) = unsupported_export(name, kind) {
            return Err(cannot_import(rule, &target, name, what));
          }
          named.push(name.clone());
        }
        // V8 reports the missing export at the import.
        None if exports.complete => {}
        // An `export *` the source doesn't show: the sandbox decides.
        None => named.push(name.clone()),
      }
    }
    let implicit = if requested.all {
      exports
        .names
        .iter()
        .filter(|(name, kind)| {
          unsupported_export(name, kind).is_none()
            && !requested.names.contains(*name)
        })
        .map(|(name, _)| name.clone())
        .collect()
    } else {
      Vec::new()
    };
    self.check_entry(id, entry, &target)?;
    Ok(proxy_module_code(rule, &target, &named, &implicit))
  }

  /// Prepares sandbox `id` when the app first imports it: reads the
  /// manifests of its entries in the app's graph, works out what it runs
  /// with and checks its pin. An entry imported after that (a computed
  /// `import()`) is checked on its own: it is recorded like the others, and
  /// a manifest of its own that the running sandbox doesn't already apply
  /// is an error, since the sandbox's permissions are set when it starts.
  fn check_entry(
    &self,
    id: SandboxId,
    entry: &ModuleSpecifier,
    target: &ModuleSpecifier,
  ) -> Result<(), JsErrorBox> {
    let prepared = self.prepare(id, Some((entry, target)))?;
    let key = self.entry_key(entry, target);
    if prepared.modules.lock().contains(target)
      || prepared.entries.lock().contains_key(&key)
    {
      return Ok(());
    }
    let rule = self.matcher(id);
    let graph = self.main_graph.graph();
    let manifest = manifests::read_manifest(rule, &graph, target)?;
    if let Some(json) = manifest.as_ref().map(|m| &m.permissions_json)
      && !prepared
        .entries
        .lock()
        .values()
        .any(|counted| counted.as_ref() == Some(json))
    {
      return Err(JsErrorBox::type_error(format!(
        "\"{target}\" declares a manifest of its own, but it was first imported after the sandbox of isolate[\"{rule}\"] started, which runs with the manifests of the entries the app's module graph held then. Import it from a module the app imports statically (a dynamic import the graph reaches only through a computed import, or with --unstable-lazy-dynamic-imports, comes too late), so its manifest counts from the start"
      )));
    }
    let json = manifest.as_ref().map(|m| m.permissions_json.clone());
    let mut manifests = prepared
      .entries
      .lock()
      .iter()
      .map(|(key, manifest)| {
        (
          key.clone(),
          manifest.clone().unwrap_or(serde_json::Value::Null),
        )
      })
      .collect::<BTreeMap<_, _>>();
    manifests
      .insert(key.clone(), json.clone().unwrap_or(serde_json::Value::Null));
    self.check_rule_pin(
      id,
      &prepared.options,
      manifests,
      std::slice::from_ref(target),
    )?;
    prepared.entries.lock().insert(key, json);
    prepared.modules.lock().insert(target.clone());
    Ok(())
  }

  /// The key the lockfile names entry `entry` (naming `target`) by.
  fn entry_key(
    &self,
    entry: &ModuleSpecifier,
    target: &ModuleSpecifier,
  ) -> String {
    manifests::entry_key(
      entry,
      target,
      &self.root_dir,
      &self.classifier.jsr_url,
    )
  }

  /// The key of rule `id`'s pin (see [`super::rule_key`]).
  pub fn rule_key(&self, id: SandboxId) -> String {
    let (matcher, rule) = self.config.rules.get_index(id.0).unwrap();
    super::rule_key(matcher, &rule.matcher, &self.root_dir)
  }

  /// What a first pin of rule `id` yields to: each other rule that `graph`
  /// puts a module rule `id` covers under, with its grant. Only what the
  /// graph shows: an earlier rule that covers none of those modules yet (a
  /// symlinked folder pointing elsewhere, a URL no redirect reaches) takes
  /// them later only with approval.
  fn first_yields(
    &self,
    id: SandboxId,
    graph: &ModuleGraph,
  ) -> Vec<ende_lockfile::IsolateYield> {
    let key = self.rule_key(id);
    let (written, rule) = self.config.rules.get_index(id.0).unwrap();
    let alone = IsolateConfig {
      rules: [(written.clone(), rule.clone())].into_iter().collect(),
    };
    let covered =
      super::classify(graph, &alone, &self.classifier.jsr_url, &self.sys);
    let mut yields: Vec<ende_lockfile::IsolateYield> = Vec::new();
    for module in covered.sandboxes.keys() {
      let Some(other) = self.sandbox_of(graph, module) else {
        continue;
      };
      let other_key = self.rule_key(other);
      if other_key != key
        && !yields.iter().any(|yielded| yielded.rule == other_key)
      {
        yields.push(ende_lockfile::IsolateYield {
          rule: other_key,
          grant: self.config.rules[other.0].permissions_json.clone(),
        });
      }
    }
    yields
  }

  /// How `root`, an entry point, is shown in a command to type.
  fn shown_root(&self, root: &ModuleSpecifier) -> String {
    super::shown_module(&self.cwd, root)
  }

  /// `ende install --entrypoint` with what approves this run of sandbox
  /// `id`: the run's entry points that reach the sandbox statically (or are
  /// its entries), or else `modules`, the entries the run imports (a
  /// sandbox reached only through a computed import, or from `ende eval`).
  fn entrypoint_command(
    &self,
    id: SandboxId,
    modules: &[ModuleSpecifier],
  ) -> String {
    let graph = self.main_graph.graph();
    let mut roots = graph
      .roots
      .iter()
      .filter(|root| matches!(root.scheme(), "file" | "http" | "https" | "jsr"))
      .filter(|root| !root.path().contains("$ende$"))
      .filter(|root| {
        self.sandbox_of(&graph, root) == Some(id) || {
          let segment = graph.segment(std::slice::from_ref(*root));
          !manifests::sandbox_entries(&segment, id, &|specifier| {
            self.sandbox_of(&segment, specifier)
          })
          .is_empty()
        }
      })
      .map(|root| self.shown_root(root))
      .collect::<Vec<_>>();
    if roots.is_empty() {
      roots = modules
        .iter()
        .map(|module| self.shown_root(module))
        .collect();
    }
    if roots.is_empty() {
      "ende install --entrypoint <the module you run>".to_string()
    } else {
      format!("ende install --entrypoint {}", roots.join(" "))
    }
  }

  /// Checks rule `id`'s pin for this run, which gets `options` and
  /// imports entries with `manifests` (`modules`: the modules), recording
  /// what needs no approval. (That no module another pin covers moved here
  /// is `super::PinChecks`'s job, done when the graph loads.)
  fn check_rule_pin(
    &self,
    id: SandboxId,
    options: &PermissionsOptions,
    manifests: BTreeMap<String, serde_json::Value>,
    modules: &[ModuleSpecifier],
  ) -> Result<(), JsErrorBox> {
    let (rule, config) = self.config.rules.get_index(id.0).unwrap();
    let key = self.rule_key(id);
    let grant = extension_grant_permissions_options(&config.permissions)
      .map_err(|err| JsErrorBox::type_error(err.to_string()))?;
    let approved = self
      .lockfile
      .as_ref()
      .and_then(|lockfile| lockfile.lock().isolate_rule(&key).cloned());
    let notice = !self.noticed.lock().contains(&id);
    let mut pin = manifests::decide_run(
      manifests::RunInputs {
        key,
        matcher: rule,
        grant_json: &config.permissions_json,
        grant: &grant,
        options,
        manifests: &manifests,
        yields_to: match approved {
          Some(_) => Vec::new(),
          None => self.first_yields(id, &self.main_graph.graph()),
        },
        root_dir: &self.root_dir,
        entrypoint_command: String::new(),
        notice,
      },
      approved.as_ref(),
      self.desc_parser.as_ref(),
    )?;
    if !pin.needs_approval.is_empty() {
      pin.approve_command = self.entrypoint_command(id, modules);
    }
    let noticed = crate::pins::Pinned::notice(&pin);
    crate::pins::check_pin(&pin, self.lockfile.as_deref(), self.no_lock_flag)?;
    if noticed {
      self.noticed.lock().insert(id);
    }
    Ok(())
  }

  /// Sandbox `id` as the app's graph has it now, with `first` (an entry
  /// and its module) if that is the import that prepares it (see
  /// [`Self::check_entry`]).
  fn prepare(
    &self,
    id: SandboxId,
    first: Option<(&ModuleSpecifier, &ModuleSpecifier)>,
  ) -> Result<Arc<PreparedSandbox>, JsErrorBox> {
    if let Some(prepared) = self.prepared.lock().get(&id) {
      return Ok(prepared.clone());
    }
    let (rule, config) = self.config.rules.get_index(id.0).unwrap();
    let graph = self.main_graph.graph();
    let mut entries = manifests::sandbox_entries(&graph, id, &|specifier| {
      self.sandbox_of(&graph, specifier)
    });
    if let Some((entry, target)) = first
      && !entries.values().any(|counted| counted == target)
    {
      entries.insert(entry.clone(), target.clone());
    }
    // One entry per module: a jsr import and its registry URL are the same.
    let mut modules = HashSet::new();
    entries.retain(|_, target| modules.insert(target.clone()));
    let sandboxed = entries.clone();
    let mut read: BTreeMap<String, Option<crate::extensions::PackageManifest>> =
      BTreeMap::new();
    for (entry, target) in &entries {
      let manifest = manifests::read_manifest(rule, &graph, target)?;
      read.insert(self.entry_key(entry, target), manifest);
    }
    let grant = extension_grant_permissions_options(&config.permissions)
      .map_err(|err| JsErrorBox::type_error(err.to_string()))?;
    let mut per_entry = Vec::new();
    for (key, manifest) in &read {
      if let Some(manifest) = manifest {
        let (narrowed, _) = manifests::effective_options(
          rule,
          &grant,
          std::iter::once((key.as_str(), manifest)),
          self.desc_parser.as_ref(),
        )?;
        per_entry.push((key.clone(), narrowed));
      }
    }
    let narrowing = (!per_entry.is_empty()).then(|| {
      let mut grant = grant.clone();
      grant.prompt = false;
      (grant, per_entry)
    });
    let (options, warnings) = manifests::effective_options(
      rule,
      &grant,
      read
        .iter()
        .filter_map(|(key, manifest)| Some((key.as_str(), manifest.as_ref()?))),
      self.desc_parser.as_ref(),
    )?;
    let entries = read
      .iter()
      .map(|(key, manifest)| {
        (
          key.clone(),
          manifest
            .as_ref()
            .map(|manifest| manifest.permissions_json.clone()),
        )
      })
      .collect::<BTreeMap<_, _>>();
    let targets = sandboxed.values().cloned().collect::<Vec<_>>();
    self.check_rule_pin(
      id,
      &options,
      entries
        .iter()
        .map(|(key, manifest)| {
          (
            key.clone(),
            manifest.clone().unwrap_or(serde_json::Value::Null),
          )
        })
        .collect(),
      &targets,
    )?;
    let mut map = self.prepared.lock();
    if let Some(existing) = map.get(&id) {
      return Ok(existing.clone());
    }
    let prepared = Arc::new(PreparedSandbox {
      options,
      narrowing,
      entries: Mutex::new(entries),
      modules: Mutex::new(modules),
    });
    map.insert(id, prepared.clone());
    drop(map);
    for warning in warnings {
      log::warn!("{} {}", crate::colors::yellow("Warning"), warning);
    }
    Ok(prepared)
  }

  /// `container`, sandbox `id`'s, with what explains a denial its
  /// manifests cause (see `ManifestNarrowing`).
  fn with_manifest_narrowing(
    &self,
    id: SandboxId,
    container: PermissionsContainer,
  ) -> Result<PermissionsContainer, JsErrorBox> {
    let prepared = self.prepare(id, None)?;
    let Some((grant, per_entry)) = &prepared.narrowing else {
      return Ok(container);
    };
    let build = |options: &PermissionsOptions| {
      Permissions::from_options(self.desc_parser.as_ref(), options)
        .map(|permissions| {
          PermissionsContainer::new(self.desc_parser.clone(), permissions)
            .with_import_symlinks_resolved()
        })
        .map_err(|err| JsErrorBox::type_error(err.to_string()))
    };
    let mut manifests = Vec::with_capacity(per_entry.len());
    for (entry, options) in per_entry {
      manifests.push((entry.clone(), build(options)?));
    }
    Ok(container.with_manifest_narrowing(
      ende_runtime::ende_permissions::ManifestNarrowing::new(
        build(grant)?,
        manifests,
      ),
    ))
  }

  /// `ende install`: drops the pins of rules no longer declared, records
  /// grant changes, with `--entrypoint` approves the moves of the entry
  /// points' modules from one pin to another rule (see `super::PinChecks`), and with `--entrypoint` (`app`: the
  /// entry points' graph and the entry points) adds each entry point's run
  /// to what's approved. It asks before recording what needs approval; `yes`
  /// approves without asking, and without a terminal changes need `yes`.
  /// It never removes an approved run, and reads no entry again, so nothing
  /// it can't load gets it stuck or resets anything.
  pub fn approve_changes(
    &self,
    app: Option<(&ModuleGraph, &[ModuleSpecifier])>,
    yes: bool,
  ) -> Result<(), ende_core::error::AnyError> {
    let Some(lockfile) = &self.lockfile else {
      return Ok(());
    };
    let keys = (0..self.config.rules.len())
      .map(|index| self.rule_key(SandboxId(index)))
      .collect::<Vec<_>>();
    let pinned = lockfile.lock().content.isolate.clone();
    // The entry points' modules that another pin covers, by the rule they
    // now fall under, with that pin. (One that falls under no rule is the
    // removal of its rule, below.)
    let mut moves: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    if let Some((graph, _)) = app
      && let Some(checks) = super::PinChecks::new(
        &self.config,
        &pinned,
        &self.root_dir,
        &self.classifier.jsr_url,
        &self.cwd,
      )
    {
      for found in checks.moves(graph) {
        if let (Some(module), Some(now)) = (&found.module, found.now) {
          let list = moves.entry(now).or_default();
          let item = (found.pin, checks.shown(module));
          if !list.contains(&item) {
            list.push(item);
          }
        }
      }
    }
    let mut reached = HashSet::new();
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for (index, (matcher, config)) in self.config.rules.iter().enumerate() {
      let id = SandboxId(index);
      let key = self.rule_key(id);
      if !seen.insert(key.clone()) {
        // another spelling of a rule declared earlier: one pin
        continue;
      }
      let approved = pinned.get(&key);
      let moves = moves.remove(&key).unwrap_or_default();
      let grant = extension_grant_permissions_options(&config.permissions)?;
      let mut runs = Vec::new();
      if let Some((graph, roots)) = app {
        for root in roots {
          let segment = graph.segment(std::slice::from_ref(root));
          let mut modules = HashSet::new();
          let mut read = BTreeMap::new();
          for (entry, target) in
            manifests::sandbox_entries(&segment, id, &|specifier| {
              self.sandbox_of(&segment, specifier)
            })
          {
            if !modules.insert(target.clone()) {
              continue;
            }
            let manifest =
              manifests::read_manifest(matcher, &segment, &target)?;
            read.insert(self.entry_key(&entry, &target), manifest);
          }
          // An entry point that is itself an entry: a run that imports
          // just it, as a computed import does.
          if read.is_empty()
            && self.sandbox_of(&segment, root) == Some(id)
            && matches!(segment.get(root), Some(deno_graph::Module::Js(_)))
          {
            let target = segment.resolve(root).clone();
            let manifest =
              manifests::read_manifest(matcher, &segment, &target)?;
            read.insert(self.entry_key(root, &target), manifest);
          }
          if read.is_empty() {
            continue;
          }
          reached.insert(root.clone());
          let (options, _) = manifests::effective_options(
            matcher,
            &grant,
            read.iter().filter_map(|(key, manifest)| {
              Some((key.as_str(), manifest.as_ref()?))
            }),
            self.desc_parser.as_ref(),
          )?;
          let manifests = read
            .into_iter()
            .map(|(key, manifest)| {
              (
                key,
                manifest
                  .map(|manifest| manifest.permissions_json)
                  .unwrap_or(serde_json::Value::Null),
              )
            })
            .collect();
          runs.push((self.shown_root(root), options, manifests));
        }
      }
      if let Some(pin) = manifests::decide_install(
        manifests::InstallInputs {
          key,
          matcher,
          grant_json: &config.permissions_json,
          grant: &grant,
          moves,
          yields_to: match (approved, app) {
            (None, Some((graph, _))) => self.first_yields(id, graph),
            _ => Vec::new(),
          },
          runs,
          root_dir: &self.root_dir,
        },
        approved,
        self.desc_parser.as_ref(),
      )? {
        items.push(pin);
      }
    }
    // A rule ende.json no longer declares: dropping its pin would run what
    // it sandboxed in the app (or under another rule), so that needs
    // approval too; a run that imports a module it covers is refused until
    // then (see `super::PinChecks`).
    for (key, pin) in &pinned {
      if !keys.contains(key) {
        items.push(manifests::RulePin::removal(key, pin));
      }
    }
    if let Some((_, roots)) = app {
      for root in roots.iter().filter(|root| !reached.contains(*root)) {
        log::warn!(
          "{} {} imports no sandboxed module, so `ende install --entrypoint` approves nothing for it.",
          crate::colors::yellow("Warning"),
          self.shown_root(root)
        );
      }
    }
    crate::pins::approve_changes(items, lockfile, yes)
  }

  /// The error for an import, from inside sandbox `outer`, of a module in
  /// another sandbox.
  pub fn nested_error(
    &self,
    outer: SandboxId,
    inner: SandboxId,
    specifier: &ModuleSpecifier,
    referrer: &str,
  ) -> JsErrorBox {
    JsErrorBox::type_error(format!(
      "\"{specifier}\" is in the sandbox of isolate[\"{}\"], but it is imported from inside the sandbox of isolate[\"{}\"] (by {referrer}). Sandboxes can't be nested yet: change the rules so that one sandbox holds both",
      self.matcher(inner),
      self.matcher(outer),
    ))
  }

  /// The permissions of sandbox `id`'s worker: its grant ∩ its entries'
  /// manifests, never prompting,
  /// with every path no sandbox may write denied: the protected paths, the
  /// app's local modules outside this sandbox (including other sandboxes'),
  /// and the folders of the other folder rules.
  fn worker_options(
    &self,
    id: SandboxId,
  ) -> Result<PermissionsOptions, JsErrorBox> {
    let mut options = self.prepare(id, None)?.options.clone();
    options.prompt = false;

    let mut denied = self.protected_paths.clone();
    denied.extend(self.classifier.other_folders(id));
    let graph = self.main_graph.graph();
    for module in graph.modules() {
      let specifier = module.specifier();
      if specifier.scheme() == "file"
        && self.sandbox_of(&graph, specifier) != Some(id)
      {
        denied.extend(paths_of(specifier, &self.sys));
      }
    }
    if !denied.is_empty() {
      let deny_write = options.deny_write.get_or_insert_with(Vec::new);
      for path in denied {
        let path = path.to_string_lossy().into_owned();
        if !deny_write.contains(&path) {
          deny_write.push(path);
        }
      }
    }

    Ok(options)
  }

  /// Warns, once per rule, when sandbox `id`'s grant asks for permissions
  /// the app itself doesn't have.
  fn warn_wider_grant(&self, id: SandboxId) -> Result<(), JsErrorBox> {
    let (matcher, rule) = self.config.rules.get_index(id.0).unwrap();
    if self.warned_grants.lock().insert(id) {
      let grant = extension_grant_permissions_options(&rule.permissions)
        .map_err(|err| JsErrorBox::type_error(err.to_string()))?;
      let (_, ungranted) = self
        .app_options
        .intersect_requested(&grant, self.desc_parser.as_ref())
        .map_err(|err| JsErrorBox::type_error(err.to_string()))?;
      if !ungranted.is_empty() {
        let mut warning = format!(
          "isolate[\"{matcher}\"] grants permissions the app itself doesn't have. A sandbox never gets more than the app, so using them will be denied:"
        );
        for permission in &ungranted {
          match &permission.entry {
            Some(entry) => {
              warning.push_str(&format!("\n  {} \"{entry}\"", permission.name))
            }
            None => warning.push_str(&format!("\n  {}", permission.name)),
          }
        }
        log::warn!("{} {}", crate::colors::yellow("Warning"), warning);
      }
    }
    Ok(())
  }

  /// What a worker of sandbox `id` loads the sandbox's own local modules
  /// with when the app's graph doesn't hold them (a worker the sandbox
  /// starts, or a module it imports by a computed path): its permissions,
  /// plus reading its own folders, still chained to the app's. `None` for a
  /// rule that isn't a folder: its own modules then need its own grant.
  pub fn own_code_permissions(
    &self,
    id: SandboxId,
    sandbox: &PermissionsContainer,
  ) -> Result<Option<PermissionsContainer>, JsErrorBox> {
    let Some(folder) = self.classifier.canonical_folder(id, &self.sys) else {
      return Ok(None);
    };
    let folders = [folder];
    let Some(app) = sandbox.app_permissions() else {
      return Ok(None);
    };
    let mut options = self.worker_options(id)?;
    match &mut options.allow_read {
      Some(list) if list.is_empty() => {}
      Some(list) => {
        list.extend(folders.iter().map(|f| f.to_string_lossy().into_owned()))
      }
      None => {
        options.allow_read = Some(
          folders
            .iter()
            .map(|folder| folder.to_string_lossy().into_owned())
            .collect(),
        )
      }
    }
    let permissions =
      Permissions::from_options(self.desc_parser.as_ref(), &options)
        .map_err(|err| JsErrorBox::type_error(err.to_string()))?;
    // Imports are checked by where the file really is, so a link in the
    // folder reads nothing outside it.
    Ok(Some(
      self.with_manifest_narrowing(
        id,
        PermissionsContainer::new(self.desc_parser.clone(), permissions)
          .with_import_symlinks_resolved()
          .chained_to(app),
      )?,
    ))
  }

  /// The folder of each folder rule, with symlinks resolved, by matcher.
  pub fn own_folders(&self) -> Vec<(String, PathBuf)> {
    (0..self.config.rules.len())
      .map(SandboxId)
      .filter_map(|id| {
        let folder = self.classifier.canonical_folder(id, &self.sys)?;
        Some((self.matcher(id).to_string(), folder))
      })
      .collect()
  }

  /// Errors if a rule grants write access to the folder that holds the
  /// app's ende.json (or to everything): the sandbox could then rewrite the
  /// app around the protected paths.
  pub fn check_write_grants(
    &self,
    project_root: &std::path::Path,
    extension_folders: &[(String, PathBuf)],
    task_paths: &[(String, PathBuf)],
  ) -> Result<(), JsErrorBox> {
    let root =
      real_path(project_root).unwrap_or_else(|| project_root.to_path_buf());
    for (matcher, rule) in &self.config.rules {
      let grant = extension_grant_permissions_options(&rule.permissions)
        .map_err(|err| JsErrorBox::type_error(err.to_string()))?;
      let holds_root = match &grant.allow_write {
        None => false,
        Some(list) if list.is_empty() => true,
        Some(list) => list.iter().any(|entry| {
          let entry = std::path::Path::new(entry);
          let entry = real_path(entry).unwrap_or_else(|| entry.to_path_buf());
          root.starts_with(entry)
        }),
      };
      if holds_root {
        return Err(JsErrorBox::type_error(format!(
          "isolate[\"{matcher}\"] grants write access to the folder that holds ende.json, or above it (or to everything). A sandbox could then rewrite the app; grant only the folders it writes, like \"./out\"."
        )));
      }
    }
    // Nor may it overlap, either way, what configures or pins the app, or
    // the folder of another sandbox or of an extension.
    let protected = self
      .protected_paths
      .iter()
      .map(|path| real_path(path).unwrap_or_else(|| path.clone()))
      .collect::<Vec<_>>();
    let own_folders = self.own_folders();
    for (matcher, writable) in self.write_grant_paths() {
      let within = ende_runtime::ende_permissions::path_within;
      for (task, path) in task_paths {
        let path = real_path(path).unwrap_or_else(|| path.clone());
        if within(&path, &writable) || within(&writable, &path) {
          return Err(JsErrorBox::type_error(format!(
            "isolate[\"{matcher}\"] may write {}, which holds {}, which task \"{task}\" runs; a sandbox's write grant can't cover a script a task runs. Move the script out of the granted folder, or narrow the grant",
            writable.display(),
            path.display()
          )));
        }
      }
      if let Some(path) = protected.iter().find(|path| within(path, &writable))
      {
        return Err(JsErrorBox::type_error(format!(
          "isolate[\"{matcher}\"] may write {}, which holds {}, a config file or lockfile Ende uses (or its import map, vendor folder, node_modules or module cache); grant a folder of data instead",
          writable.display(),
          path.display()
        )));
      }
      if let Some(path) = protected.iter().find(|path| within(&writable, path))
      {
        return Err(JsErrorBox::type_error(format!(
          "isolate[\"{matcher}\"] may write {}, which is inside {}, where the app keeps its config, lockfile, import map, vendor folder, node_modules or module cache; grant a folder of data instead",
          writable.display(),
          path.display()
        )));
      }
      let others = own_folders
        .iter()
        .filter(|(other, _)| *other != matcher)
        .map(|(other, folder)| (format!("isolate[\"{other}\"]"), folder))
        .chain(
          extension_folders
            .iter()
            .map(|(name, folder)| (format!("extension \"{name}\""), folder)),
        );
      for (other, folder) in others {
        if within(folder, &writable) || within(&writable, folder) {
          return Err(JsErrorBox::type_error(format!(
            "isolate[\"{matcher}\"] may write {}, which overlaps {}, the folder of {other}; grant a folder of data instead",
            writable.display(),
            folder.display()
          )));
        }
      }
    }
    Ok(())
  }

  /// What each rule's write grant names, as written (absolute).
  pub fn write_grant_entries(&self) -> Vec<(String, PathBuf)> {
    let mut entries = Vec::new();
    for (matcher, rule) in &self.config.rules {
      let Ok(grant) = extension_grant_permissions_options(&rule.permissions)
      else {
        continue;
      };
      for entry in grant.allow_write.iter().flatten() {
        entries.push((matcher.clone(), PathBuf::from(entry)));
      }
    }
    entries
  }

  /// What each rule's write grant covers, with symlinks resolved. Every
  /// grant is scoped (see [`Self::check_write_grants`]).
  pub fn write_grant_paths(&self) -> Vec<(String, PathBuf)> {
    let mut paths = Vec::new();
    for (matcher, rule) in &self.config.rules {
      let Ok(grant) = extension_grant_permissions_options(&rule.permissions)
      else {
        continue;
      };
      for entry in grant.allow_write.iter().flatten() {
        let entry = std::path::Path::new(entry);
        paths.push((
          matcher.clone(),
          real_path(entry).unwrap_or_else(|| entry.to_path_buf()),
        ));
      }
    }
    paths
  }

  /// Errors if a rule may write where a local module of the app's graph
  /// lives (other than its own sandbox's): the module could then be
  /// rewritten. Checked once the app's static graph is in, before it runs.
  pub fn check_write_grants_against_graph(
    &self,
    graph: &ModuleGraph,
  ) -> Result<(), JsErrorBox> {
    let writable = self.write_grant_paths();
    if writable.is_empty() {
      return Ok(());
    }
    for module in graph.modules() {
      let specifier = module.specifier();
      if specifier.scheme() != "file" {
        continue;
      }
      // The paths classification already resolved (as written, then with
      // symlinks resolved when that differs).
      let Some(real) = self.cached_paths(specifier).pop() else {
        continue;
      };
      for (matcher, folder) in &writable {
        let id = self.id_of(matcher)?;
        if real.starts_with(folder) && !self.is_own(id, graph, specifier) {
          return Err(JsErrorBox::type_error(format!(
            "isolate[\"{matcher}\"] may write {}, which holds the app's module {}; grant a folder of data instead",
            folder.display(),
            real.display()
          )));
        }
      }
    }
    Ok(())
  }
}

/// `path` with symlinks resolved in the part of it that exists.
fn real_path(path: &std::path::Path) -> Option<PathBuf> {
  crate::util::fs::canonicalize_path_maybe_not_exists(path).ok()
}

/// Why export `name` of kind `kind` can't cross, if it can't. A `then`
/// export never does: it would make the module's namespace look like a
/// promise to whoever imports it.
pub(super) fn unsupported_export(
  name: &str,
  kind: &super::exports::ExportKind,
) -> Option<&'static str> {
  if name == "then" {
    return Some(THEN_EXPORT);
  }
  kind.unsupported()
}

const THEN_EXPORT: &str =
  "a then export, which would make the module look like a promise";

/// The scheme of the module a sandbox's harness loads an entry through:
/// it re-exports the entry's namespace as `ns`, so a `then` export in the
/// entry can't make the import look like a promise and hang it.
pub const ENTRY_SCHEME: &str = "isolate-entry";

/// The entry an `isolate-entry:` specifier names.
pub fn parse_entry_specifier(
  specifier: &ModuleSpecifier,
) -> Result<ModuleSpecifier, JsErrorBox> {
  percent_encoding::percent_decode_str(specifier.path())
    .decode_utf8()
    .ok()
    .and_then(|entry| ModuleSpecifier::parse(&entry).ok())
    .ok_or_else(|| {
      JsErrorBox::type_error(format!("Invalid module \"{specifier}\""))
    })
}

/// The code of an `isolate-entry:` module for `entry`.
pub fn entry_module_code(entry: &ModuleSpecifier) -> String {
  format!("export * as ns from {};\n", js_string(entry.as_str()))
}

fn cannot_import(
  rule: &str,
  entry: &ModuleSpecifier,
  name: &str,
  what: &str,
) -> JsErrorBox {
  JsErrorBox::type_error(cannot_import_message(
    rule,
    entry.as_str(),
    name,
    what,
  ))
}

/// Why `name` can't be imported from `entry`, a module of the sandbox of
/// `isolate[rule]`: it is `what`. Shared by running, `ende check` and the
/// editor.
pub(super) fn cannot_import_message(
  rule: &str,
  entry: &str,
  name: &str,
  what: &str,
) -> String {
  format!(
    "Cannot import \"{name}\" from \"{entry}\": it is {what}, which can't leave the sandbox of isolate[\"{rule}\"]. A sandboxed package can only export functions (called async, with arguments and results structured-cloned) and structured-clonable constants"
  )
}

/// Starts sandbox workers for the main thread (see
/// [`SandboxWorkerProvider`]).
pub struct CliSandboxWorkerProvider(pub Arc<SandboxRegistry>);

impl SandboxWorkerProvider for CliSandboxWorkerProvider {
  fn worker_spec(&self, rule: &str) -> Result<SandboxWorkerSpec, JsErrorBox> {
    let registry = &self.0;
    let id = registry.id_of(rule)?;
    let options = registry.worker_options(id)?;
    registry.warn_wider_grant(id)?;
    let permissions =
      Permissions::from_options(registry.desc_parser.as_ref(), &options)
        .map_err(|err| JsErrorBox::type_error(err.to_string()))?;
    Ok(SandboxWorkerSpec {
      main_module: registry.harness_url(id),
      // Imports are checked by where the file really is (see
      // `SandboxRegistry::is_own`).
      permissions: registry.with_manifest_narrowing(
        id,
        PermissionsContainer::new(registry.desc_parser.clone(), permissions)
          .with_import_symlinks_resolved(),
      )?,
    })
  }
}

/// The specifier of the proxy module for `entry`, asked for `requested`.
/// Importers that ask for the same exports share one; the functions and
/// constants they get are the same either way (see `runtime/js/11_workers.js`).
pub fn proxy_specifier(
  entry: &ModuleSpecifier,
  requested: &Requested,
) -> ModuleSpecifier {
  let payload = serde_json::json!({
    "entry": entry.as_str(),
    "names": requested.names,
    "all": requested.all,
  });
  let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
    .encode(serde_json::to_vec(&payload).unwrap());
  ModuleSpecifier::parse(&format!("{ISOLATE_SCHEME}:{encoded}")).unwrap()
}

/// The entry and the request a proxy specifier names. The app may write
/// one itself: the entry must then still be a sandboxed module.
pub fn parse_proxy_specifier(
  specifier: &ModuleSpecifier,
) -> Result<(ModuleSpecifier, Requested), JsErrorBox> {
  let invalid =
    || JsErrorBox::type_error(format!("Invalid module \"{specifier}\""));
  let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
    .decode(specifier.path())
    .map_err(|_| invalid())?;
  let payload: serde_json::Value =
    serde_json::from_slice(&bytes).map_err(|_| invalid())?;
  let entry = payload
    .get("entry")
    .and_then(|entry| entry.as_str())
    .and_then(|entry| ModuleSpecifier::parse(entry).ok())
    .ok_or_else(invalid)?;
  let names = payload
    .get("names")
    .and_then(|names| names.as_array())
    .ok_or_else(invalid)?
    .iter()
    .map(|name| name.as_str().map(String::from).ok_or_else(invalid))
    .collect::<Result<_, _>>()?;
  let all = payload
    .get("all")
    .and_then(|all| all.as_bool())
    .ok_or_else(invalid)?;
  Ok((entry, Requested { names, all }))
}

fn js_string(value: &str) -> String {
  serde_json::to_string(value).unwrap()
}

/// The proxy module: starts the sandbox (once), then binds each export.
fn proxy_module_code(
  rule: &str,
  entry: &ModuleSpecifier,
  named: &[String],
  implicit: &[String],
) -> String {
  let list = |names: &[String]| serde_json::to_string(names).unwrap();
  let mut code = format!(
    "const {{ start }} = globalThis.Ende[globalThis.Ende.internal].sandboxes;\nconst bindings = await start({}, {}, {}, {});\n",
    js_string(rule),
    js_string(entry.as_str()),
    list(named),
    list(implicit),
  );
  for (index, name) in named.iter().chain(implicit).enumerate() {
    code.push_str(&format!(
      "const b{index} = bindings[{index}];\nexport {{ b{index} as {} }};\n",
      js_string(name)
    ));
  }
  code
}

/// The main module of a sandbox's worker. It loads an entry when the app
/// first imports it and reports each export as a function, a constant
/// (snapshotted here) or something that can't cross, then serves the app's
/// calls. It runs in the sandbox's own realm, so sandboxed code can tamper
/// with it, but only to its own detriment: the app only ever receives
/// structured-cloned data.
const HARNESS: &str = r#"const rule = __ENDE_RULE__;
const entries = new Map();
const hostObjectBrand = Symbol.for("Deno.core.hostObject");
const functionToString = Function.prototype.toString;
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
function isClass(value) {
  try {
    return /^class\b/.test(functionToString.call(value));
  } catch {
    return false;
  }
}
// Why `value` can't be snapshotted as plain data, if it can't.
function notData(value, seen = new Set()) {
  if (typeof value === "function") return "an object holding a function";
  if (value === null || typeof value !== "object" || seen.has(value)) {
    return undefined;
  }
  seen.add(value);
  if (value[hostObjectBrand] !== undefined) return "a host object";
  const proto = Object.getPrototypeOf(value);
  if (Array.isArray(value) || proto === Object.prototype || proto === null) {
    for (const key of Reflect.ownKeys(value)) {
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (descriptor.get || descriptor.set) {
        return `a value with a getter or setter ("${String(key)}")`;
      }
      const reason = notData(descriptor.value, seen);
      if (reason !== undefined) return reason;
    }
    return undefined;
  }
  if (value instanceof Map) {
    for (const [key, item] of value) {
      const reason = notData(key, seen) ?? notData(item, seen);
      if (reason !== undefined) return reason;
    }
    return undefined;
  }
  if (value instanceof Set) {
    for (const item of value) {
      const reason = notData(item, seen);
      if (reason !== undefined) return reason;
    }
    return undefined;
  }
  if (
    value instanceof Date || value instanceof RegExp ||
    value instanceof ArrayBuffer || ArrayBuffer.isView(value) ||
    value instanceof Error
  ) {
    return undefined;
  }
  const name = proto?.constructor?.name;
  return name ? `an instance of ${name}` : "an object with a prototype of its own";
}
function kindOf(name, value) {
  if (name === "then") {
    return ["unsupported", "a then export, which would make the module look like a promise"];
  }
  if (typeof value === "function") {
    return isClass(value) ? ["unsupported", "a class"] : ["function"];
  }
  const reason = notData(value);
  if (reason !== undefined) return ["unsupported", reason];
  try {
    structuredClone(value);
  } catch {
    return ["unsupported", "a value that can't be structured-cloned"];
  }
  return ["value", value];
}
addEventListener("message", async ({ data }) => {
  const id = data?.id;
  let reply;
  try {
    if (typeof data.load === "string") {
      // Through a module that re-exports the entry's namespace: awaiting
      // the namespace itself would call a `then` it exports.
      const { ns: namespace } = await import(
        "isolate-entry:" + encodeURIComponent(data.load)
      );
      entries.set(data.load, namespace);
      const list = [];
      for (const name of Object.keys(namespace)) {
        list.push([name, ...kindOf(name, namespace[name])]);
      }
      reply = { id, ok: true, value: list };
    } else {
      const { entry, name, args } = data;
      const namespace = entries.get(entry);
      if (namespace === undefined || typeof namespace[name] !== "function") {
        throw new TypeError(`"${name}" is not a function that "${entry}" exports in the sandbox of isolate["${rule}"]`);
      }
      reply = { id, ok: true, value: await namespace[name](...args) };
    }
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
mod test {
  use super::*;

  #[test]
  fn proxy_specifiers_round_trip() {
    let entry =
      ModuleSpecifier::parse("https://jsr.io/@std/csv/1.0.0/mod.ts?x=1#y")
        .unwrap();
    let requested = Requested {
      names: ["default".to_string(), "a-b".to_string()].into(),
      all: true,
    };
    let specifier = proxy_specifier(&entry, &requested);
    assert_eq!(specifier.scheme(), ISOLATE_SCHEME);
    assert_eq!(
      parse_proxy_specifier(&specifier).unwrap(),
      (entry, requested)
    );
    let bad = ModuleSpecifier::parse("isolate:not-json").unwrap();
    assert!(parse_proxy_specifier(&bad).is_err());
  }

  #[test]
  fn proxy_code_binds_named_then_implicit_exports() {
    let entry = ModuleSpecifier::parse("file:///app/vendor/a/mod.ts").unwrap();
    let code = proxy_module_code(
      "./vendor/a/",
      &entry,
      &["default".to_string()],
      &["parse".to_string()],
    );
    assert_eq!(
      code,
      "const { start } = globalThis.Ende[globalThis.Ende.internal].sandboxes;\nconst bindings = await start(\"./vendor/a/\", \"file:///app/vendor/a/mod.ts\", [\"default\"], [\"parse\"]);\nconst b0 = bindings[0];\nexport { b0 as \"default\" };\nconst b1 = bindings[1];\nexport { b1 as \"parse\" };\n"
    );
  }
}
