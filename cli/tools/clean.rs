// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno_graph::ModuleGraph;
use deno_graph::packages::PackageSpecifiers;
use ende_cache_dir::GlobalOrLocalHttpCache;
use ende_core::error::AnyError;
use ende_core::url::Url;
use sys_traits::FsCanonicalize;
use sys_traits::FsCreateDirAll;
use walkdir::WalkDir;

use crate::args::CleanFlags;
use crate::args::Flags;
use crate::colors;
use crate::display;
use crate::factory::CliFactory;
use crate::graph_container::CollectSpecifiersOptions;
use crate::graph_container::ModuleGraphContainer;
use crate::graph_container::ModuleGraphUpdatePermit;
use crate::graph_util::BuildGraphOptions;
use crate::util::fs::FsCleaner;
use crate::util::progress_bar::ProgressBar;
use crate::util::progress_bar::ProgressBarStyle;
use crate::util::progress_bar::ProgressMessagePrompt;

pub async fn clean(
  flags: Arc<Flags>,
  clean_flags: CleanFlags,
) -> Result<(), AnyError> {
  if !clean_flags.except_paths.is_empty() {
    return clean_except(flags, &clean_flags.except_paths, clean_flags.dry_run)
      .await;
  }

  let factory = CliFactory::from_flags(flags);
  let ende_dir = factory.ende_dir()?;
  if ende_dir.root.exists() {
    let no_of_files = walkdir::WalkDir::new(&ende_dir.root).into_iter().count();
    let progress_bar = ProgressBar::new(ProgressBarStyle::ProgressBars);
    let progress_guard =
      progress_bar.update_with_prompt(ProgressMessagePrompt::Cleaning, "");
    progress_guard.set_total_size(no_of_files.try_into().unwrap());
    let mut cleaner = FsCleaner::new(Some(progress_guard));

    // On a dry run, tally up what would be removed without deleting anything;
    // the reported output is otherwise identical to a real clean.
    if clean_flags.dry_run {
      cleaner.tally(&ende_dir.root)?;
    } else {
      cleaner.rm_rf(&ende_dir.root)?;
    }

    // Drop the guard so that progress bar disappears.
    drop(cleaner.progress_guard.take());

    log::info!(
      "{} {} {}",
      colors::green("Removed"),
      ende_dir.root.display(),
      colors::gray(&format!(
        "({} files, {})",
        cleaner.files_removed + cleaner.dirs_removed,
        display::human_size(cleaner.bytes_removed as f64)
      ))
    );

    if clean_flags.dry_run {
      log::info!("{}", colors::yellow("Aborting due to --dry-run flag"));
    } else {
      report_locked_files(&cleaner.failed);
    }
  }

  Ok(())
}

/// Warns about cache files that could not be removed because another process is
/// holding them open. On Windows this resolves the offending process(es) via
/// the Restart Manager and points the user at a running Ende language server,
/// which is the most common cause (see issue #33903).
fn report_locked_files(failed: &[(PathBuf, std::io::Error)]) {
  if failed.is_empty() {
    return;
  }
  let paths = failed
    .iter()
    .map(|(path, _)| path.clone())
    .collect::<Vec<_>>();
  let processes = crate::util::windows::processes_locking_files(&paths);
  log::warn!("{}", format_locked_files_message(&paths, &processes));
}

fn format_locked_files_message(
  paths: &[PathBuf],
  processes: &[crate::util::windows::LockingProcess],
) -> String {
  const MAX_LISTED: usize = 10;
  let mut message = format!(
    "{} could not be removed because they are in use by another process:",
    if paths.len() == 1 {
      "1 file".to_string()
    } else {
      format!("{} files", paths.len())
    }
  );
  for path in paths.iter().take(MAX_LISTED) {
    message.push_str(&format!("\n  {}", path.display()));
  }
  if paths.len() > MAX_LISTED {
    message.push_str(&format!("\n  ... and {} more", paths.len() - MAX_LISTED));
  }

  if processes.is_empty() {
    message.push_str(
      "\n\nThis is usually an editor running the Ende language server holding \
       the cache open. Close it (or any running `ende` process) and run `ende \
       clean` again.",
    );
  } else {
    message.push_str("\n\nHeld open by:");
    for process in processes {
      let label = match &process.exe {
        Some(exe) => exe.display().to_string(),
        None => process.name.clone(),
      };
      let hint = if process.is_ende {
        " (a running Ende process, likely the language server in your editor)"
      } else {
        ""
      };
      message.push_str(&format!("\n  {} (PID {}){}", label, process.pid, hint));
    }
    message
      .push_str("\n\nClose the process(es) above and run `ende clean` again.");
  }

  message
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Found {
  Match,
  Prefix,
}

#[derive(Clone, Debug, Default)]
struct PathNode {
  exact: bool,
  children: BTreeMap<OsString, usize>,
}

#[derive(Debug)]
struct PathTrie {
  root: usize,
  nodes: Vec<PathNode>,
  rewrites: Vec<(PathBuf, PathBuf)>,
}

impl PathTrie {
  fn new() -> Self {
    Self {
      root: 0,
      nodes: vec![PathNode {
        exact: false,
        children: Default::default(),
      }],
      rewrites: vec![],
    }
  }

  fn add_rewrite(&mut self, from: PathBuf, to: PathBuf) {
    self.rewrites.push((from, to));
  }

  fn rewrite<'a>(&self, s: Cow<'a, Path>) -> Cow<'a, Path> {
    let normalized = deno_path_util::normalize_path(s);
    for (from, to) in &self.rewrites {
      if normalized.starts_with(from) {
        return Cow::Owned(to.join(normalized.strip_prefix(from).unwrap()));
      }
    }
    normalized
  }

  fn insert(&mut self, s: PathBuf) {
    let normalized = self.rewrite(Cow::Owned(s));
    let components = normalized.components().map(|c| c.as_os_str());
    let mut node = self.root;

    for component in components {
      if let Some(nd) = self.nodes[node].children.get(component).copied() {
        node = nd;
      } else {
        let id = self.nodes.len();
        self.nodes.push(PathNode::default());
        self.nodes[node]
          .children
          .insert(component.to_os_string(), id);
        node = id;
      }
    }

    self.nodes[node].exact = true;
  }

  fn find(&self, s: &Path) -> Option<Found> {
    let normalized = self.rewrite(Cow::Borrowed(s));
    let components = normalized.components().map(|c| c.as_os_str());
    let mut node = self.root;

    for component in components {
      if let Some(nd) = self.nodes[node].children.get(component).copied() {
        node = nd;
      } else {
        return None;
      }
    }

    Some(if self.nodes[node].exact {
      Found::Match
    } else {
      Found::Prefix
    })
  }
}

fn try_get_canonicalized_root_dir<Sys: FsCanonicalize + FsCreateDirAll>(
  sys: &Sys,
  root_dir: &Path,
) -> Result<PathBuf, std::io::Error> {
  match sys.fs_canonicalize(root_dir) {
    Ok(path) => Ok(path),
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
      sys.fs_create_dir_all(root_dir)?;
      sys.fs_canonicalize(root_dir)
    }
    Err(err) => Err(err),
  }
}

async fn clean_except(
  flags: Arc<Flags>,
  entrypoints: &[String],
  dry_run: bool,
) -> Result<(), AnyError> {
  let mut state = FsCleaner::default();

  let factory = CliFactory::from_flags(flags.clone());
  let sys = factory.sys();
  let options = factory.cli_options()?;
  let main_graph_container = factory.main_module_graph_container().await?;
  let roots = main_graph_container.collect_specifiers(
    entrypoints,
    CollectSpecifiersOptions {
      include_ignored_specified: true,
    },
  )?;
  let http_cache = factory.global_http_cache()?;
  let local_or_global_http_cache = factory.http_cache()?.clone();
  let ende_dir = factory.ende_dir()?.clone();
  let ende_dir_root_canonical =
    try_get_canonicalized_root_dir(&sys, &ende_dir.root)
      .unwrap_or(ende_dir.root.clone());

  let mut permit = main_graph_container.acquire_update_permit().await;
  let graph = permit.graph_mut();
  graph.packages = PackageSpecifiers::default();
  let graph_builder = factory.module_graph_builder().await?;
  graph_builder
    .build_graph_roots(
      graph,
      roots.clone(),
      BuildGraphOptions {
        loader: None,
        is_dynamic: false,
      },
    )
    .await?;

  let mut keep = HashSet::new();

  let mut keep_paths_trie = PathTrie::new();
  if ende_dir_root_canonical != ende_dir.root {
    keep_paths_trie
      .add_rewrite(ende_dir.root.clone(), ende_dir_root_canonical.clone());
  }
  for (_, entry) in graph.walk(
    roots.iter(),
    deno_graph::WalkOptions {
      check_js: deno_graph::CheckJsOption::False,
      follow_dynamic: true,
      kind: graph.graph_kind(),
      prefer_fast_check_graph: false,
    },
  ) {
    match entry {
      deno_graph::ModuleEntryRef::Module(module) => match module {
        deno_graph::Module::Js(js_module) => {
          keep.insert(&js_module.specifier);
        }
        deno_graph::Module::Json(json_module) => {
          keep.insert(&json_module.specifier);
        }
        deno_graph::Module::Wasm(wasm_module) => {
          keep.insert(&wasm_module.specifier);
        }
        deno_graph::Module::Npm(_)
        | deno_graph::Module::Node(_)
        | deno_graph::Module::External(_) => {}
      },
      deno_graph::ModuleEntryRef::Err(_) => {}
      deno_graph::ModuleEntryRef::Redirect(_) => {}
    }
  }

  for url in &keep {
    if (url.scheme() == "http" || url.scheme() == "https")
      && let Ok(path) = http_cache.local_path_for_url(url)
    {
      keep_paths_trie.insert(path);
    }
    if let Some(path) = ende_dir
      .gen_cache
      .get_cache_filename_with_extension(url, "js")
    {
      let path = ende_dir.gen_cache.location.join(path);
      keep_paths_trie.insert(path);
    }
  }

  if dry_run {
    #[allow(clippy::print_stderr, reason = "actually want to output")]
    {
      eprintln!("would remove:");
    }
  }

  let jsr_url = crate::args::jsr_url();
  add_jsr_meta_paths(graph, &mut keep_paths_trie, jsr_url, &|url| {
    http_cache
      .local_path_for_url(url)
      .map_err(Into::into)
      .map(Some)
  })?;
  walk_removing(
    &mut state,
    walkdir::WalkDir::new(&ende_dir.root)
      .contents_first(false)
      .min_depth(2),
    &keep_paths_trie,
    &ende_dir.root,
    dry_run,
  )?;
  let mut vendor_cleaned = FsCleaner::default();
  if let Some(vendor_dir) = options.vendor_dir_path()
    && let GlobalOrLocalHttpCache::Local(cache) = local_or_global_http_cache
  {
    let mut trie = PathTrie::new();
    if ende_dir_root_canonical != ende_dir.root {
      trie.add_rewrite(ende_dir.root.clone(), ende_dir_root_canonical);
    }
    let cache = cache.clone();
    add_jsr_meta_paths(graph, &mut trie, jsr_url, &|url| match cache
      .local_path_for_url(url)
    {
      Ok(path) => Ok(path),
      Err(err) => {
        log::warn!(
          "failed to get local path for jsr meta url {}: {}",
          url,
          err
        );
        Ok(None)
      }
    })?;
    for url in keep {
      if url.scheme() == "http" || url.scheme() == "https" {
        match cache.local_path_for_url(url) {
          Ok(Some(path)) => {
            trie.insert(path);
          }
          Ok(None) => {}
          Err(err) => {
            log::warn!("failed to get local path for url {}: {}", url, err);
          }
        }
      }
    }

    walk_removing(
      &mut vendor_cleaned,
      WalkDir::new(vendor_dir).contents_first(false),
      &trie,
      vendor_dir,
      dry_run,
    )?;
  }

  if !dry_run {
    log_stats(&state, &ende_dir.root);

    if let Some(dir) = options.vendor_dir_path() {
      log_stats(&vendor_cleaned, dir);
    }

    let mut failed = state.failed;
    failed.extend(vendor_cleaned.failed);
    report_locked_files(&failed);
  }

  Ok(())
}

fn log_stats(cleaner: &FsCleaner, dir: &Path) {
  if cleaner.bytes_removed == 0
    && cleaner.dirs_removed == 0
    && cleaner.files_removed == 0
  {
    return;
  }
  log::info!(
    "{} {}",
    colors::green("Removed"),
    colors::gray(&format!(
      "{} files, {} from {}",
      cleaner.files_removed + cleaner.dirs_removed,
      display::human_size(cleaner.bytes_removed as f64),
      dir.display()
    ))
  );
}

fn add_jsr_meta_paths(
  graph: &ModuleGraph,
  path_trie: &mut PathTrie,
  jsr_url: &Url,
  url_to_path: &dyn Fn(&Url) -> Result<Option<PathBuf>, AnyError>,
) -> Result<(), AnyError> {
  for package in graph.packages.mappings().values() {
    let Ok(base_url) = jsr_url.join(&format!("{}/", &package.name)) else {
      continue;
    };
    let keep = url_to_path(&base_url.join("meta.json").unwrap())?;
    if let Some(keep) = keep {
      path_trie.insert(keep);
    }
    let keep = url_to_path(
      &base_url
        .join(&format!("{}_meta.json", package.version))
        .unwrap(),
    )?;
    if let Some(keep) = keep {
      path_trie.insert(keep);
    }
  }
  Ok(())
}

// TODO(nathanwhit): use strategy pattern instead of branching on dry_run
fn walk_removing(
  cleaner: &mut FsCleaner,
  walker: WalkDir,
  trie: &PathTrie,
  base: &Path,
  dry_run: bool,
) -> Result<(), AnyError> {
  let mut walker = walker.into_iter();
  while let Some(entry) = walker.next() {
    let entry = entry?;
    if let Some(found) = trie.find(entry.path()) {
      if entry.file_type().is_dir() && matches!(found, Found::Match) {
        walker.skip_current_dir();
        continue;
      }
      continue;
    }
    if !entry.path().starts_with(base) {
      panic!(
        "would have removed a file outside of the base directory: base: {}, path: {}",
        base.display(),
        entry.path().display()
      );
    }
    if entry.file_type().is_dir() {
      if dry_run {
        #[allow(clippy::print_stderr, reason = "actually want to output")]
        {
          eprintln!(" {}", entry.path().display());
        }
      } else {
        cleaner.rm_rf(entry.path())?;
      }
      walker.skip_current_dir();
    } else if dry_run {
      #[allow(clippy::print_stderr, reason = "actually want to output")]
      {
        eprintln!(" {}", entry.path().display());
      }
    } else {
      cleaner.remove_file(entry.path(), Some(entry.metadata()?))?;
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use std::path::Path;
  use std::path::PathBuf;

  use super::Found::*;
  use super::format_locked_files_message;
  use crate::util::windows::LockingProcess;

  #[test]
  fn locked_files_message_without_processes() {
    let paths = vec![PathBuf::from("/deno/dep_analysis_cache_v2")];
    let message = format_locked_files_message(&paths, &[]);
    assert!(message.starts_with(
      "1 file could not be removed because they are in use by another process:"
    ));
    assert!(message.contains("dep_analysis_cache_v2"));
    assert!(message.contains("Ende language server"));
  }

  #[test]
  fn locked_files_message_lists_holding_processes() {
    let paths = vec![
      PathBuf::from("/deno/dep_analysis_cache_v2"),
      PathBuf::from("/deno/check_cache_v2"),
    ];
    let processes = vec![
      LockingProcess {
        pid: 1234,
        name: "deno".to_string(),
        exe: Some(PathBuf::from("C:/Program Files/deno/deno.exe")),
        is_ende: true,
      },
      LockingProcess {
        pid: 5678,
        name: "Some Editor".to_string(),
        exe: None,
        is_ende: false,
      },
    ];
    let message = format_locked_files_message(&paths, &processes);
    assert!(message.starts_with("2 files could not be removed"));
    assert!(message.contains("Held open by:"));
    assert!(message.contains("deno.exe (PID 1234)"));
    assert!(message.contains("likely the language server"));
    assert!(message.contains("Some Editor (PID 5678)"));
    assert!(message.contains("Close the process(es) above"));
  }

  #[test]
  fn locked_files_message_truncates_long_lists() {
    let paths = (0..15)
      .map(|i| PathBuf::from(format!("/deno/file_{i}")))
      .collect::<Vec<_>>();
    let message = format_locked_files_message(&paths, &[]);
    assert!(message.starts_with("15 files could not be removed"));
    assert!(message.contains("... and 5 more"));
  }

  #[test]
  fn path_trie() {
    let mut trie = super::PathTrie::new();

    #[cfg(unix)]
    {
      trie.add_rewrite(
        Path::new("/RewriteMe").into(),
        Path::new("/Actual").into(),
      );
    }
    #[cfg(windows)]
    {
      trie.add_rewrite(
        Path::new("C:/RewriteMe").into(),
        Path::new("C:/Actual").into(),
      );
    }

    let paths = {
      #[cfg(unix)]
      {
        [
          "/foo/bar/deno",
          "/foo/bar/deno/1",
          "/foo/bar/deno/2",
          "/foo/baz",
          "/Actual/thing/quux",
        ]
      }
      #[cfg(windows)]
      {
        [
          r"C:\foo\bar\deno",
          r"C:\foo\bar\deno\1",
          r"C:\foo\bar\deno\2",
          r"C:\foo\baz",
          r"D:\thing",
          r"C:\Actual\thing\quux",
        ]
      }
    };

    let cases = {
      #[cfg(unix)]
      {
        [
          ("/", Some(Prefix)),
          ("/foo", Some(Prefix)),
          ("/foo/", Some(Prefix)),
          ("/foo/bar", Some(Prefix)),
          ("/foo/bar/deno", Some(Match)),
          ("/foo/bar/deno/1", Some(Match)),
          ("/foo/bar/deno/2", Some(Match)),
          ("/foo/baz", Some(Match)),
          ("/fo", None),
          ("/foo/baz/deno", None),
          ("/Actual/thing/quux", Some(Match)),
          ("/RewriteMe/thing/quux", Some(Match)),
          ("/RewriteMe/thing", Some(Prefix)),
        ]
      }
      #[cfg(windows)]
      {
        [
          (r"C:\", Some(Prefix)),
          (r"C:\foo", Some(Prefix)),
          (r"C:\foo\", Some(Prefix)),
          (r"C:\foo\", Some(Prefix)),
          (r"C:\foo\bar", Some(Prefix)),
          (r"C:\foo\bar\deno\1", Some(Match)),
          (r"C:\foo\bar\deno\2", Some(Match)),
          (r"C:\foo\baz", Some(Match)),
          (r"C:\fo", None),
          (r"C:\foo\baz\deno", None),
          (r"D:\", Some(Prefix)),
          (r"E:\", None),
          (r"C:\Actual\thing\quux", Some(Match)),
          (r"C:\RewriteMe\thing\quux", Some(Match)),
          (r"C:\RewriteMe\thing", Some(Prefix)),
        ]
      }
    };

    for pth in paths {
      let path = Path::new(pth);
      trie.insert(path.into());
    }

    for (input, expect) in cases {
      let path = Path::new(input);
      assert_eq!(trie.find(path), expect, "on input: {input}");
    }
  }
}
