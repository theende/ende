// Copyright 2018-2026 the Ende authors. MIT license.

//! `ende audit`: which dependencies of a module graph run sandboxed (the
//! `isolate` key of ende.json), what each may do, and whether the lockfile
//! pins their code. It only reports; `ende run` and the other commands that
//! run code enforce the rules (`crate::isolate`).
//!
//! The name is reused: upstream `audit` was npm's vulnerability audit.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::sync::Arc;

use deno_graph::GraphKind;
use deno_graph::JsrLoadError;
use deno_graph::ModuleErrorKind;
use deno_graph::ModuleGraph;
use deno_graph::ModuleLoadError;
use deno_graph::source::recommended_registry_package_url_to_nv;
use deno_semver::package::PackageNv;
use deno_terminal::colors;
use ende_config::ende_json::IsolateConfig;
use ende_config::ende_json::IsolateMatcher;
use ende_core::ModuleSpecifier;
use ende_core::error::AnyError;
use ende_core::serde_json;
use ende_core::serde_json::Value;
use ende_core::serde_json::json;
use ende_core::url::Url;
use ende_runtime::ende_permissions::PermissionDescriptorParser;
use ende_runtime::ende_permissions::PermissionsOptions;

use crate::args::AuditFlags;
use crate::args::Flags;
use crate::args::extension_grant_permissions_options;
use crate::args::jsr_url;
use crate::factory::CliFactory;
use crate::graph_container::CollectSpecifiersOptions;
use crate::isolate;
use crate::isolate::SandboxId;
use crate::sys::CliSys;
use crate::util::display;

const JSON_SCHEMA_VERSION: u8 = 1;

pub async fn audit(
  flags: Arc<Flags>,
  audit_flags: AuditFlags,
) -> Result<(), AnyError> {
  let factory = CliFactory::from_flags(flags);
  let cli_options = factory.cli_options()?;
  let sys = CliSys::default();
  let config = cli_options.workspace().to_isolate_config()?;
  isolate::check_folders(
    &config,
    &cli_options.workspace().root_dir_path(),
    &sys,
  )?;
  // A broken `extensions` key is reported where extensions load.
  if let Ok(extensions) = cli_options.workspace().to_extensions_config() {
    isolate::check_extensions(&config, &extensions, jsr_url(), &sys)?;
  }
  let app = cli_options.permissions_options()?;
  let parser = factory.permission_desc_parser()?;

  // What the lockfile pinned before this run: building the graph adds what
  // it finds, and audit never writes the lockfile.
  let pinned = factory.maybe_lockfile().await?.map(|lockfile| {
    let lockfile = lockfile.lock();
    Pinned {
      remote: lockfile.remote().keys().cloned().collect(),
      jsr: lockfile.content.packages.jsr.keys().cloned().collect(),
    }
  });

  let main_graph_container = factory.main_module_graph_container().await?;
  let roots = main_graph_container.collect_specifiers(
    &audit_flags.files,
    CollectSpecifiersOptions {
      include_ignored_specified: true,
    },
  )?;
  let module_graph_creator = factory.module_graph_creator().await?;
  let graph = module_graph_creator
    .create_graph(GraphKind::CodeOnly, roots)
    .await?;
  let failed = IntegrityFailures::of(&graph, jsr_url());
  // A failed integrity check is reported, not an error: the graph is
  // otherwise only checked when every module loaded as pinned (checking it
  // with such a failure would exit).
  if failed.is_empty() {
    module_graph_creator.graph_valid(&graph)?;
  }

  let classification = isolate::classify(&graph, &config, jsr_url(), &sys);

  let mut effective_by_rule = Vec::with_capacity(config.rules.len());
  for rule in config.rules.values() {
    let grant = extension_grant_permissions_options(&rule.permissions)?;
    let effective = sandbox_effective(&app, &grant, parser.as_ref())?;
    effective_by_rule.push((grant, effective));
  }

  let entries = entries(&graph, &classification, &pinned, &failed, jsr_url());
  let report = entries
    .iter()
    .map(|(key, entry)| {
      let rule = key.sandbox.and_then(|id| {
        let (matcher, rule) = config.rules.get_index(id.0)?;
        Some((matcher, rule, &effective_by_rule[id.0]))
      });
      ReportEntry {
        name: key.unit.name(&config),
        kind: key.unit.kind(&config),
        matcher: rule.map(|(matcher, _, _)| matcher.clone()),
        granted: rule.map(|(_, rule, _)| rule.permissions_json.clone()),
        effective: match rule {
          Some((_, _, (_, effective))) => effective.clone(),
          None => app.clone(),
        },
        can_escape: rule
          .map(|(_, _, (grant, _))| granted_kinds(grant, &["run", "ffi"]))
          .unwrap_or_default(),
        process_wide: rule
          .map(|(_, _, (grant, _))| granted_kinds(grant, &["env", "sys"]))
          .unwrap_or_default(),
        lockfile: entry.lockfile,
        modules: entry.modules,
      }
    })
    .collect::<Vec<_>>();
  let unmatched = classification
    .unmatched
    .iter()
    .filter_map(|id| config.rules.get_index(id.0))
    .map(|(matcher, _)| matcher.clone())
    .collect::<Vec<_>>();

  if audit_flags.json {
    display::write_json_to_stdout(&json!({
      "version": JSON_SCHEMA_VERSION,
      "entries": report.iter().map(ReportEntry::to_json).collect::<Vec<_>>(),
      "unmatchedRules": unmatched,
    }))
  } else {
    let mut output = String::new();
    for entry in &report {
      entry.write_text(&mut output);
    }
    if !unmatched.is_empty() {
      output
        .push_str(&format!("{}\n", colors::bold("Rules that match nothing:")));
      for warning in classification.warnings(&config) {
        output.push_str(&format!("  {warning}\n"));
      }
    }
    ende_print::drop_write_stdout(output.as_bytes());
    Ok(())
  }
}

/// What the lockfile pinned before the graph was built.
struct Pinned {
  remote: HashSet<String>,
  jsr: HashSet<PackageNv>,
}

/// The modules and packages whose code doesn't match the lockfile.
#[derive(Default)]
struct IntegrityFailures {
  modules: HashSet<ModuleSpecifier>,
  packages: HashSet<PackageNv>,
}

impl IntegrityFailures {
  fn of(graph: &ModuleGraph, jsr_url: &Url) -> Self {
    let mut failures = Self::default();
    for error in graph.module_errors() {
      let ModuleErrorKind::Load { specifier, err, .. } = error.as_kind() else {
        continue;
      };
      match err {
        ModuleLoadError::HttpsChecksumIntegrity(_)
        | ModuleLoadError::Jsr(JsrLoadError::ContentChecksumIntegrity(_)) => {
          failures.modules.insert(specifier.clone());
          if let Some(nv) =
            recommended_registry_package_url_to_nv(jsr_url, specifier)
          {
            failures.packages.insert(nv);
          }
        }
        ModuleLoadError::Jsr(
          JsrLoadError::PackageVersionManifestChecksumIntegrity(nv, _),
        ) => {
          failures.packages.insert(nv.as_ref().clone());
        }
        _ => {}
      }
    }
    failures
  }

  fn is_empty(&self) -> bool {
    self.modules.is_empty() && self.packages.is_empty()
  }
}

/// What a report entry is about.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Unit {
  /// A version of a JSR package.
  Jsr(PackageNv),
  /// The remote modules of one origin that no rule matches.
  Remote(String),
  /// The remote modules or the local folder a URL or folder rule matches.
  Rule(SandboxId),
  /// The local modules no rule matches: the app's own code.
  Local,
}

impl Unit {
  fn name(&self, config: &IsolateConfig) -> String {
    match self {
      Self::Jsr(nv) => format!("jsr:{nv}"),
      Self::Remote(origin) => origin.clone(),
      Self::Rule(id) => config
        .rules
        .get_index(id.0)
        .map(|(matcher, _)| matcher.clone())
        .unwrap_or_default(),
      Self::Local => "local modules (the app)".to_string(),
    }
  }

  fn kind(&self, config: &IsolateConfig) -> &'static str {
    match self {
      Self::Jsr(_) => "jsr",
      Self::Remote(_) => "remote",
      Self::Rule(id) => match config.rules.get_index(id.0) {
        Some((_, rule))
          if matches!(rule.matcher, IsolateMatcher::Folder(_)) =>
        {
          "local"
        }
        _ => "remote",
      },
      Self::Local => "local",
    }
  }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EntryKey {
  /// Sandboxed entries first, in rule order.
  order: (usize, u8),
  sandbox: Option<SandboxId>,
  unit: Unit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Lockfile {
  Pinned,
  Unpinned,
  IntegrityMismatch,
  /// No lockfile is in use.
  Missing,
  Local,
}

impl Lockfile {
  fn as_str(&self) -> &'static str {
    match self {
      Self::Pinned => "pinned",
      Self::Unpinned => "unpinned",
      Self::IntegrityMismatch => "integrity mismatch",
      Self::Missing => "no lockfile",
      Self::Local => "local",
    }
  }

  /// The status of an entry holding modules of `self` and `other`: the
  /// worst of the two.
  fn combine(self, other: Self) -> Self {
    self.max(other)
  }
}

struct Entry {
  lockfile: Lockfile,
  modules: usize,
}

/// One entry per JSR package version, per origin of remote modules, per
/// URL or folder rule, and one for the app's own local modules.
fn entries(
  graph: &ModuleGraph,
  classification: &isolate::Classification,
  pinned: &Option<Pinned>,
  failed: &IntegrityFailures,
  jsr_url: &Url,
) -> BTreeMap<EntryKey, Entry> {
  let mut entries: BTreeMap<EntryKey, Entry> = BTreeMap::new();
  let mut add = |sandbox: Option<SandboxId>, unit: Unit, lockfile: Lockfile| {
    let kind_order = match unit {
      Unit::Jsr(_) => 0,
      Unit::Remote(_) | Unit::Rule(_) => 1,
      Unit::Local => 2,
    };
    let key = EntryKey {
      order: (sandbox.map(|id| id.0).unwrap_or(usize::MAX), kind_order),
      sandbox,
      unit,
    };
    let entry = entries.entry(key).or_insert(Entry {
      lockfile,
      modules: 0,
    });
    entry.lockfile = entry.lockfile.combine(lockfile);
    entry.modules += 1;
  };
  let mut seen = HashSet::new();
  for (_, module) in graph.specifiers() {
    let specifier = match module {
      Ok(module) => module.specifier(),
      Err(error) => error.specifier(),
    };
    if !seen.insert(specifier) {
      continue;
    }
    let sandbox = classification.sandboxes.get(specifier).copied();
    // A package whose version manifest fails its check is in the graph as
    // the `jsr:` specifier it was imported by.
    let failed_package = match module {
      Err(error) => match error.as_kind() {
        ModuleErrorKind::Load {
          err:
            ModuleLoadError::Jsr(
              JsrLoadError::PackageVersionManifestChecksumIntegrity(nv, _),
            ),
          ..
        } => Some(nv.as_ref().clone()),
        _ => None,
      },
      Ok(_) => None,
    };
    if let Some(nv) = recommended_registry_package_url_to_nv(jsr_url, specifier)
      .or(failed_package)
    {
      let lockfile = match pinned {
        _ if failed.packages.contains(&nv) => Lockfile::IntegrityMismatch,
        None => Lockfile::Missing,
        Some(pinned) if pinned.jsr.contains(&nv) => Lockfile::Pinned,
        Some(_) => Lockfile::Unpinned,
      };
      add(sandbox, Unit::Jsr(nv), lockfile);
      continue;
    }
    match specifier.scheme() {
      "file" => add(
        sandbox,
        sandbox.map(Unit::Rule).unwrap_or(Unit::Local),
        Lockfile::Local,
      ),
      "http" | "https" => {
        let lockfile = match pinned {
          _ if failed.modules.contains(specifier) => {
            Lockfile::IntegrityMismatch
          }
          None => Lockfile::Missing,
          Some(pinned) if pinned.remote.contains(specifier.as_str()) => {
            Lockfile::Pinned
          }
          Some(_) => Lockfile::Unpinned,
        };
        let unit = match sandbox {
          Some(id) => Unit::Rule(id),
          None => Unit::Remote(specifier.origin().ascii_serialization()),
        };
        add(sandbox, unit, lockfile);
      }
      // data:, blob: and extension: modules belong to their importer.
      _ => {}
    }
  }
  entries
}

struct ReportEntry {
  name: String,
  kind: &'static str,
  matcher: Option<String>,
  granted: Option<Value>,
  effective: PermissionsOptions,
  can_escape: Vec<&'static str>,
  process_wide: Vec<&'static str>,
  lockfile: Lockfile,
  modules: usize,
}

impl ReportEntry {
  fn to_json(&self) -> Value {
    let lists = |lists: [(&'static str, Option<Vec<String>>); 8]| {
      lists
        .into_iter()
        .filter_map(|(name, list)| Some((name.to_string(), json!(list?))))
        .collect::<serde_json::Map<_, _>>()
    };
    let effective = json!({
      "allow": lists(allow_lists(&self.effective).map(|(n, l)| (n, l.clone()))),
      "deny": lists(effective_deny_lists(&self.effective)),
      "prompts": may_prompt(&self.effective),
    });
    json!({
      "name": self.name,
      "kind": self.kind,
      "sandboxed": self.matcher.is_some(),
      "matcher": self.matcher,
      "granted": self.granted,
      // Manifests of sandboxed packages aren't read yet.
      "requested": null,
      "effective": effective,
      "canEscape": self.can_escape,
      "processWide": self.process_wide,
      "lockfile": self.lockfile.as_str(),
      "modules": self.modules,
    })
  }

  fn write_text(&self, output: &mut String) {
    let dash = "—".to_string();
    let effective = describe(&self.effective);
    let (sandboxed, effective) = match &self.matcher {
      Some(_) => ("yes".to_string(), effective),
      None => (
        "no, runs in the app".to_string(),
        format!("the app's: {effective}"),
      ),
    };
    let mut lines = vec![
      ("sandboxed", sandboxed),
      (
        "matcher",
        self
          .matcher
          .as_ref()
          .map(|matcher| format!("\"{matcher}\""))
          .unwrap_or_else(|| dash.clone()),
      ),
      (
        "granted",
        self
          .granted
          .as_ref()
          .map(|granted| granted.to_string())
          .unwrap_or_else(|| dash.clone()),
      ),
      ("requested", dash.clone()),
      ("effective", effective),
    ];
    if !self.can_escape.is_empty() {
      lines.push((
        "can escape",
        format!(
          "{} (a subprocess or native code ignores Ende's permissions)",
          self.can_escape.join(", ")
        ),
      ));
    }
    if !self.process_wide.is_empty() {
      lines.push((
        "process-wide",
        format!(
          "{} (shared with the whole process, not just the sandbox)",
          self.process_wide.join(", ")
        ),
      ));
    }
    let lockfile = match self.lockfile {
      Lockfile::Local => "local, not pinned".to_string(),
      other => other.as_str().to_string(),
    };
    lines.push(("lockfile", lockfile));
    output.push_str(&format!("{}\n", colors::bold(&self.name)));
    for (label, value) in lines {
      output.push_str(&format!("  {:<14}{value}\n", format!("{label}:")));
    }
    output.push('\n');
  }
}

/// Each permission's allow list, in a fixed order. `Some(vec![])` allows
/// all of it.
fn allow_lists(
  options: &PermissionsOptions,
) -> [(&'static str, &Option<Vec<String>>); 8] {
  [
    ("read", &options.allow_read),
    ("write", &options.allow_write),
    ("net", &options.allow_net),
    ("env", &options.allow_env),
    ("sys", &options.allow_sys),
    ("run", &options.allow_run),
    ("ffi", &options.allow_ffi),
    ("import", &options.allow_import),
  ]
}

/// Each permission's deny list, with its ignore list (ignored is denied
/// too), in the order of `allow_lists`. `Some(vec![])` denies all of it.
fn deny_lists(
  options: &PermissionsOptions,
) -> [(&'static str, Option<Vec<String>>); 8] {
  let merged = |deny: &Option<Vec<String>>, ignore: &Option<Vec<String>>| {
    let mut list = deny.clone();
    merge_deny(&mut list, ignore);
    list
  };
  [
    ("read", merged(&options.deny_read, &options.ignore_read)),
    ("write", options.deny_write.clone()),
    ("net", options.deny_net.clone()),
    ("env", merged(&options.deny_env, &options.ignore_env)),
    ("sys", options.deny_sys.clone()),
    ("run", options.deny_run.clone()),
    ("ffi", options.deny_ffi.clone()),
    ("import", options.deny_import.clone()),
  ]
}

/// The deny lists that narrow something `options` allows. A denial of a
/// permission that isn't allowed at all changes nothing, so it's left out.
fn effective_deny_lists(
  options: &PermissionsOptions,
) -> [(&'static str, Option<Vec<String>>); 8] {
  let allowed = allow_lists(options);
  let mut denied = deny_lists(options);
  for ((_, allow), (_, deny)) in allowed.iter().zip(denied.iter_mut()) {
    if allow.is_none() {
      *deny = None;
    }
  }
  denied
}

/// Adds the entries of `from` to the deny list `into`. An empty list denies
/// everything, so it absorbs the other.
fn merge_deny(into: &mut Option<Vec<String>>, from: &Option<Vec<String>>) {
  let Some(from) = from else {
    return;
  };
  match into {
    None => *into = Some(from.clone()),
    Some(list) if list.is_empty() => {}
    Some(_) if from.is_empty() => *into = Some(Vec::new()),
    Some(list) => {
      for entry in from {
        if !list.contains(entry) {
          list.push(entry.clone());
        }
      }
    }
  }
}

/// What a sandbox may do: what both its grant and the app allow, minus
/// what either denies. Never prompts.
fn sandbox_effective(
  app: &PermissionsOptions,
  grant: &PermissionsOptions,
  parser: &dyn PermissionDescriptorParser,
) -> Result<PermissionsOptions, AnyError> {
  // Keeps the app's deny lists; the grant's are added below.
  let (mut effective, _) = app.intersect_requested(grant, parser)?;
  for (into, from) in [
    (&mut effective.deny_read, &grant.deny_read),
    (&mut effective.ignore_read, &grant.ignore_read),
    (&mut effective.deny_write, &grant.deny_write),
    (&mut effective.deny_net, &grant.deny_net),
    (&mut effective.deny_env, &grant.deny_env),
    (&mut effective.ignore_env, &grant.ignore_env),
    (&mut effective.deny_sys, &grant.deny_sys),
    (&mut effective.deny_run, &grant.deny_run),
    (&mut effective.deny_ffi, &grant.deny_ffi),
    (&mut effective.deny_import, &grant.deny_import),
  ] {
    merge_deny(into, from);
  }
  effective.prompt = false;
  Ok(effective)
}

/// Whether code running with `options` can be prompted for more than it is
/// allowed: prompting is on and some permission isn't allowed whole.
fn may_prompt(options: &PermissionsOptions) -> bool {
  options.prompt
    && allow_lists(options)
      .iter()
      .any(|(_, list)| !matches!(list.as_deref(), Some([])))
}

/// The permissions `options` allows, like `read: /data; deny read:
/// /data/secret; net`, then ` (+ prompts)` if it may prompt for more.
fn describe(options: &PermissionsOptions) -> String {
  let mut parts = Vec::new();
  for ((name, allow), (_, deny)) in allow_lists(options)
    .into_iter()
    .zip(effective_deny_lists(options))
  {
    match allow.as_deref() {
      None => continue,
      Some([]) => parts.push(name.to_string()),
      Some(list) => parts.push(format!("{name}: {}", list.join(", "))),
    }
    match deny.as_deref() {
      None => {}
      Some([]) => parts.push(format!("deny {name}")),
      Some(list) => parts.push(format!("deny {name}: {}", list.join(", "))),
    }
  }
  let mut text = if parts.is_empty() {
    "none".to_string()
  } else {
    parts.join("; ")
  };
  if may_prompt(options) {
    text.push_str(" (+ prompts)");
  }
  text
}

/// Which of `names` the grant allows at all.
fn granted_kinds(
  grant: &PermissionsOptions,
  names: &[&'static str],
) -> Vec<&'static str> {
  allow_lists(grant)
    .into_iter()
    .filter(|(name, list)| names.contains(name) && list.is_some())
    .map(|(name, _)| name)
    .collect()
}

#[cfg(test)]
mod test {
  use pretty_assertions::assert_eq;

  use super::*;

  fn strings(list: &[&str]) -> Option<Vec<String>> {
    Some(list.iter().map(|s| s.to_string()).collect())
  }

  #[test]
  fn describe_lists_allowed_and_denied_permissions() {
    assert_eq!(describe(&PermissionsOptions::default()), "none");
    assert_eq!(
      describe(&PermissionsOptions {
        allow_read: strings(&["/a", "/b"]),
        ignore_read: strings(&["/a/x"]),
        allow_net: Some(vec![]),
        deny_net: strings(&["x.com"]),
        // Nothing to narrow: env isn't allowed.
        deny_env: strings(&["HOME"]),
        ..Default::default()
      }),
      "read: /a, /b; deny read: /a/x; net; deny net: x.com"
    );
  }

  #[test]
  fn describe_marks_prompting() {
    let prompting = PermissionsOptions {
      allow_read: strings(&["/a"]),
      prompt: true,
      ..Default::default()
    };
    assert_eq!(describe(&prompting), "read: /a (+ prompts)");
    assert!(may_prompt(&prompting));
    // With everything allowed there is nothing left to prompt for.
    let all = PermissionsOptions {
      allow_read: Some(vec![]),
      allow_write: Some(vec![]),
      allow_net: Some(vec![]),
      allow_env: Some(vec![]),
      allow_sys: Some(vec![]),
      allow_run: Some(vec![]),
      allow_ffi: Some(vec![]),
      allow_import: Some(vec![]),
      prompt: true,
      ..Default::default()
    };
    assert!(!may_prompt(&all));
  }

  #[test]
  fn sandbox_keeps_the_grants_and_the_apps_denials() {
    let parser =
      ende_runtime::ende_permissions::RuntimePermissionDescriptorParser::new(
        CliSys::default(),
      );
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let data = std::path::Path::new(root).join("app").join("data");
    let path = |path: &std::path::Path| path.to_string_lossy().into_owned();
    let app = PermissionsOptions {
      allow_read: Some(vec![]),
      deny_read: Some(vec![path(&data.join("app_secret"))]),
      allow_net: Some(vec![]),
      prompt: true,
      ..Default::default()
    };
    let grant = PermissionsOptions {
      allow_read: Some(vec![path(&data)]),
      deny_read: Some(vec![path(&data.join("secret"))]),
      allow_net: Some(vec![]),
      deny_net: strings(&["evil.example.com"]),
      ..Default::default()
    };
    let effective = sandbox_effective(&app, &grant, &parser).unwrap();
    assert_eq!(effective.allow_read, Some(vec![path(&data)]));
    assert_eq!(
      effective.deny_read,
      Some(vec![
        path(&data.join("app_secret")),
        path(&data.join("secret"))
      ])
    );
    assert_eq!(effective.deny_net, strings(&["evil.example.com"]));
    assert!(!effective.prompt);
  }

  #[test]
  fn an_empty_deny_list_denies_everything() {
    let mut list = strings(&["a"]);
    merge_deny(&mut list, &Some(vec![]));
    assert_eq!(list, Some(vec![]));
    merge_deny(&mut list, &strings(&["b"]));
    assert_eq!(list, Some(vec![]));
    let mut list = None;
    merge_deny(&mut list, &strings(&["b"]));
    merge_deny(&mut list, &strings(&["b", "c"]));
    assert_eq!(list, strings(&["b", "c"]));
  }

  #[test]
  fn run_and_ffi_can_escape_env_and_sys_are_process_wide() {
    let grant = PermissionsOptions {
      allow_run: Some(vec!["git".to_string()]),
      allow_env: Some(vec![]),
      allow_read: Some(vec![]),
      ..Default::default()
    };
    assert_eq!(granted_kinds(&grant, &["run", "ffi"]), vec!["run"]);
    assert_eq!(granted_kinds(&grant, &["env", "sys"]), vec!["env"]);
  }

  #[tokio::test]
  async fn a_redirected_module_is_listed_under_its_rule() {
    use deno_graph::source::MemoryLoader;
    use deno_graph::source::Source;
    let module = |specifier: &str, content: &str| {
      (
        specifier.to_string(),
        Source::Module {
          specifier: specifier.to_string(),
          maybe_headers: None,
          content: content.to_string(),
        },
      )
    };
    let loader = MemoryLoader::new(
      vec![
        module(
          "file:///app/main.ts",
          "import 'https://cdn.example.com/lib/a.js';
           import 'https://cdn.example.com/lib/b.js';",
        ),
        module("https://cdn.example.com/lib/a.js", ""),
        (
          "https://cdn.example.com/lib/b.js".to_string(),
          Source::Redirect("https://other.test/b.js".to_string()),
        ),
        module("https://other.test/b.js", ""),
      ],
      Vec::new(),
    );
    let analyzer = deno_graph::ast::CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    graph
      .build(
        vec![Url::parse("file:///app/main.ts").unwrap()],
        Vec::new(),
        &loader,
        deno_graph::BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    let config = IsolateConfig::parse(
      json!({ "https://cdn.example.com/lib/": { "permissions": [] } }),
      &Url::parse("file:///app/ende.json").unwrap(),
    )
    .unwrap();
    let jsr_url = Url::parse("https://registry.test/").unwrap();
    let classification =
      isolate::classify(&graph, &config, &jsr_url, &CliSys::default());
    let entries = entries(
      &graph,
      &classification,
      &None,
      &IntegrityFailures::default(),
      &jsr_url,
    );
    let listed = entries
      .iter()
      .map(|(key, entry)| (key.unit.name(&config), entry.modules))
      .collect::<Vec<_>>();
    // b.js runs from other.test, but in the lib/ sandbox: no entry of its
    // own for other.test.
    assert_eq!(
      listed,
      vec![
        ("https://cdn.example.com/lib/".to_string(), 2),
        ("local modules (the app)".to_string(), 1),
      ]
    );
  }

  #[test]
  fn the_worst_lockfile_status_wins() {
    assert_eq!(
      Lockfile::Pinned.combine(Lockfile::Unpinned),
      Lockfile::Unpinned
    );
    assert_eq!(
      Lockfile::IntegrityMismatch.combine(Lockfile::Pinned),
      Lockfile::IntegrityMismatch
    );
  }
}
