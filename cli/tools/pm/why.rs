// Copyright 2018-2026 the Deno authors. MIT license.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use deno_semver::jsr::JsrDepPackageReq;
use deno_semver::package::PackageKind;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;

use crate::args::Flags;
use crate::args::WhyFlags;
use crate::colors;
use crate::factory::CliFactory;

const MAX_PATHS_PER_VERSION: usize = 15;

/// Split `name@version` into `(name, version)`.
fn split_name_version(s: &str) -> Option<(&str, &str)> {
  let at_pos = s[1..].find('@').map(|p| p + 1)?;
  Some((&s[..at_pos], &s[at_pos + 1..]))
}

/// Internal id for a jsr package: its lockfile key, e.g. "@std/async@1.2.0".
/// npm packages are not supported in Ende, so they're not included.
type PkgId = String;

pub async fn why(
  flags: Arc<Flags>,
  why_flags: WhyFlags,
) -> Result<(), AnyError> {
  let factory = CliFactory::from_flags(flags);
  let lockfile = factory.maybe_lockfile().await?.cloned().ok_or_else(|| {
    ende_core::anyhow::anyhow!("No lockfile found. Run `ende install` first.")
  })?;

  // Workspace-declared root dependency requirements (from ende.json `imports`,
  // `scopes`, `jsxImportSource`, etc.). These are matched against the
  // lockfile's `specifiers` map to decide which entries are user-imported
  // roots vs. transitive resolutions that the lockfile also stores under
  // `specifiers` (as JSR does for every dep req).
  let workspace = factory.cli_options()?.workspace();
  let mut root_reqs: HashSet<JsrDepPackageReq> = HashSet::new();
  for ende_json in workspace.ende_jsons() {
    root_reqs.extend(ende_json.dependencies());
  }

  let lockfile_guard = lockfile.lock();
  let content = &lockfile_guard.content;

  // Parse the query: optional "jsr:" prefix and optional version.
  let query = &why_flags.package;
  if query.starts_with("npm:") {
    return Err(super::super::npm_not_supported_error(query));
  }
  let name_and_version = query.strip_prefix("jsr:").unwrap_or(query);
  let (query_name, query_version) =
    if let Some(at_pos) = name_and_version[1..].find('@') {
      let at_pos = at_pos + 1;
      (
        &name_and_version[..at_pos],
        Some(name_and_version[at_pos + 1..].to_string()),
      )
    } else {
      (name_and_version, None)
    };

  // Collect all jsr packages with their deps. We also build a forward dep
  // map so that we can derive the reverse map below.
  let mut all_packages: Vec<PkgId> = Vec::new();
  let mut forward_deps: HashMap<PkgId, Vec<PkgId>> = HashMap::new();

  for (nv, info) in content.packages.jsr.iter() {
    let id: PkgId = nv.to_string();
    all_packages.push(id.clone());
    let deps: Vec<PkgId> = info
      .dependencies
      .iter()
      .filter(|dep_req| dep_req.kind == PackageKind::Jsr)
      .filter_map(|dep_req| {
        let resolved = content.packages.specifiers.get(dep_req)?;
        Some(format!("{}@{}", dep_req.req.name, resolved))
      })
      .collect();
    forward_deps.insert(id, deps);
  }

  // Find matching packages (filtered by name and optional version).
  let matching: Vec<&PkgId> = all_packages
    .iter()
    .filter(|id| {
      let Some((name, version)) = split_name_version(id) else {
        return false;
      };
      name == query_name
        && query_version.as_deref().is_none_or(|v| version == v)
    })
    .collect();

  if matching.is_empty() {
    bail!("package '{}' not found in the dependency tree", query);
  }

  // Build reverse-dep map.
  let mut reverse_deps: BTreeMap<PkgId, Vec<PkgId>> = BTreeMap::new();
  for (parent, deps) in &forward_deps {
    for dep in deps {
      reverse_deps
        .entry(dep.clone())
        .or_default()
        .push(parent.clone());
    }
  }
  for parents in reverse_deps.values_mut() {
    parents.sort();
    parents.dedup();
  }

  // A lockfile specifier is a root iff it matches a req the user declared in
  // an ende.json. JSR transitive deps also appear in `specifiers` (the
  // lockfile normalizes JSR deps through the specifiers map), so we can't
  // treat every entry as a root.
  let mut root_specifier_to_pkg: HashMap<PkgId, Vec<String>> = HashMap::new();
  for (req, resolved) in content.packages.specifiers.iter() {
    if req.kind != PackageKind::Jsr || !root_reqs.contains(req) {
      continue;
    }
    let name = req.req.name.as_str();
    let pkg_key = format!("{}@{}", name, resolved);
    root_specifier_to_pkg
      .entry(pkg_key)
      .or_default()
      .push(req.to_string());
  }

  for id in &matching {
    log::info!("{}", colors::bold(id));

    if let Some(specifiers) = root_specifier_to_pkg.get(*id) {
      for spec in specifiers {
        log::info!("  {}", colors::green(spec));
      }
    }

    let mut paths = find_paths_to_root(
      id,
      &reverse_deps,
      &root_specifier_to_pkg,
      MAX_PATHS_PER_VERSION,
    );

    paths.sort_by_key(|p| p.len());

    if paths.is_empty() && !root_specifier_to_pkg.contains_key(*id) {
      log::info!(
        "  (no dependency path found -- try running `ende install` to refresh the lockfile)"
      );
    }

    if paths.len() > MAX_PATHS_PER_VERSION {
      for path in &paths[..MAX_PATHS_PER_VERSION] {
        log::info!("{}", format_dependency_path(path, &root_specifier_to_pkg));
      }
      log::info!(
        "  ... and {} more paths",
        paths.len() - MAX_PATHS_PER_VERSION
      );
    } else {
      for path in &paths {
        log::info!("{}", format_dependency_path(path, &root_specifier_to_pkg));
      }
    }

    log::info!("");
  }

  Ok(())
}

/// Find dependency paths from root packages to the target.
///
/// `max_paths` caps how many paths are collected; once reached the
/// search short-circuits to avoid exponential expansion on wide
/// diamond dependency graphs (e.g. `ms` reachable via 30+ routes
/// in a Next.js app).
fn find_paths_to_root<'a>(
  target_id: &'a PkgId,
  reverse_deps: &'a BTreeMap<PkgId, Vec<PkgId>>,
  root_specifiers: &HashMap<PkgId, Vec<String>>,
  max_paths: usize,
) -> Vec<Vec<&'a PkgId>> {
  let mut paths: Vec<Vec<&'a PkgId>> = Vec::new();
  let mut current_path: Vec<&'a PkgId> = vec![target_id];

  fn dfs<'a>(
    current_id: &'a PkgId,
    target_id: &'a PkgId,
    current_path: &mut Vec<&'a PkgId>,
    paths: &mut Vec<Vec<&'a PkgId>>,
    reverse_deps: &'a BTreeMap<PkgId, Vec<PkgId>>,
    root_specifiers: &HashMap<PkgId, Vec<String>>,
    max_paths: usize,
  ) {
    if paths.len() >= max_paths {
      return;
    }

    // Check if we reached a root (but not the starting node itself,
    // since direct specifiers are printed separately by the caller)
    if current_id != target_id && root_specifiers.contains_key(current_id) {
      paths.push(current_path.iter().rev().copied().collect());
      return;
    }

    if let Some(parents) = reverse_deps.get(current_id) {
      for parent in parents {
        if paths.len() >= max_paths {
          return;
        }
        // Path-based cycle check: only skip nodes already on the
        // current path. This prevents infinite loops while still
        // allowing a node to appear in multiple independent paths.
        if current_path.contains(&parent) {
          continue;
        }
        current_path.push(parent);
        dfs(
          parent,
          target_id,
          current_path,
          paths,
          reverse_deps,
          root_specifiers,
          max_paths,
        );
        current_path.pop();
      }
    }
  }

  dfs(
    target_id,
    target_id,
    &mut current_path,
    &mut paths,
    reverse_deps,
    root_specifiers,
    max_paths,
  );

  paths
}

/// Format a dependency path as a string.
fn format_dependency_path(
  path: &[&PkgId],
  root_specifiers: &HashMap<PkgId, Vec<String>>,
) -> String {
  if path.is_empty() {
    return String::new();
  }

  let root = path[0];

  let mut out = String::new();

  if let Some(specs) = root_specifiers.get(root) {
    out.push_str(&format!("  {}", colors::green(&specs[0])));
  } else {
    out.push_str(&format!("  {}", root));
  }

  for id in &path[1..] {
    out.push_str(&format!(" > {}", id));
  }

  out
}
