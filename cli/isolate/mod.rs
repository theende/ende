// Copyright 2018-2026 the Ende authors. MIT license.

//! Per-package permissions (`isolate` in ende.json): which modules of a graph
//! fall under which rule. Nothing is enforced yet; `ende audit` reports the
//! classification.
//!
//! Modules are matched by their resolved specifier, the one the graph holds
//! after the import map, so a remapped import can't skip its sandbox
//! unnoticed: a rule that then matches nothing is reported.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use deno_graph::ModuleGraph;
use deno_graph::source::recommended_registry_package_url_to_nv;
use deno_path_util::url_to_file_path;
use deno_terminal::colors;
use ende_config::ende_json::ExtensionsConfig;
use ende_config::ende_json::IsolateConfig;
use ende_config::ende_json::IsolateConfigError;
use ende_config::ende_json::IsolateMatcher;
use ende_config::workspace::Workspace;
use ende_core::ModuleSpecifier;
use ende_core::url::Url;
use sys_traits::FsCanonicalize;

/// A sandbox: the index of its rule in `IsolateConfig::rules`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SandboxId(pub usize);

/// Which rule each module of a graph falls under.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Classification {
  /// The modules some rule matches. A module matched by several rules
  /// belongs to the first one written. Every other module runs in the app.
  pub sandboxes: BTreeMap<ModuleSpecifier, SandboxId>,
  /// Rules that match no module of the graph, in declaration order.
  pub unmatched: Vec<SandboxId>,
}

impl Classification {
  /// A warning for each rule that matches nothing.
  pub fn warnings(&self, config: &IsolateConfig) -> Vec<String> {
    self
      .unmatched
      .iter()
      .filter_map(|id| config.rules.get_index(id.0))
      .map(|(matcher, _)| unmatched_rule_warning(matcher))
      .collect()
  }
}

pub fn unmatched_rule_warning(matcher: &str) -> String {
  format!(
    "isolate[\"{matcher}\"] matches no module in the graph, so it sandboxes nothing. Rules match resolved specifiers, after the import map."
  )
}

/// Puts each module of `graph`, including those that failed to load, under
/// the first rule of `config` that matches it.
///
/// - A module is matched by its final specifier and by every specifier that
///   redirects to it, through any number of hops, so a redirect out of a
///   sandboxed URL prefix doesn't take its module out of the sandbox. The
///   lowest-numbered rule matching any of these names wins.
/// - `jsr:@scope/name` matches the package's modules on the registry at
///   `jsr_url`, whatever their version.
/// - A local folder matches a module when the module's path, as the graph
///   holds it or with symlinks resolved, is inside the folder, as written or
///   with symlinks resolved. The graph keeps the path a module was imported
///   by, so this catches a sandboxed folder reached through a symlink, and
///   a symlink in the folder that points out of it.
pub fn classify(
  graph: &ModuleGraph,
  config: &IsolateConfig,
  jsr_url: &Url,
  sys: &impl FsCanonicalize,
) -> Classification {
  let rules = config
    .rules
    .values()
    .map(|rule| Rule::new(&rule.matcher, sys))
    .collect::<Vec<_>>();
  // Every name of each module: its final specifier, then the specifiers
  // that redirect to it.
  let mut names: BTreeMap<&ModuleSpecifier, Vec<&ModuleSpecifier>> =
    BTreeMap::new();
  for (_, module) in graph.specifiers() {
    let specifier = match module {
      Ok(module) => module.specifier(),
      Err(error) => error.specifier(),
    };
    names.entry(specifier).or_insert_with(|| vec![specifier]);
  }
  for from in graph.redirects.keys() {
    if let Some(list) = names.get_mut(graph.resolve(from)) {
      list.push(from);
    }
  }
  let mut sandboxes = BTreeMap::new();
  for (specifier, names) in names {
    let names = names
      .into_iter()
      .map(|name| (name, paths_of(name, sys)))
      .collect::<Vec<_>>();
    if let Some(index) = rules.iter().position(|rule| {
      names
        .iter()
        .any(|(name, paths)| rule.matches(name, paths, jsr_url))
    }) {
      sandboxes.insert(specifier.clone(), SandboxId(index));
    }
  }
  let unmatched = (0..rules.len())
    .map(SandboxId)
    .filter(|id| !sandboxes.values().any(|matched| matched == id))
    .collect();
  Classification {
    sandboxes,
    unmatched,
  }
}

/// Errors if a local folder rule holds the project root once symlinks are
/// resolved. The config check compares folders as written only.
pub fn check_folders(
  config: &IsolateConfig,
  project_root: &Path,
  sys: &impl FsCanonicalize,
) -> Result<(), IsolateConfigError> {
  let project_root = sys
    .fs_canonicalize(project_root)
    .unwrap_or_else(|_| project_root.to_path_buf());
  for (matcher, rule) in &config.rules {
    let IsolateMatcher::Folder(folder) = &rule.matcher else {
      continue;
    };
    if paths_of(folder, sys)
      .iter()
      .any(|folder| project_root.starts_with(folder))
    {
      return Err(IsolateConfigError::FolderHoldsProjectRoot {
        matcher: matcher.clone(),
      });
    }
  }
  Ok(())
}

/// Errors if a rule matches the source of a declared extension, comparing
/// resolved forms: a `jsr:@scope/name` rule matches an `https:` source on
/// the registry at `jsr_url`, and a folder rule overlaps a local
/// extension's folder (either holding the other) through symlinks too. The
/// config check compares as written only. An extension's code and its
/// dependencies are governed by its own grant.
pub fn check_extensions(
  config: &IsolateConfig,
  extensions: &ExtensionsConfig,
  jsr_url: &Url,
  sys: &impl FsCanonicalize,
) -> Result<(), IsolateConfigError> {
  for (matcher, rule) in &config.rules {
    let rule = Rule::new(&rule.matcher, sys);
    for (name, extension) in &extensions.extensions {
      let source_paths = paths_of(&extension.source, sys);
      let folder_paths = source_paths
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect::<Vec<_>>();
      let folders_overlap = rule.folder_paths.iter().any(|rule_folder| {
        folder_paths.iter().any(|folder| {
          folder.starts_with(rule_folder) || rule_folder.starts_with(folder)
        })
      });
      if rule.matches(&extension.source, &source_paths, jsr_url)
        || jsr_source_overlaps_url_rule(
          rule.matcher,
          &extension.source,
          jsr_url,
        )
      {
        return Err(IsolateConfigError::MatchesExtension {
          matcher: matcher.clone(),
          extension: name.clone(),
        });
      }
      if folders_overlap {
        return Err(IsolateConfigError::OverlapsExtensionFolder {
          matcher: matcher.clone(),
          extension: name.clone(),
        });
      }
    }
  }
  Ok(())
}

/// Whether a URL rule overlaps the code a `jsr:` source can load. Without
/// resolving its version, a `jsr:@scope/name` source covers every URL under
/// `<registry>/@scope/name/`: a URL-prefix rule collides if either holds the
/// other, and an exact-URL rule if it lies under that folder.
fn jsr_source_overlaps_url_rule(
  matcher: &IsolateMatcher,
  source: &Url,
  jsr_url: &Url,
) -> bool {
  if source.scheme() != "jsr" {
    return false;
  }
  let Ok(reference) =
    deno_semver::jsr::JsrPackageReqReference::from_specifier(source)
  else {
    return false;
  };
  let Ok(package_url) = jsr_url.join(&format!("{}/", reference.req().name))
  else {
    return false;
  };
  let package = package_url.as_str();
  match matcher {
    IsolateMatcher::UrlPrefix(prefix) => {
      package.starts_with(prefix.as_str())
        || prefix.as_str().starts_with(package)
    }
    IsolateMatcher::Url(url) => url.as_str().starts_with(package),
    IsolateMatcher::JsrPackage(_) | IsolateMatcher::Folder(_) => false,
  }
}

/// Whether the "not enforced yet" warning was printed in this process.
static NOT_ENFORCED_WARNED: AtomicBool = AtomicBool::new(false);

/// Prints, once per process, that `isolate` rules aren't enforced yet, when
/// the workspace root declares any (or an `isolate` key that doesn't
/// parse): code under them still runs with the app's permissions.
pub fn warn_not_enforced(workspace: &Workspace) {
  let declared = match workspace.to_isolate_config() {
    Ok(config) => !config.rules.is_empty(),
    Err(_) => true,
  };
  if declared && !NOT_ENFORCED_WARNED.swap(true, Ordering::Relaxed) {
    log::warn!("{} {}", colors::yellow("Warning"), NOT_ENFORCED_WARNING);
  }
}

pub const NOT_ENFORCED_WARNING: &str = "\"isolate\" rules are not enforced yet: sandboxed dependencies still run with the app's permissions.";

struct Rule<'a> {
  matcher: &'a IsolateMatcher,
  /// For a folder: its path as written and with symlinks resolved.
  folder_paths: Vec<PathBuf>,
}

impl<'a> Rule<'a> {
  fn new(matcher: &'a IsolateMatcher, sys: &impl FsCanonicalize) -> Self {
    let folder_paths = match matcher {
      IsolateMatcher::Folder(folder) => paths_of(folder, sys),
      _ => Vec::new(),
    };
    Self {
      matcher,
      folder_paths,
    }
  }

  fn matches(
    &self,
    specifier: &ModuleSpecifier,
    module_paths: &[PathBuf],
    jsr_url: &Url,
  ) -> bool {
    match self.matcher {
      // A `jsr:` specifier is in the graph only when its package failed
      // to load.
      IsolateMatcher::JsrPackage(name) => {
        recommended_registry_package_url_to_nv(jsr_url, specifier)
          .is_some_and(|nv| nv.name.as_str() == name)
          || self.matcher.matches_specifier(specifier)
      }
      IsolateMatcher::UrlPrefix(_) | IsolateMatcher::Url(_) => {
        self.matcher.matches_specifier(specifier)
      }
      IsolateMatcher::Folder(_) => module_paths.iter().any(|path| {
        self
          .folder_paths
          .iter()
          .any(|folder| path.starts_with(folder))
      }),
    }
  }
}

/// The path of a `file:` URL as written, then with symlinks resolved if
/// that differs. Empty for other URLs.
fn paths_of(url: &Url, sys: &impl FsCanonicalize) -> Vec<PathBuf> {
  if url.scheme() != "file" {
    return Vec::new();
  }
  let Ok(path) = url_to_file_path(url) else {
    return Vec::new();
  };
  let mut paths = vec![path];
  if let Ok(real) = sys.fs_canonicalize(&paths[0])
    && real != paths[0]
  {
    paths.push(real);
  }
  paths
}

#[cfg(test)]
mod test {
  use deno_graph::GraphKind;
  use deno_graph::source::MemoryLoader;
  use deno_graph::source::ResolveError;
  use deno_graph::source::Resolver;
  use deno_graph::source::Source;
  use ende_core::serde_json::json;
  use pretty_assertions::assert_eq;

  use super::*;
  use crate::sys::CliSys;

  /// Not jsr.io: deno_graph would load modules there as JSR packages,
  /// which needs their metadata. The classification only sees the URLs.
  fn jsr_url() -> Url {
    Url::parse("https://registry.test/").unwrap()
  }

  fn config(json: ende_core::serde_json::Value, base: &Url) -> IsolateConfig {
    IsolateConfig::parse(json, base).unwrap()
  }

  /// An import map, for the remapping test.
  #[derive(Debug)]
  struct ImportMapResolver(import_map::ImportMap);

  impl Resolver for ImportMapResolver {
    fn resolve(
      &self,
      specifier_text: &str,
      referrer_range: &deno_graph::Range,
      _kind: deno_graph::source::ResolutionKind,
    ) -> Result<ModuleSpecifier, ResolveError> {
      self
        .0
        .resolve(specifier_text, &referrer_range.specifier)
        .map_err(|err| {
          ResolveError::Other(deno_error::JsErrorBox::generic(err.to_string()))
        })
    }
  }

  /// A graph from `root`, whose modules are `modules` (specifier, source).
  async fn graph(
    root: &str,
    modules: &[(&str, &str)],
    resolver: Option<&dyn Resolver>,
  ) -> ModuleGraph {
    let loader = MemoryLoader::new(
      modules
        .iter()
        .map(|(specifier, content)| {
          (
            specifier.to_string(),
            Source::Module {
              specifier: specifier.to_string(),
              maybe_headers: None,
              content: content.to_string(),
            },
          )
        })
        .collect(),
      Vec::new(),
    );
    let analyzer = deno_graph::ast::CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    graph
      .build(
        vec![Url::parse(root).unwrap()],
        Vec::new(),
        &loader,
        deno_graph::BuildOptions {
          module_analyzer: &analyzer,
          resolver,
          ..Default::default()
        },
      )
      .await;
    graph
  }

  fn sandboxed(
    classification: &Classification,
    config: &IsolateConfig,
  ) -> Vec<(String, String)> {
    classification
      .sandboxes
      .iter()
      .map(|(specifier, id)| {
        (
          specifier.to_string(),
          config.rules.get_index(id.0).unwrap().0.clone(),
        )
      })
      .collect()
  }

  #[tokio::test]
  async fn jsr_rule_matches_every_version_and_entry() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let graph = graph(
      "file:///app/main.ts",
      &[
        (
          "file:///app/main.ts",
          "import 'https://registry.test/@std/csv/1.0.0/mod.ts';
           import 'https://registry.test/@std/csv/0.224.0/parse.ts';
           import 'https://registry.test/@std/csvx/1.0.0/mod.ts';",
        ),
        (
          "https://registry.test/@std/csv/1.0.0/mod.ts",
          "import './stringify.ts';",
        ),
        ("https://registry.test/@std/csv/1.0.0/stringify.ts", ""),
        ("https://registry.test/@std/csv/0.224.0/parse.ts", ""),
        ("https://registry.test/@std/csvx/1.0.0/mod.ts", ""),
      ],
      None,
    )
    .await;
    let config =
      config(json!({ "jsr:@std/csv": { "permissions": [] } }), &base);
    let classification =
      classify(&graph, &config, &jsr_url(), &CliSys::default());
    assert_eq!(
      sandboxed(&classification, &config),
      vec![
        (
          "https://registry.test/@std/csv/0.224.0/parse.ts".to_string(),
          "jsr:@std/csv".to_string()
        ),
        (
          "https://registry.test/@std/csv/1.0.0/mod.ts".to_string(),
          "jsr:@std/csv".to_string()
        ),
        (
          "https://registry.test/@std/csv/1.0.0/stringify.ts".to_string(),
          "jsr:@std/csv".to_string()
        ),
      ]
    );
    assert_eq!(classification.unmatched, vec![]);
  }

  #[tokio::test]
  async fn jsr_rule_follows_the_registry_url() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let graph = graph(
      "file:///app/main.ts",
      &[
        (
          "file:///app/main.ts",
          "import 'http://localhost:4250/@std/csv/1.0.0/mod.ts';",
        ),
        ("http://localhost:4250/@std/csv/1.0.0/mod.ts", ""),
      ],
      None,
    )
    .await;
    let config =
      config(json!({ "jsr:@std/csv": { "permissions": [] } }), &base);
    let local_registry = Url::parse("http://localhost:4250/").unwrap();
    let classification =
      classify(&graph, &config, &local_registry, &CliSys::default());
    assert_eq!(classification.sandboxes.len(), 1);
    // The same module isn't a JSR module for another registry.
    let classification =
      classify(&graph, &config, &jsr_url(), &CliSys::default());
    assert_eq!(classification.sandboxes.len(), 0);
  }

  #[tokio::test]
  async fn url_prefix_and_exact_url() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let graph = graph(
      "file:///app/main.ts",
      &[
        (
          "file:///app/main.ts",
          "import 'https://cdn.example.com/lib/a.ts';
           import 'https://cdn.example.com/lib/sub/b.ts';
           import 'https://cdn.example.com/library.ts';
           import 'https://cdn.example.com/one/x.ts';
           import 'https://cdn.example.com/one/y.ts';",
        ),
        ("https://cdn.example.com/lib/a.ts", ""),
        ("https://cdn.example.com/lib/sub/b.ts", ""),
        ("https://cdn.example.com/library.ts", ""),
        ("https://cdn.example.com/one/x.ts", ""),
        ("https://cdn.example.com/one/y.ts", ""),
      ],
      None,
    )
    .await;
    let config = config(
      json!({
        "https://cdn.example.com/lib/": { "permissions": [] },
        "https://cdn.example.com/one/x.ts": { "permissions": [] },
      }),
      &base,
    );
    let classification =
      classify(&graph, &config, &jsr_url(), &CliSys::default());
    assert_eq!(
      sandboxed(&classification, &config),
      vec![
        (
          "https://cdn.example.com/lib/a.ts".to_string(),
          "https://cdn.example.com/lib/".to_string()
        ),
        (
          "https://cdn.example.com/lib/sub/b.ts".to_string(),
          "https://cdn.example.com/lib/".to_string()
        ),
        (
          "https://cdn.example.com/one/x.ts".to_string(),
          "https://cdn.example.com/one/x.ts".to_string()
        ),
      ]
    );
  }

  #[tokio::test]
  async fn redirects_keep_a_module_under_its_rule() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let redirect =
      |to: &str| Source::<String, String>::Redirect(to.to_string());
    let module = |specifier: &str| Source::Module {
      specifier: specifier.to_string(),
      maybe_headers: None,
      content: String::new(),
    };
    let loader = MemoryLoader::new(
      vec![
        (
          "file:///app/main.ts".to_string(),
          Source::Module {
            specifier: "file:///app/main.ts".to_string(),
            maybe_headers: None,
            content: "import 'https://cdn.example.com/lib/a.js';
              import 'https://cdn.example.com/lib/b.js';
              import 'https://short.test/c.js';
              import 'https://cdn.example.com/lib/d.js';
              import 'https://other.test/plain.js';"
              .to_string(),
          },
        ),
        (
          "https://cdn.example.com/lib/a.js".to_string(),
          module("https://cdn.example.com/lib/a.js"),
        ),
        // Out of the prefix, to another host.
        (
          "https://cdn.example.com/lib/b.js".to_string(),
          redirect("https://other.test/b.js"),
        ),
        (
          "https://other.test/b.js".to_string(),
          module("https://other.test/b.js"),
        ),
        // From outside into the prefix.
        (
          "https://short.test/c.js".to_string(),
          redirect("https://cdn.example.com/lib/c.js"),
        ),
        (
          "https://cdn.example.com/lib/c.js".to_string(),
          module("https://cdn.example.com/lib/c.js"),
        ),
        // Two hops out of the prefix.
        (
          "https://cdn.example.com/lib/d.js".to_string(),
          redirect("https://hop.test/d.js"),
        ),
        (
          "https://hop.test/d.js".to_string(),
          redirect("https://final.test/d.js"),
        ),
        (
          "https://final.test/d.js".to_string(),
          module("https://final.test/d.js"),
        ),
        (
          "https://other.test/plain.js".to_string(),
          module("https://other.test/plain.js"),
        ),
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
    graph.valid().unwrap();
    let rules = config(
      json!({
        "https://other.test/": { "permissions": ["net"] },
        "https://cdn.example.com/lib/": { "permissions": [] },
      }),
      &base,
    );
    let classification =
      classify(&graph, &rules, &jsr_url(), &CliSys::default());
    assert_eq!(
      sandboxed(&classification, &rules),
      vec![
        (
          "https://cdn.example.com/lib/a.js".to_string(),
          "https://cdn.example.com/lib/".to_string()
        ),
        (
          "https://cdn.example.com/lib/c.js".to_string(),
          "https://cdn.example.com/lib/".to_string()
        ),
        (
          "https://final.test/d.js".to_string(),
          "https://cdn.example.com/lib/".to_string()
        ),
        // Matched by both rules, through its redirect and its own URL: the
        // first rule written wins.
        (
          "https://other.test/b.js".to_string(),
          "https://other.test/".to_string()
        ),
        (
          "https://other.test/plain.js".to_string(),
          "https://other.test/".to_string()
        ),
      ]
    );
    // Without the other.test rule, b.js stays in the lib/ sandbox.
    let rules = config(
      json!({ "https://cdn.example.com/lib/": { "permissions": [] } }),
      &base,
    );
    let classification =
      classify(&graph, &rules, &jsr_url(), &CliSys::default());
    assert_eq!(
      classification
        .sandboxes
        .get(&Url::parse("https://other.test/b.js").unwrap()),
      Some(&SandboxId(0))
    );
    assert_eq!(classification.sandboxes.len(), 4);
  }

  #[tokio::test]
  async fn first_matching_rule_wins() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let graph = graph(
      "file:///app/main.ts",
      &[
        (
          "file:///app/main.ts",
          "import 'https://cdn.example.com/lib/a.ts';",
        ),
        ("https://cdn.example.com/lib/a.ts", ""),
      ],
      None,
    )
    .await;
    let config = config(
      json!({
        "https://cdn.example.com/lib/a.ts": { "permissions": [] },
        "https://cdn.example.com/lib/": { "permissions": ["net"] },
      }),
      &base,
    );
    let classification =
      classify(&graph, &config, &jsr_url(), &CliSys::default());
    assert_eq!(
      classification.sandboxes.values().collect::<Vec<_>>(),
      vec![&SandboxId(0)]
    );
    assert_eq!(classification.unmatched, vec![SandboxId(1)]);
  }

  #[tokio::test]
  async fn import_map_remap_is_matched_as_resolved() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let import_map = import_map::parse_from_json(
      Url::parse("file:///app/import_map.json").unwrap(),
      r#"{ "imports": { "https://cdn.example.com/lib/": "./vendor/lib/" } }"#,
    )
    .unwrap()
    .import_map;
    let resolver = ImportMapResolver(import_map);
    let graph = graph(
      "file:///app/main.ts",
      &[
        (
          "file:///app/main.ts",
          "import 'https://cdn.example.com/lib/a.ts';",
        ),
        ("file:///app/vendor/lib/a.ts", ""),
      ],
      Some(&resolver),
    )
    .await;
    let config = config(
      json!({
        "https://cdn.example.com/lib/": { "permissions": [] },
        "./vendor/lib/": { "permissions": [] },
      }),
      &base,
    );
    let classification =
      classify(&graph, &config, &jsr_url(), &CliSys::default());
    // The import as written names the URL, but the module that runs is the
    // remapped one: the URL rule matches nothing and says so.
    assert_eq!(
      sandboxed(&classification, &config),
      vec![(
        "file:///app/vendor/lib/a.ts".to_string(),
        "./vendor/lib/".to_string()
      )]
    );
    assert_eq!(classification.unmatched, vec![SandboxId(0)]);
    assert_eq!(
      classification.warnings(&config),
      vec![
        "isolate[\"https://cdn.example.com/lib/\"] matches no module in the graph, so it sandboxes nothing. Rules match resolved specifiers, after the import map.".to_string()
      ]
    );
  }

  #[tokio::test]
  async fn unmatched_rule_warns() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let graph =
      graph("file:///app/main.ts", &[("file:///app/main.ts", "")], None).await;
    let config = config(
      json!({
        "jsr:@std/csv": { "permissions": [] },
        "./vendor/": { "permissions": [] },
      }),
      &base,
    );
    let classification =
      classify(&graph, &config, &jsr_url(), &CliSys::default());
    assert_eq!(classification.sandboxes.len(), 0);
    assert_eq!(classification.unmatched, vec![SandboxId(0), SandboxId(1)]);
    assert_eq!(classification.warnings(&config).len(), 2);
  }

  #[tokio::test]
  async fn failed_modules_are_classified_too() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    // The import is never served, so the graph holds an error for it.
    let graph = graph(
      "file:///app/main.ts",
      &[(
        "file:///app/main.ts",
        "import 'https://cdn.example.com/lib/a.ts';",
      )],
      None,
    )
    .await;
    assert_eq!(graph.module_errors().count(), 1);
    let config = config(
      json!({ "https://cdn.example.com/lib/": { "permissions": [] } }),
      &base,
    );
    let classification =
      classify(&graph, &config, &jsr_url(), &CliSys::default());
    assert_eq!(classification.sandboxes.len(), 1);
  }

  fn extensions_config(
    json: ende_core::serde_json::Value,
    base: &Url,
  ) -> ExtensionsConfig {
    ExtensionsConfig::parse(json, base).unwrap()
  }

  fn extension_error(
    rules: &IsolateConfig,
    extensions: &ExtensionsConfig,
  ) -> Option<String> {
    check_extensions(rules, extensions, &jsr_url(), &CliSys::default())
      .err()
      .map(|err| err.to_string())
  }

  #[test]
  fn extension_on_the_registry_collides_with_a_jsr_rule() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let extensions = extensions_config(
      json!({
        "@acme/fmt": {
          "source": "https://registry.test/@acme/fmt/1.2.3/mod.ts",
          "permissions": [],
        },
      }),
      &base,
    );
    let rules =
      config(json!({ "jsr:@acme/fmt": { "permissions": [] } }), &base);
    assert_eq!(
      extension_error(&rules, &extensions).as_deref(),
      Some(
        "isolate[\"jsr:@acme/fmt\"] matches the source of extension \"@acme/fmt\". An extension already runs in a sandbox of its own, with the grant in extensions[\"@acme/fmt\"], so an \"isolate\" rule can't match it too."
      )
    );
    // So does a jsr: source, and an https: source under a URL rule.
    for (source, matcher) in [
      ("jsr:@acme/fmt@1", "jsr:@acme/fmt"),
      (
        "https://cdn.example.com/ext/mod.ts",
        "https://cdn.example.com/",
      ),
    ] {
      let extensions = extensions_config(
        json!({ "@acme/fmt": { "source": source, "permissions": [] } }),
        &base,
      );
      let rules = config(json!({ matcher: { "permissions": [] } }), &base);
      assert!(extension_error(&rules, &extensions).is_some(), "{source}");
    }
    // Another package, or the same one on another registry, is fine.
    for matcher in ["jsr:@acme/other", "jsr:@other/fmt"] {
      let rules = config(json!({ matcher: { "permissions": [] } }), &base);
      assert_eq!(extension_error(&rules, &extensions), None, "{matcher}");
    }
    let elsewhere = extensions_config(
      json!({
        "@acme/fmt": {
          "source": "https://other.test/@acme/fmt/1.2.3/mod.ts",
          "permissions": [],
        },
      }),
      &base,
    );
    assert_eq!(extension_error(&rules, &elsewhere), None);
  }

  #[test]
  fn jsr_extension_collides_with_url_rules_on_its_registry_folder() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let extensions = extensions_config(
      json!({ "@s/n": { "source": "jsr:@s/n@^1", "permissions": [] } }),
      &base,
    );
    for matcher in [
      // Prefixes that hold the package's folder.
      "https://registry.test/",
      "https://registry.test/@s/",
      "https://registry.test/@s/n/",
      // A prefix and an exact URL inside it.
      "https://registry.test/@s/n/1.0.0/",
      "https://registry.test/@s/n/1.0.0/mod.ts",
    ] {
      let rules = config(json!({ matcher: { "permissions": [] } }), &base);
      assert!(extension_error(&rules, &extensions).is_some(), "{matcher}");
    }
    for matcher in [
      // A neighbouring package, and the package on another host.
      "https://registry.test/@s/name2/",
      "https://registry.test/@s/name2/1.0.0/mod.ts",
      "https://other.test/@s/n/",
      // An exact URL is one module, not a folder.
      "https://registry.test/@s/n",
    ] {
      let rules = config(json!({ matcher: { "permissions": [] } }), &base);
      assert_eq!(extension_error(&rules, &extensions), None, "{matcher}");
    }
  }

  #[test]
  fn folder_rules_collide_with_an_extension_folder_either_way() {
    let base = Url::parse("file:///app/ende.json").unwrap();
    let extensions = extensions_config(
      json!({
        "@acme/fmt": { "source": "./ext/fmt/mod.ts", "permissions": [] },
      }),
      &base,
    );
    // The rule holds the extension's folder, or sits inside it.
    for matcher in ["./ext/", "./ext/fmt/", "./ext/fmt/vendor/"] {
      let rules = config(json!({ matcher: { "permissions": [] } }), &base);
      assert!(extension_error(&rules, &extensions).is_some(), "{matcher}");
    }
    let rules =
      config(json!({ "./ext/fmt/vendor/": { "permissions": [] } }), &base);
    assert!(
      extension_error(&rules, &extensions)
        .unwrap()
        .contains("overlaps the folder of extension \"@acme/fmt\"")
    );
    for matcher in ["./ext/other/", "./vendor/"] {
      let rules = config(json!({ matcher: { "permissions": [] } }), &base);
      assert_eq!(extension_error(&rules, &extensions), None, "{matcher}");
    }
  }

  /// A project in a temp folder, with `vendor/real/` holding the code and
  /// `vendor/link` a symlink to it.
  #[cfg(unix)]
  fn symlinked_project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = CliSys::default().fs_canonicalize(dir.path()).unwrap();
    let real = root.join("vendor").join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("mod.ts"), "").unwrap();
    std::fs::write(root.join("outside.ts"), "").unwrap();
    std::os::unix::fs::symlink("real", root.join("vendor").join("link"))
      .unwrap();
    std::os::unix::fs::symlink("../../outside.ts", real.join("out.ts"))
      .unwrap();
    (dir, root)
  }

  #[cfg(unix)]
  #[tokio::test]
  async fn local_folder_matches_through_symlinks() {
    let (_dir, root) = symlinked_project();
    let url = |path: &Path| deno_path_util::url_from_file_path(path).unwrap();
    let base = url(&root.join("ende.json"));
    let main = url(&root.join("main.ts"));
    let via_link = url(&root.join("vendor").join("link").join("mod.ts"));
    let via_real = url(&root.join("vendor").join("real").join("mod.ts"));
    let link_out = url(&root.join("vendor").join("real").join("out.ts"));
    let graph = graph(
      main.as_str(),
      &[
        (
          main.as_str(),
          &format!(
            "import '{via_link}'; import '{via_real}'; import '{link_out}';"
          ),
        ),
        (via_link.as_str(), ""),
        (via_real.as_str(), ""),
        (link_out.as_str(), ""),
      ],
      None,
    )
    .await;
    // The graph keeps both paths to the same file as two modules.
    assert_eq!(graph.modules().count(), 4);
    for matcher in ["./vendor/real/", "./vendor/link/"] {
      let config = config(json!({ matcher: { "permissions": [] } }), &base);
      let classification =
        classify(&graph, &config, &jsr_url(), &CliSys::default());
      let mut matched = classification
        .sandboxes
        .keys()
        .map(|specifier| specifier.to_string())
        .collect::<Vec<_>>();
      matched.sort();
      // Both paths to the sandboxed code match either way it is named, and
      // so does a symlink in the folder that points out of it.
      let mut expected = vec![
        via_link.to_string(),
        via_real.to_string(),
        link_out.to_string(),
      ];
      expected.sort();
      assert_eq!(matched, expected, "{matcher}");
    }
  }

  #[cfg(unix)]
  #[test]
  fn folder_rule_collides_with_an_extension_through_a_symlink() {
    let (_dir, root) = symlinked_project();
    let base =
      deno_path_util::url_from_file_path(&root.join("ende.json")).unwrap();
    // The extension lives in vendor/real; the rule names it through the
    // link, which the config check (as written) can't see.
    let extensions = extensions_config(
      json!({
        "@acme/fmt": { "source": "./vendor/real/mod.ts", "permissions": [] },
      }),
      &base,
    );
    let rules =
      config(json!({ "./vendor/link/": { "permissions": [] } }), &base);
    rules.check_extensions(&extensions).unwrap();
    assert!(extension_error(&rules, &extensions).is_some());
  }

  #[cfg(unix)]
  #[test]
  fn folder_holding_the_project_root_through_a_symlink() {
    let (_dir, root) = symlinked_project();
    let project = root.join("vendor").join("real");
    let base =
      deno_path_util::url_from_file_path(&project.join("ende.json")).unwrap();
    std::os::unix::fs::symlink("..", project.join("up")).unwrap();
    let rules = config(json!({ "./up/": { "permissions": [] } }), &base);
    assert_eq!(
      check_folders(&rules, &project, &CliSys::default())
        .unwrap_err()
        .to_string(),
      "isolate[\"./up/\"] is the folder that holds ende.json, or above it. A sandbox can't hold the app itself; name the folder of the code to sandbox, like \"./vendor/name/\"."
    );
    let rules = config(json!({ "./other/": { "permissions": [] } }), &base);
    check_folders(&rules, &project, &CliSys::default()).unwrap();
  }
}
