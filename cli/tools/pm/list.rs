// Copyright 2018-2026 the Deno authors. MIT license.

use std::collections::HashMap;
use std::sync::Arc;

use deno_semver::package::PackageKind;
use deno_semver::package::PackageReq;
use deno_terminal::colors;
use ende_cache_dir::GlobalOrLocalHttpCache;
use ende_cache_dir::file_fetcher::CacheSetting;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;

use super::deps::DepManager;
use super::deps::DepManagerArgs;
use super::outdated::filter::FilterSet;
use crate::args::Flags;
use crate::args::ListFlags;
use crate::factory::CliFactory;
use crate::file_fetcher::CreateCliFileFetcherOptions;
use crate::file_fetcher::create_cli_file_fetcher;
use crate::jsr::JsrFetchResolver;

/// Internal id for a jsr package: its lockfile key, e.g. `@std/async@1.2.0`.
type PkgId = String;

/// Split a `name@version` lockfile key into `(name, version)`.
fn split_name_version(s: &str) -> (&str, &str) {
  match s[1..].find('@').map(|p| p + 1) {
    Some(at_pos) => (&s[..at_pos], &s[at_pos + 1..]),
    None => (s, ""),
  }
}

/// The resolved dependency graph of the workspace, derived from the lockfile.
/// npm packages are not supported in Ende, so they're not included.
struct DepGraph {
  /// Forward edges: package -> its (resolved) dependencies.
  forward: HashMap<PkgId, Vec<PkgId>>,
}

impl DepGraph {
  fn from_lockfile_content(content: &ende_lockfile::PackagesContent) -> Self {
    let mut forward: HashMap<PkgId, Vec<PkgId>> = HashMap::new();
    for (nv, info) in content.jsr.iter() {
      let deps = info
        .dependencies
        .iter()
        .filter(|dep_req| dep_req.kind == PackageKind::Jsr)
        .filter_map(|dep_req| {
          let resolved = content.specifiers.get(dep_req)?;
          Some(format!("{}@{}", dep_req.req.name, resolved))
        })
        .collect();
      forward.insert(nv.to_string(), deps);
    }
    Self { forward }
  }

  /// Resolve a declared root (`name@version`) to its lockfile node.
  fn root_id(&self, name_version: &str) -> Option<PkgId> {
    let id = name_version.to_string();
    self.forward.contains_key(&id).then_some(id)
  }

  fn children(&self, id: &PkgId) -> &[PkgId] {
    self.forward.get(id).map(Vec::as_slice).unwrap_or(&[])
  }
}

/// A declared root dependency, after filtering.
struct Root {
  alias: Option<String>,
  name: String,
  required: String,
  resolved: Option<String>,
}

impl Root {
  fn label(&self) -> String {
    let mut label = format!("jsr:{}", self.name);
    if let Some(alias) = &self.alias {
      label = format!("{} ({})", label, alias);
    }
    match &self.resolved {
      Some(v) => format!("{} {}", label, colors::gray(v)),
      None => format!("{} {}", label, colors::gray("(unresolved)")),
    }
  }
}

// ---------------------------------------------------------------------------
// Flat output (default, --depth 0)
// ---------------------------------------------------------------------------

#[allow(clippy::print_stdout, reason = "list output")]
fn print_flat_table(roots: &[Root]) {
  const HEADINGS: &[&str] = &["Package", "Required", "Resolved"];

  let name_of = |r: &Root| match &r.alias {
    Some(a) => format!("jsr:{} ({})", r.name, a),
    None => format!("jsr:{}", r.name),
  };
  let resolved_of = |r: &Root| r.resolved.clone().unwrap_or("-".into());

  // Column widths use byte length. Package names, versions and requirements
  // are ASCII, so this matches display width here.
  let mut w = [HEADINGS[0].len(), HEADINGS[1].len(), HEADINGS[2].len()];
  for r in roots {
    w[0] = w[0].max(name_of(r).len());
    w[1] = w[1].max(r.required.len());
    w[2] = w[2].max(resolved_of(r).len());
  }

  let pad = |s: &str, n: usize| format!("{}{}", s, " ".repeat(n - s.len()));
  let fills: Vec<String> = w.iter().map(|n| "─".repeat(n + 2)).collect();

  println!("┌{}┬{}┬{}┐", fills[0], fills[1], fills[2]);
  println!(
    "│ {} │ {} │ {} │",
    colors::intense_blue(pad(HEADINGS[0], w[0])),
    colors::intense_blue(pad(HEADINGS[1], w[1])),
    colors::intense_blue(pad(HEADINGS[2], w[2])),
  );
  for r in roots {
    println!("├{}┼{}┼{}┤", fills[0], fills[1], fills[2]);
    println!(
      "│ {} │ {} │ {} │",
      pad(&name_of(r), w[0]),
      pad(&r.required, w[1]),
      pad(&resolved_of(r), w[2]),
    );
  }
  println!("└{}┴{}┴{}┘", fills[0], fills[1], fills[2]);
}

// ---------------------------------------------------------------------------
// Tree output (--depth >= 1)
// ---------------------------------------------------------------------------

#[allow(clippy::print_stdout, reason = "list output")]
fn print_tree(roots: &[Root], graph: &DepGraph, max_depth: u16) {
  fn walk(
    id: &PkgId,
    graph: &DepGraph,
    prefix: &str,
    depth: u16,
    max_depth: u16,
    on_path: &mut Vec<PkgId>,
  ) {
    if depth >= max_depth {
      return;
    }
    let mut children: Vec<&PkgId> = graph.children(id).iter().collect();
    children.sort();
    children.dedup();
    let mut iter = children.iter().peekable();
    while let Some(child) = iter.next() {
      let last = iter.peek().is_none();
      let (branch, cont) = if last {
        ("└─ ", "   ")
      } else {
        ("├─ ", "│  ")
      };
      let (name, version) = split_name_version(child);
      let cyclic = on_path.contains(child);
      let suffix = if cyclic {
        colors::gray(" (cycle)").to_string()
      } else {
        String::new()
      };
      println!(
        "{}{}jsr:{} {}{}",
        prefix,
        branch,
        name,
        colors::gray(version),
        suffix,
      );
      if !cyclic {
        on_path.push((*child).clone());
        walk(
          child,
          graph,
          &format!("{}{}", prefix, cont),
          depth + 1,
          max_depth,
          on_path,
        );
        on_path.pop();
      }
    }
  }

  for root in roots {
    println!("{}", root.label());
    if let Some(resolved) = &root.resolved {
      let name_version = format!("{}@{}", root.name, resolved);
      if let Some(id) = graph.root_id(&name_version) {
        let mut on_path = vec![id.clone()];
        walk(&id, graph, "", 0, max_depth, &mut on_path);
      }
    }
  }
}

// ---------------------------------------------------------------------------

pub async fn list(
  flags: Arc<Flags>,
  list_flags: ListFlags,
) -> Result<(), AnyError> {
  let factory = CliFactory::from_flags(flags.clone());
  let cli_options = factory.cli_options()?;
  let workspace = cli_options.workspace();
  let http_client = factory.http_client_provider();
  let deps_http_cache = factory.global_http_cache()?;
  let file_fetcher = create_cli_file_fetcher(
    factory.blob_store().clone(),
    GlobalOrLocalHttpCache::Global(deps_http_cache.clone()),
    http_client.clone(),
    factory.memory_files().clone(),
    factory.sys(),
    CreateCliFileFetcherOptions {
      allow_remote: true,
      cache_setting: CacheSetting::RespectHeaders,
      download_log_level: log::Level::Trace,
      progress_bar: None,
    },
  );
  let file_fetcher = Arc::new(file_fetcher);
  let jsr_fetch_resolver = Arc::new(JsrFetchResolver::new(
    file_fetcher.clone(),
    factory.jsr_version_resolver()?.clone(),
  ));

  if !cli_options.start_dir.has_ende_json() {
    bail!(
      "No ende.json in \"{}\".",
      cli_options.initial_cwd().display(),
    );
  }

  let args = DepManagerArgs {
    module_load_preparer: factory.module_load_preparer().await?.clone(),
    jsr_fetch_resolver,
    progress_bar: factory.text_only_progress_bar().clone(),
    permissions_container: factory.root_permissions_container()?.clone(),
    main_module_graph_container: factory
      .main_module_graph_container()
      .await?
      .clone(),
    lockfile: factory.maybe_lockfile().await?.cloned(),
  };

  // Name filters (positional args, with wildcard support) are applied while
  // collecting declared deps, so we don't resolve packages we won't display.
  let filter_set = FilterSet::from_filter_strings(
    list_flags.filters.iter().map(|s| s.as_str()),
  )?;
  let filter_fn = |alias: Option<&str>, req: &PackageReq| {
    if filter_set.is_empty() {
      return true;
    }
    filter_set.matches(alias.unwrap_or(&req.name))
  };

  let mut deps = if list_flags.recursive {
    DepManager::from_workspace(workspace, filter_fn, args)?
  } else {
    DepManager::from_workspace_dir(&cli_options.start_dir, filter_fn, args)?
  };

  // Resolve concrete versions (and, when a tree is requested, populate the
  // lockfile graph). Best effort: if resolution fails we still list the
  // declared requirements.
  let resolved_ok = deps.resolve_current_versions().await.is_ok();

  // Build the filtered set of declared root dependencies.
  let mut roots: Vec<Root> = Vec::new();
  for (id, dep) in deps.deps_with_ids() {
    let alias = (dep.alias_or_name() != dep.req.name)
      .then(|| dep.alias_or_name().to_string());
    let resolved = if resolved_ok {
      deps.resolved_version(id).map(|nv| nv.version.to_string())
    } else {
      None
    };
    roots.push(Root {
      alias,
      name: dep.req.name.to_string(),
      required: dep.req.version_req.to_string(),
      resolved,
    });
  }
  roots.sort_by(|a, b| (&a.name, &a.required).cmp(&(&b.name, &b.required)));

  if roots.is_empty() {
    log::info!("No matching dependencies.");
    return Ok(());
  }

  // Tree output requires the resolved dependency graph from the lockfile.
  if list_flags.depth >= 1 {
    let lockfile = factory.maybe_lockfile().await?.cloned();
    let graph = match &lockfile {
      Some(lockfile) => {
        let guard = lockfile.lock();
        DepGraph::from_lockfile_content(&guard.content.packages)
      }
      None => DepGraph::from_lockfile_content(&Default::default()),
    };
    print_tree(&roots, &graph, list_flags.depth);
    return Ok(());
  }

  print_flat_table(&roots);

  Ok(())
}
