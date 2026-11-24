// Copyright 2018-2026 the Deno authors. MIT license.

//! Property test for [`FsRoot`]: random trees of directories, files and
//! symlinks (some pointing out of the root, some in loops), random names and
//! random operations. Whatever a name resolves to, no operation may read,
//! change, create or remove anything outside the root.
//!
//! The case count follows proptest's defaults, so `PROPTEST_CASES=20000`
//! runs a longer search locally.

#![allow(
  clippy::disallowed_methods,
  reason = "fixtures and assertions use the ambient file system on purpose"
)]
#![allow(
  clippy::disallowed_types,
  reason = "proptest's `prop_oneof!` expands to `std::sync::Arc`"
)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

use proptest::prelude::*;

use crate::OpenOptions;
use crate::root::FsRoot;
use crate::root::FsRootError;
use crate::root::split_parent;

const MARKER: &str = "MARKER-OUTSIDE-THE-ROOT";

/// Names used for entries in the tree and for components of the names the
/// operations take.
const NAMES: &[&str] = &["a", "b", "c", "l1", "l2", "l3"];

#[derive(Debug, Clone)]
enum Kind {
  Dir,
  File,
  /// A symlink whose target is written relative to the link.
  Link(&'static str),
  /// A symlink whose target is an absolute path to `<base>/outside`
  /// (optionally a name under it).
  AbsLink(&'static str),
}

/// Relative symlink targets: inside, out of the root, loops, and dangling.
const LINK_TARGETS: &[&str] = &[
  ".",
  "..",
  "../..",
  "../outside",
  "../outside/marker.txt",
  "../../outside/marker.txt",
  "a",
  "a/b",
  "b/../..",
  "l1",
  "l2",
  "l3",
  "missing",
];

/// Names under `<base>/outside` for absolute symlink targets.
const ABS_TARGETS: &[&str] = &["", "marker.txt", "sub"];

#[derive(Debug, Clone)]
struct Node {
  /// Directory under the root the entry goes in ("" for the root itself).
  dir: &'static str,
  name: &'static str,
  kind: Kind,
}

#[derive(Debug, Clone)]
enum Op {
  OpenRead,
  Stat,
  Lstat,
  /// The parent route of the ops: open the parent as a root, then create
  /// and write the leaf there without following it.
  WriteParentRoute,
  /// `FsRoot::open` with create + write (follows a final symlink while it
  /// stays inside).
  WriteFollow,
  Mkdir,
  Remove,
  RemoveRecursive,
  RenameFrom,
  RenameTo,
  ReadDir,
  OpenRoot,
}

fn kind_strategy() -> impl Strategy<Value = Kind> {
  // Weighted toward symlinks: they are what a name can escape through.
  prop_oneof![
    2 => Just(Kind::Dir),
    1 => Just(Kind::File),
    3 => proptest::sample::select(LINK_TARGETS).prop_map(Kind::Link),
    2 => proptest::sample::select(ABS_TARGETS).prop_map(Kind::AbsLink),
  ]
}

fn node_strategy() -> impl Strategy<Value = Node> {
  (
    // Mostly at the top of the root, where names start.
    prop_oneof![
      3 => Just(""),
      1 => proptest::sample::select(&["a", "a/b", "b"][..]),
    ],
    proptest::sample::select(NAMES),
    kind_strategy(),
  )
    .prop_map(|(dir, name, kind)| Node { dir, name, kind })
}

/// A name for an operation: 1–4 components, sometimes absolute. The first
/// component is usually an entry the tree may hold, so names walk through
/// its symlinks.
fn name_strategy() -> impl Strategy<Value = (Option<bool>, Vec<&'static str>)> {
  let first = prop_oneof![
    3 => proptest::sample::select(NAMES),
    1 => proptest::sample::select(&[".", "..", "marker.txt", "new"][..]),
  ];
  let component = proptest::sample::select(
    &[
      "a",
      "b",
      "c",
      "l1",
      "l2",
      "l3",
      ".",
      "..",
      "marker.txt",
      "outside",
      "new",
    ][..],
  );
  (
    // None: relative; Some(false): "/"-prefixed; Some(true): prefixed with
    // the absolute path of `<base>/outside`.
    prop_oneof![
      6 => Just(None),
      1 => Just(Some(false)),
      1 => Just(Some(true)),
    ],
    (first, proptest::collection::vec(component, 0..=3)),
  )
    .prop_map(|(abs, (first, mut rest))| {
      rest.insert(0, first);
      (abs, rest)
    })
}

fn op_strategy() -> impl Strategy<Value = Op> {
  prop_oneof![
    Just(Op::OpenRead),
    Just(Op::Stat),
    Just(Op::Lstat),
    Just(Op::WriteParentRoute),
    Just(Op::WriteFollow),
    Just(Op::Mkdir),
    Just(Op::Remove),
    Just(Op::RemoveRecursive),
    Just(Op::RenameFrom),
    Just(Op::RenameTo),
    Just(Op::ReadDir),
    Just(Op::OpenRoot),
  ]
}

/// Everything under `path`, as relative name -> what it is, so a before and
/// after snapshot shows any change outside the root.
fn snapshot(path: &Path) -> BTreeMap<PathBuf, String> {
  fn walk(base: &Path, path: &Path, out: &mut BTreeMap<PathBuf, String>) {
    let Ok(meta) = fs::symlink_metadata(path) else {
      return;
    };
    let rel = path.strip_prefix(base).unwrap().to_path_buf();
    let ty = meta.file_type();
    let value = if ty.is_symlink() {
      format!("link {}", fs::read_link(path).unwrap().display())
    } else if ty.is_dir() {
      let mut names: Vec<_> = fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
      names.sort();
      for child in &names {
        walk(base, child, out);
      }
      format!("dir {:o}", meta.mode() & 0o7777)
    } else {
      format!(
        "file {:o} {} {:?}",
        meta.mode() & 0o7777,
        meta.ino(),
        fs::read(path).unwrap()
      )
    };
    out.insert(rel, value);
  }
  let mut out = BTreeMap::new();
  walk(path, path, &mut out);
  out
}

struct World {
  _tmp: tempfile::TempDir,
  base: PathBuf,
  root_path: PathBuf,
  outside: PathBuf,
}

fn build(nodes: &[Node]) -> World {
  let tmp = tempfile::TempDir::new().unwrap();
  // Canonical, so real paths compare directly (macOS: /var -> /private/var).
  let base = fs::canonicalize(tmp.path()).unwrap();
  let root_path = base.join("root");
  let outside = base.join("outside");
  fs::create_dir(&root_path).unwrap();
  fs::create_dir(&outside).unwrap();
  fs::create_dir(outside.join("sub")).unwrap();
  fs::write(outside.join("marker.txt"), MARKER).unwrap();
  for node in nodes {
    let dir = root_path.join(node.dir);
    // Entries whose directory doesn't exist (or isn't a directory) are
    // skipped: the tree is whatever the list manages to build.
    if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
      continue;
    }
    let path = dir.join(node.name);
    if fs::symlink_metadata(&path).is_ok() {
      continue;
    }
    let _ = match &node.kind {
      Kind::Dir => fs::create_dir(&path),
      Kind::File => fs::write(&path, format!("inside {}", node.name)),
      Kind::Link(target) => std::os::unix::fs::symlink(target, &path),
      Kind::AbsLink(target) => {
        std::os::unix::fs::symlink(outside.join(target), &path)
      }
    };
  }
  World {
    _tmp: tmp,
    base,
    root_path,
    outside,
  }
}

fn make_name(
  world: &World,
  (abs, parts): &(Option<bool>, Vec<&str>),
) -> PathBuf {
  let mut name = match abs {
    None => PathBuf::new(),
    Some(false) => PathBuf::from("/"),
    Some(true) => world.outside.clone(),
  };
  for part in parts {
    name.push(part);
  }
  name
}

/// Whether the name must escape whatever the tree holds: absolute, or a
/// `..` reached before any real component (`..`, `./../x`). A climb after a
/// real component depends on the tree: `a/../..` is NotFound while `a`
/// doesn't exist (as with Go's `os.Root` on Unix), and `l/../..` stays inside
/// when `l` links two levels down.
fn always_escapes(name: &Path) -> bool {
  if name.is_absolute() {
    return true;
  }
  for c in name.components() {
    match c {
      std::path::Component::CurDir => {}
      std::path::Component::ParentDir => return true,
      _ => return false,
    }
  }
  false
}

fn write_options() -> OpenOptions {
  OpenOptions {
    write: true,
    create: true,
    truncate: true,
    ..Default::default()
  }
}

/// Runs `op` on `name`. Returns the error if it failed, and on success
/// checks what can be checked about the thing it reached.
fn run(
  world: &World,
  root: &FsRoot,
  op: &Op,
  name: &Path,
) -> Result<(), FsRootError> {
  let root_real = root.real_path().unwrap();
  let marker_meta = fs::metadata(world.outside.join("marker.txt")).unwrap();
  let outside_meta = fs::metadata(&world.outside).unwrap();
  let base_meta = fs::metadata(&world.base).unwrap();
  let is_outside = |dev: u64, ino: Option<u64>| {
    [&marker_meta, &outside_meta, &base_meta]
      .iter()
      .any(|m| m.dev() == dev && Some(m.ino()) == ino)
  };
  match op {
    Op::OpenRead => {
      let file = root.open(
        name,
        &OpenOptions {
          read: true,
          ..Default::default()
        },
      )?;
      let real = crate::root::real_path_of_file(&file).unwrap();
      assert!(
        real.starts_with(&root_real),
        "{name:?} opened {real:?}, outside {root_real:?}"
      );
      let meta = file.metadata().unwrap();
      assert!(!is_outside(meta.dev(), Some(meta.ino())));
    }
    Op::Stat => {
      let stat = root.stat(name)?;
      assert!(!is_outside(stat.dev, stat.ino), "stat {name:?} reached out");
    }
    Op::Lstat => {
      let stat = root.lstat(name)?;
      assert!(
        !is_outside(stat.dev, stat.ino),
        "lstat {name:?} reached out"
      );
    }
    Op::WriteParentRoute => {
      let (parent, leaf) = split_parent(name)?;
      let parent = root.open_root(&parent)?;
      let real = parent.real_path().unwrap();
      assert!(
        real.starts_with(&root_real),
        "parent of {name:?} is {real:?}"
      );
      let mut options = write_options();
      options.read = false;
      let mut file = parent.open_leaf(&leaf, &options)?;
      std::io::Write::write_all(&mut file, b"written").unwrap();
    }
    Op::WriteFollow => {
      let mut file = root.open(name, &write_options())?;
      let real = crate::root::real_path_of_file(&file).unwrap();
      assert!(real.starts_with(&root_real), "{name:?} wrote {real:?}");
      std::io::Write::write_all(&mut file, b"written").unwrap();
    }
    Op::Mkdir => root.mkdir(name, true, None)?,
    Op::Remove => root.remove(name, false)?,
    Op::RemoveRecursive => root.remove(name, true)?,
    Op::RenameFrom => root.rename(name, Path::new("moved"))?,
    Op::RenameTo => root.rename(Path::new("a"), name)?,
    Op::ReadDir => {
      root.read_dir(name)?;
    }
    Op::OpenRoot => {
      let sub = root.open_root(name)?;
      let real = sub.real_path().unwrap();
      assert!(
        real.starts_with(&root_real),
        "sub-root {name:?} is {real:?}"
      );
    }
  }
  Ok(())
}

proptest! {
  #![proptest_config(ProptestConfig {
    // 1024 cases take well under a second; `PROPTEST_CASES` overrides it.
    cases: std::env::var("PROPTEST_CASES")
      .ok()
      .and_then(|cases| cases.parse().ok())
      .unwrap_or(1024),
    // Don't write regression files into the source tree; a failure prints
    // the minimised case instead.
    failure_persistence: None,
    ..ProptestConfig::default()
  })]

  #[test]
  fn no_operation_reaches_outside_the_root(
    nodes in proptest::collection::vec(node_strategy(), 0..10),
    name in name_strategy(),
    op in op_strategy(),
  ) {
    let world = build(&nodes);
    let name = make_name(&world, &name);
    let outside_before = snapshot(&world.outside);
    let base_entries_before: Vec<_> = fs::read_dir(&world.base)
      .unwrap()
      .map(|e| e.unwrap().file_name())
      .collect();

    let root = FsRoot::open_ambient(&world.root_path).unwrap();
    let result = run(&world, &root, &op, &name);

    if always_escapes(&name) {
      prop_assert!(
        matches!(result, Err(FsRootError::Escape(_))),
        "{op:?} {name:?} must escape, got {result:?}"
      );
    }
    prop_assert_eq!(
      snapshot(&world.outside),
      outside_before,
      "{:?} {:?} changed something outside the root",
      op,
      name
    );
    let mut base_entries_after: Vec<_> = fs::read_dir(&world.base)
      .unwrap()
      .map(|e| e.unwrap().file_name())
      .collect();
    let mut base_entries_before = base_entries_before;
    base_entries_before.sort();
    base_entries_after.sort();
    prop_assert_eq!(base_entries_after, base_entries_before);
  }
}
