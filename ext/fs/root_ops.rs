// Copyright 2026 the Ende authors. MIT license.

//! Ops behind `Ende.openRoot()`: file access through an [`FsRoot`], which
//! names can't escape (see `root.rs`).
//!
//! Every operation is permission-checked where it really lands, never by
//! resolving a path string a second time:
//!
//! - Operations that only read and may follow a final symlink (open for
//!   reading, stat, readFile, readTextFile, readDir, openRoot) open the target
//!   through the root, ask the OS for the opened handle's real path, and check
//!   that before anything reaches JS.
//! - Operations that create, write, truncate, remove, rename, mkdir or lstat
//!   open the parent directory through the root, check the parent's real path
//!   plus the leaf, then act on the leaf through the parent handle without
//!   following it. So a final symlink is not written through, even one that
//!   stays inside the root.
//!
//! Before either, the name is checked where it lexically points under the
//! root's real path ([`Pre`]), so a denied name fails the same way whether or
//! not it exists, and resolving it can't block on a FIFO under a denied path.

use std::borrow::Cow;
use std::cell::RefCell;
use std::ffi::OsString;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use deno_error::JsErrorBox;
use ende_core::OpState;
use ende_core::Resource;
use ende_core::ResourceId;
use ende_core::convert::Uint8Array;
use ende_core::op2;
use ende_core::unsync::spawn_blocking;
use ende_io::StdFileResourceInner;
use ende_io::fs::FileResource;
use ende_io::fs::FsError;
use ende_io::fs::FsStat;
use ende_permissions::OpenAccessKind;
use ende_permissions::PermissionsContainer;

use crate::FsDirEntry;
use crate::FsRoot;
use crate::FsRootError;
use crate::OpenOptions;
use crate::interface::FileSystemRc;
use crate::ops::FsOpenOptions;
use crate::ops::FsOpsError;
use crate::ops::FsOpsErrorKind;
use crate::ops::SerializableStat;
use crate::ops::root_operation_error;
use crate::root::lexical_name;
use crate::root::real_path_of_file;
use crate::root::split_parent;

/// A root shared with the blocking pool, so it must be `Send`: `MaybeArc`
/// is an `Rc` unless the `sync` feature is on.
#[allow(
  clippy::disallowed_types,
  reason = "moved into spawn_blocking, which needs Send"
)]
type SharedRoot = std::sync::Arc<FsRoot>;

/// An open `Ende.Root`: the directory handle and what it was opened for.
struct RootResource {
  root: SharedRoot,
  read: bool,
  write: bool,
}

impl Resource for RootResource {
  fn name(&self) -> Cow<'_, str> {
    "fsRoot".into()
  }
}

/// What an operation needs the root to have been opened for.
#[derive(Clone, Copy)]
enum Needs {
  Read,
  Write,
  ReadWrite,
}

impl Needs {
  fn of(options: &OpenOptions) -> Self {
    match options.access_kind() {
      OpenAccessKind::Read | OpenAccessKind::ReadNoFollow => Needs::Read,
      OpenAccessKind::Write | OpenAccessKind::WriteNoFollow => Needs::Write,
      OpenAccessKind::ReadWrite | OpenAccessKind::ReadWriteNoFollow => {
        Needs::ReadWrite
      }
    }
  }

  /// The permission check for a path this operation acts on. The path is
  /// already where the OS says the target is, so it isn't resolved again.
  fn check(self) -> Check {
    Check::Open(match self {
      Needs::Read => OpenAccessKind::ReadNoFollow,
      Needs::Write => OpenAccessKind::WriteNoFollow,
      Needs::ReadWrite => OpenAccessKind::ReadWriteNoFollow,
    })
  }
}

/// A permission check on a path an operation acts on.
#[derive(Clone, Copy)]
enum Check {
  Open(OpenAccessKind),
  /// For a recursive remove: a denied path *inside* it blocks it too.
  WriteStrict,
}

fn not_capable(api: &str, option: &str) -> FsOpsError {
  FsOpsErrorKind::Other(JsErrorBox::new(
    "NotCapable",
    format!(
      "{api} needs a root opened with {{ {option}: true }}; pass it to \
       Ende.openRoot()"
    ),
  ))
  .into_box()
}

fn join_error(err: tokio::task::JoinError) -> FsOpsError {
  FsOpsError::from(FsError::JoinError(err))
}

/// The root behind `rid`, if it was opened for what the operation needs.
fn get_root(
  state: &OpState,
  rid: ResourceId,
  needs: Needs,
  api: &str,
) -> Result<SharedRoot, FsOpsError> {
  let resource = state
    .resource_table
    .get::<RootResource>(rid)
    .map_err(|err| FsOpsErrorKind::Resource(err).into_box())?;
  let (read, write) = match needs {
    Needs::Read => (true, false),
    Needs::Write => (false, true),
    Needs::ReadWrite => (true, true),
  };
  if read && !resource.read {
    return Err(not_capable(api, "read"));
  }
  if write && !resource.write {
    return Err(not_capable(api, "write"));
  }
  Ok(resource.root.clone())
}

fn check(
  state: &OpState,
  paths: Vec<(PathBuf, Check)>,
  api: &str,
) -> Result<(), FsOpsError> {
  let permissions = state.borrow::<PermissionsContainer>();
  for (path, check) in paths {
    match check {
      Check::Open(kind) => {
        permissions.check_open(Cow::Owned(path), kind, Some(api))?;
      }
      Check::WriteStrict => {
        permissions.check_write(Cow::Owned(path), api)?;
      }
    }
  }
  Ok(())
}

/// Names an operation checks before resolving them, at their lexical place
/// under the root's real path. The check on where a name really lands comes
/// after resolving; this one keeps resolving (and what it reveals: whether a
/// name exists, a FIFO's open waiting for a writer) behind the permission
/// system too.
struct Pre {
  operation: &'static str,
  names: Vec<(PathBuf, Check)>,
}

impl Pre {
  fn one(operation: &'static str, name: &Path, check: Check) -> Self {
    Self {
      operation,
      names: vec![(name.to_path_buf(), check)],
    }
  }
}

fn pre_check(
  state: &OpState,
  root: &FsRoot,
  pre: Pre,
  api: &str,
) -> Result<(), FsOpsError> {
  let Some((first, _)) = pre.names.first() else {
    return Ok(());
  };
  let real = root.real_path().map_err(io_error(pre.operation, first))?;
  let mut paths = Vec::with_capacity(pre.names.len());
  for (name, kind) in &pre.names {
    let lexical = lexical_name(name).map_err(named(pre.operation, name))?;
    let path = if lexical.as_os_str().is_empty() {
      real.clone()
    } else {
      real.join(lexical)
    };
    paths.push((path, *kind));
  }
  check(state, paths, api)
}

/// Reports an escape under the whole name the caller gave, not the part of
/// it (a parent, a leaf) that was being resolved when it escaped.
fn named<'a>(
  operation: &'static str,
  name: &'a Path,
) -> impl Fn(FsRootError) -> FsOpsError + 'a {
  move |err| {
    let err = match err {
      FsRootError::Escape(_) => FsRootError::Escape(name.to_path_buf()),
      err => err,
    };
    root_operation_error(operation, name, err)
  }
}

fn io_error<'a>(
  operation: &'static str,
  name: &'a Path,
) -> impl Fn(std::io::Error) -> FsOpsError + 'a {
  move |err| root_operation_error(operation, name, FsRootError::Io(err))
}

/// A name resolved for acting on: the target to act on, and the paths to
/// check first.
type Resolved<T> = (T, Vec<(PathBuf, Check)>);

/// Opens `name` through the root, following a final symlink inside it, and
/// resolves it to the opened file's real path (returned with the file: the
/// path it was checked at).
fn resolve_target(
  root: &FsRoot,
  name: &Path,
  options: &OpenOptions,
  operation: &'static str,
  needs: Needs,
) -> Result<Resolved<(std::fs::File, PathBuf)>, FsOpsError> {
  let file = root.open(name, options).map_err(named(operation, name))?;
  let real = real_path_of_file(&file).map_err(io_error(operation, name))?;
  Ok(((file, real.clone()), vec![(real, needs.check())]))
}

/// The parent of `name`, opened through the root, its leaf, and the path the
/// leaf was checked at.
struct Parent {
  dir: FsRoot,
  leaf: OsString,
  checked: PathBuf,
}

/// Opens the parent of `name` through the root and resolves `name` to the
/// parent's real path plus the leaf.
///
/// On Windows an existing leaf is also opened (not followed) and checked at
/// its handle's real path: NTFS reaches a file through its short (8.3) name
/// too, which a permission rule on the long name doesn't match.
fn resolve_parent(
  root: &FsRoot,
  name: &Path,
  operation: &'static str,
  check: Check,
) -> Result<Resolved<Parent>, FsOpsError> {
  let (parent, leaf) = split_parent(name).map_err(named(operation, name))?;
  let dir = root.open_root(&parent).map_err(named(operation, name))?;
  let real = dir
    .real_path()
    .map_err(io_error(operation, name))?
    .join(&leaf);
  #[cfg(not(windows))]
  let (paths, checked) = (vec![(real.clone(), check)], real);
  #[cfg(windows)]
  let (paths, checked) = match existing_leaf_real_path(&dir, &leaf)
    .map_err(named(operation, name))?
  {
    Some(leaf_real) => {
      (vec![(real, check), (leaf_real.clone(), check)], leaf_real)
    }
    None => (vec![(real.clone(), check)], real),
  };
  Ok((Parent { dir, leaf, checked }, paths))
}

/// The real path of `leaf` in `dir` if it exists. A symlink isn't opened (it
/// is acted on by the name given); any other failure to open fails closed.
#[cfg(windows)]
fn existing_leaf_real_path(
  dir: &FsRoot,
  leaf: &std::ffi::OsStr,
) -> Result<Option<PathBuf>, FsRootError> {
  match dir.open_leaf_for_check(leaf) {
    Ok(file) => real_path_of_file(&file).map(Some).map_err(FsRootError::Io),
    Err(FsRootError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
      Ok(None)
    }
    Err(err) => match dir.lstat(Path::new(leaf)) {
      Ok(stat) if stat.is_symlink => Ok(None),
      _ => Err(err),
    },
  }
}

/// Runs an operation on the root behind `rid` in a sync op: check the names
/// lexically, resolve, check where they landed, act.
fn run_sync<T, R>(
  state: &OpState,
  rid: ResourceId,
  needs: Needs,
  api: &str,
  pre: Pre,
  resolve: impl FnOnce(&FsRoot) -> Result<Resolved<T>, FsOpsError>,
  act: impl FnOnce(T) -> Result<R, FsOpsError>,
) -> Result<R, FsOpsError> {
  let root = get_root(state, rid, needs, api)?;
  pre_check(state, &root, pre, api)?;
  let (target, paths) = resolve(&root)?;
  check(state, paths, api)?;
  act(target)
}

/// [`run_sync`] for an async op: resolving and acting run on the blocking
/// pool, the permission checks on the runtime thread before and between
/// them.
#[allow(clippy::too_many_arguments, reason = "the steps of one operation")]
async fn run_async<T, R>(
  state: Rc<RefCell<OpState>>,
  rid: ResourceId,
  needs: Needs,
  api: &'static str,
  pre: Pre,
  resolve: impl FnOnce(&FsRoot) -> Result<Resolved<T>, FsOpsError> + Send + 'static,
  act: impl FnOnce(T) -> Result<R, FsOpsError> + Send + 'static,
) -> Result<R, FsOpsError>
where
  T: Send + 'static,
  R: Send + 'static,
{
  let root = get_root(&state.borrow(), rid, needs, api)?;
  pre_check(&state.borrow(), &root, pre, api)?;
  let (target, paths) = spawn_blocking(move || resolve(&root))
    .await
    .map_err(join_error)??;
  check(&state.borrow(), paths, api)?;
  spawn_blocking(move || act(target))
    .await
    .map_err(join_error)?
}

/// Adds a file opened through a root with the path it was checked at, so
/// `Ende.FsFile` methods that check permissions (`utime`, `stat`) check it,
/// as for a file from `Ende.open()`.
fn add_file(
  state: &mut OpState,
  file: std::fs::File,
  checked: PathBuf,
) -> ResourceId {
  let file: Rc<dyn ende_io::fs::File> =
    Rc::new(StdFileResourceInner::file(file, Some(checked)));
  state
    .resource_table
    .add(FileResource::new(file, "fsFile".to_string()))
}

fn add_root(
  state: &mut OpState,
  root: FsRoot,
  read: bool,
  write: bool,
) -> ResourceId {
  state.resource_table.add(RootResource {
    root: SharedRoot::new(root),
    read,
    write,
  })
}

// --- Ende.openRoot() ---

fn open_root_access(read: bool, write: bool) -> OpenAccessKind {
  match (read, write) {
    (true, true) => OpenAccessKind::ReadWrite,
    (false, true) => OpenAccessKind::Write,
    _ => OpenAccessKind::Read,
  }
}

/// Checks `path` and refuses one inside a compiled executable's embedded
/// files, which have no directory to hold a handle to.
fn check_root_path(
  state: &OpState,
  path: &str,
  read: bool,
  write: bool,
  api: &str,
) -> Result<PathBuf, FsOpsError> {
  let path = state
    .borrow::<PermissionsContainer>()
    .check_open(
      Cow::Owned(PathBuf::from(path)),
      open_root_access(read, write),
      Some(api),
    )?
    .into_owned_path();
  if state.borrow::<FileSystemRc>().is_virtual_path(&path) {
    return Err(
      FsOpsErrorKind::Other(JsErrorBox::new(
        "NotSupported",
        format!(
          "{api} is not supported for files embedded in a compiled \
           executable: {}",
          path.display()
        ),
      ))
      .into_box(),
    );
  }
  Ok(path)
}

fn open_root_at(path: &Path) -> Result<(FsRoot, PathBuf), FsOpsError> {
  let root = FsRoot::open_ambient(path)
    .map_err(|err| root_operation_error("openroot", path, err))?;
  let real = root.real_path().map_err(|err| {
    root_operation_error("openroot", path, FsRootError::Io(err))
  })?;
  Ok((root, real))
}

/// The directory opened may not be the one checked if the path changed in
/// between; check the one the handle points at too.
fn check_opened_root(
  state: &OpState,
  real: PathBuf,
  read: bool,
  write: bool,
  api: &str,
) -> Result<(), FsOpsError> {
  let kind = match open_root_access(read, write) {
    OpenAccessKind::ReadWrite => OpenAccessKind::ReadWriteNoFollow,
    OpenAccessKind::Write => OpenAccessKind::WriteNoFollow,
    _ => OpenAccessKind::ReadNoFollow,
  };
  check(state, vec![(real, Check::Open(kind))], api)
}

#[op2(stack_trace)]
pub fn op_fs_root_open_sync(
  state: &mut OpState,
  #[string] path: &str,
  read: bool,
  write: bool,
) -> Result<ResourceId, FsOpsError> {
  let api = "Ende.openRootSync()";
  let path = check_root_path(state, path, read, write, api)?;
  let (root, real) = open_root_at(&path)?;
  check_opened_root(state, real, read, write, api)?;
  Ok(add_root(state, root, read, write))
}

#[op2(stack_trace)]
pub async fn op_fs_root_open_async(
  state: Rc<RefCell<OpState>>,
  #[string] path: String,
  read: bool,
  write: bool,
) -> Result<ResourceId, FsOpsError> {
  let api = "Ende.openRoot()";
  let path = check_root_path(&state.borrow(), &path, read, write, api)?;
  let (root, real) = spawn_blocking(move || open_root_at(&path))
    .await
    .map_err(join_error)??;
  check_opened_root(&state.borrow(), real, read, write, api)?;
  Ok(add_root(&mut state.borrow_mut(), root, read, write))
}

// --- Root.openRoot() ---

fn resolve_sub_root(
  root: &FsRoot,
  name: &Path,
  needs: Needs,
) -> Result<Resolved<FsRoot>, FsOpsError> {
  let sub = root.open_root(name).map_err(named("openroot", name))?;
  let real = sub.real_path().map_err(io_error("openroot", name))?;
  Ok((sub, vec![(real, needs.check())]))
}

fn access_of(
  state: &OpState,
  rid: ResourceId,
) -> Result<(bool, bool), FsOpsError> {
  let resource = state
    .resource_table
    .get::<RootResource>(rid)
    .map_err(|err| FsOpsErrorKind::Resource(err).into_box())?;
  Ok((resource.read, resource.write))
}

fn sub_root_needs(read: bool, write: bool) -> Needs {
  if write && read {
    Needs::ReadWrite
  } else if write {
    Needs::Write
  } else {
    Needs::Read
  }
}

#[op2(stack_trace)]
pub fn op_fs_root_open_root_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
) -> Result<ResourceId, FsOpsError> {
  let (read, write) = access_of(state, rid)?;
  let needs = sub_root_needs(read, write);
  let name = Path::new(name);
  let sub = run_sync(
    state,
    rid,
    needs,
    "Ende.Root.openRootSync()",
    Pre::one("openroot", name, needs.check()),
    |root| resolve_sub_root(root, name, needs),
    Ok,
  )?;
  Ok(add_root(state, sub, read, write))
}

#[op2(stack_trace)]
pub async fn op_fs_root_open_root_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
) -> Result<ResourceId, FsOpsError> {
  let (read, write) = access_of(&state.borrow(), rid)?;
  let needs = sub_root_needs(read, write);
  let pre = Pre::one("openroot", Path::new(&name), needs.check());
  let sub = run_async(
    state.clone(),
    rid,
    needs,
    "Ende.Root.openRoot()",
    pre,
    move |root| resolve_sub_root(root, Path::new(&name), needs),
    Ok,
  )
  .await?;
  Ok(add_root(&mut state.borrow_mut(), sub, read, write))
}

// --- Root.open(), Root.create() ---

/// Reading opens follow a final symlink inside the root; anything that can
/// change the file goes through its parent and doesn't.
fn follows(options: &OpenOptions) -> bool {
  !(options.write
    || options.append
    || options.create
    || options.create_new
    || options.truncate)
}

enum Opened {
  Target(std::fs::File, PathBuf),
  Leaf(Parent),
}

fn resolve_open(
  root: &FsRoot,
  name: &Path,
  options: &OpenOptions,
) -> Result<Resolved<Opened>, FsOpsError> {
  let needs = Needs::of(options);
  if follows(options) {
    let ((file, real), paths) =
      resolve_target(root, name, options, "open", needs)?;
    Ok((Opened::Target(file, real), paths))
  } else {
    let (parent, paths) = resolve_parent(root, name, "open", needs.check())?;
    Ok((Opened::Leaf(parent), paths))
  }
}

/// The opened file and the path it was checked at.
fn finish_open(
  opened: Opened,
  name: &Path,
  options: &OpenOptions,
) -> Result<(std::fs::File, PathBuf), FsOpsError> {
  match opened {
    Opened::Target(file, real) => Ok((file, real)),
    Opened::Leaf(parent) => {
      let file = parent
        .dir
        .open_leaf(&parent.leaf, options)
        .map_err(named("open", name))?;
      Ok((file, parent.checked))
    }
  }
}

fn open_options(options: Option<FsOpenOptions>) -> OpenOptions {
  match options {
    Some(options) => OpenOptions::from(options),
    None => OpenOptions::read(),
  }
}

#[op2(stack_trace)]
pub fn op_fs_root_open_file_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
  #[scoped] options: Option<FsOpenOptions>,
) -> Result<ResourceId, FsOpsError> {
  let options = open_options(options);
  let name = Path::new(name);
  let needs = Needs::of(&options);
  let (file, checked) = run_sync(
    state,
    rid,
    needs,
    "Ende.Root.openSync()",
    Pre::one("open", name, needs.check()),
    |root| resolve_open(root, name, &options),
    |opened| finish_open(opened, name, &options),
  )?;
  Ok(add_file(state, file, checked))
}

#[op2(stack_trace)]
pub async fn op_fs_root_open_file_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
  #[scoped] options: Option<FsOpenOptions>,
) -> Result<ResourceId, FsOpsError> {
  let options = open_options(options);
  let name = PathBuf::from(name);
  let name2 = name.clone();
  let needs = Needs::of(&options);
  let (file, checked) = run_async(
    state.clone(),
    rid,
    needs,
    "Ende.Root.open()",
    Pre::one("open", &name, needs.check()),
    move |root| resolve_open(root, &name, &options),
    move |opened| finish_open(opened, &name2, &options),
  )
  .await?;
  Ok(add_file(&mut state.borrow_mut(), file, checked))
}

// --- Root.readFile(), Root.readTextFile() ---

fn resolve_read(
  root: &FsRoot,
  name: &Path,
) -> Result<Resolved<std::fs::File>, FsOpsError> {
  let ((file, _), paths) =
    resolve_target(root, name, &OpenOptions::read(), "readfile", Needs::Read)?;
  Ok((file, paths))
}

fn read_pre(name: &Path) -> Pre {
  Pre::one("readfile", name, Needs::Read.check())
}

fn read_all(
  mut file: std::fs::File,
  name: &Path,
) -> Result<Vec<u8>, FsOpsError> {
  let mut buf = Vec::new();
  file
    .read_to_end(&mut buf)
    .map_err(io_error("readfile", name))?;
  Ok(buf)
}

fn read_text(file: std::fs::File, name: &Path) -> Result<String, FsOpsError> {
  let buf = read_all(file, name)?;
  Ok(match String::from_utf8(buf) {
    Ok(text) => text,
    Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
  })
}

#[op2(stack_trace)]
pub fn op_fs_root_read_file_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
) -> Result<Uint8Array, FsOpsError> {
  let name = Path::new(name);
  let buf = run_sync(
    state,
    rid,
    Needs::Read,
    "Ende.Root.readFileSync()",
    read_pre(name),
    |root| resolve_read(root, name),
    |file| read_all(file, name),
  )?;
  Ok(buf.into())
}

#[op2(stack_trace)]
pub async fn op_fs_root_read_file_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
) -> Result<Uint8Array, FsOpsError> {
  let name = PathBuf::from(name);
  let name2 = name.clone();
  let buf = run_async(
    state,
    rid,
    Needs::Read,
    "Ende.Root.readFile()",
    read_pre(&name),
    move |root| resolve_read(root, &name),
    move |file| read_all(file, &name2),
  )
  .await?;
  Ok(buf.into())
}

#[op2(stack_trace)]
#[string]
pub fn op_fs_root_read_text_file_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
) -> Result<String, FsOpsError> {
  let name = Path::new(name);
  run_sync(
    state,
    rid,
    Needs::Read,
    "Ende.Root.readTextFileSync()",
    read_pre(name),
    |root| resolve_read(root, name),
    |file| read_text(file, name),
  )
}

#[op2(stack_trace)]
#[string]
pub async fn op_fs_root_read_text_file_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
) -> Result<String, FsOpsError> {
  let name = PathBuf::from(name);
  let name2 = name.clone();
  run_async(
    state,
    rid,
    Needs::Read,
    "Ende.Root.readTextFile()",
    read_pre(&name),
    move |root| resolve_read(root, &name),
    move |file| read_text(file, &name2),
  )
  .await
}

// --- Root.writeFile(), Root.writeTextFile() ---

fn write_leaf(
  parent: Parent,
  name: &Path,
  options: &OpenOptions,
  data: &[u8],
) -> Result<(), FsOpsError> {
  let mut file = parent
    .dir
    .open_leaf(&parent.leaf, options)
    .map_err(named("writefile", name))?;
  #[cfg(unix)]
  if let Some(mode) = options.mode {
    use std::os::unix::fs::PermissionsExt;
    file
      .set_permissions(std::fs::Permissions::from_mode(mode))
      .map_err(io_error("writefile", name))?;
  }
  file.write_all(data).map_err(io_error("writefile", name))
}

fn resolve_write(
  root: &FsRoot,
  name: &Path,
) -> Result<Resolved<Parent>, FsOpsError> {
  resolve_parent(root, name, "writefile", Needs::Write.check())
}

#[op2(stack_trace)]
#[allow(clippy::too_many_arguments, reason = "op")]
pub fn op_fs_root_write_file_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
  mode: Option<u32>,
  append: bool,
  create: bool,
  create_new: bool,
  data: Uint8Array,
) -> Result<(), FsOpsError> {
  let options = OpenOptions::write(create, append, create_new, mode);
  let name = Path::new(name);
  run_sync(
    state,
    rid,
    Needs::Write,
    "Ende.Root.writeFileSync()",
    Pre::one("writefile", name, Needs::Write.check()),
    |root| resolve_write(root, name),
    |parent| write_leaf(parent, name, &options, &data),
  )
}

#[op2(stack_trace)]
#[allow(clippy::too_many_arguments, reason = "op")]
pub async fn op_fs_root_write_file_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
  #[smi] mode: Option<u32>,
  append: bool,
  create: bool,
  create_new: bool,
  data: Uint8Array,
) -> Result<(), FsOpsError> {
  let options = OpenOptions::write(create, append, create_new, mode);
  let name = PathBuf::from(name);
  let name2 = name.clone();
  let data = data.0;
  run_async(
    state,
    rid,
    Needs::Write,
    "Ende.Root.writeFile()",
    Pre::one("writefile", &name, Needs::Write.check()),
    move |root| resolve_write(root, &name),
    move |parent| write_leaf(parent, &name2, &options, &data),
  )
  .await
}

// --- Root.mkdir() ---

fn resolve_mkdir(
  root: &FsRoot,
  name: &Path,
) -> Result<Resolved<Parent>, FsOpsError> {
  resolve_parent(root, name, "mkdir", Needs::Write.check())
}

fn mkdir_leaf(
  parent: Parent,
  name: &Path,
  mode: Option<u32>,
) -> Result<(), FsOpsError> {
  parent
    .dir
    .mkdir(Path::new(&parent.leaf), false, mode)
    .map_err(named("mkdir", name))
}

/// The names to create, in order, for a recursive mkdir of `name`: each
/// leading part of it, so every directory is created (and checked) through
/// its own parent.
fn mkdir_steps(name: &Path) -> Vec<PathBuf> {
  let mut steps = Vec::new();
  let mut prefix = PathBuf::new();
  for component in name.components() {
    prefix.push(component);
    steps.push(prefix.clone());
  }
  steps
}

/// Whether a step of a recursive mkdir is already a directory (so it's
/// skipped). An escape or another error stops the mkdir.
fn exists_as_dir(
  root: &FsRoot,
  step: &Path,
  name: &Path,
) -> Result<bool, FsOpsError> {
  match root.open_root(step) {
    Ok(_) => Ok(true),
    Err(FsRootError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
      Ok(false)
    }
    Err(err) => Err(named("mkdir", name)(err)),
  }
}

fn mkdir_mode(mode: Option<u32>) -> Option<u32> {
  Some(mode.unwrap_or(0o777) & 0o777)
}

#[op2(stack_trace)]
pub fn op_fs_root_mkdir_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
  recursive: bool,
  mode: Option<u32>,
) -> Result<(), FsOpsError> {
  let api = "Ende.Root.mkdirSync()";
  let mode = mkdir_mode(mode);
  let name = Path::new(name);
  if !recursive {
    return run_sync(
      state,
      rid,
      Needs::Write,
      api,
      Pre::one("mkdir", name, Needs::Write.check()),
      |root| resolve_mkdir(root, name),
      |parent| mkdir_leaf(parent, name, mode),
    );
  }
  // Checked as a whole first: a step that exists is skipped without a check
  // of its own, which would otherwise tell whether it exists.
  let root = get_root(state, rid, Needs::Write, api)?;
  pre_check(
    state,
    &root,
    Pre::one("mkdir", name, Needs::Write.check()),
    api,
  )?;
  for step in mkdir_steps(name) {
    let root = get_root(state, rid, Needs::Write, api)?;
    if exists_as_dir(&root, &step, name)? {
      continue;
    }
    run_sync(
      state,
      rid,
      Needs::Write,
      api,
      Pre::one("mkdir", &step, Needs::Write.check()),
      |root| resolve_mkdir(root, &step),
      |parent| match mkdir_leaf(parent, name, mode) {
        Err(_) if exists_as_dir(&root, &step, name).unwrap_or(false) => Ok(()),
        result => result,
      },
    )?;
  }
  Ok(())
}

#[op2(stack_trace)]
pub async fn op_fs_root_mkdir_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
  recursive: bool,
  mode: Option<u32>,
) -> Result<(), FsOpsError> {
  let api = "Ende.Root.mkdir()";
  let mode = mkdir_mode(mode);
  let name = PathBuf::from(name);
  let steps = if recursive {
    // Checked as a whole first, as in the sync op.
    let root = get_root(&state.borrow(), rid, Needs::Write, api)?;
    pre_check(
      &state.borrow(),
      &root,
      Pre::one("mkdir", &name, Needs::Write.check()),
      api,
    )?;
    mkdir_steps(&name)
  } else {
    vec![name.clone()]
  };
  for step in steps {
    if recursive {
      let root = get_root(&state.borrow(), rid, Needs::Write, api)?;
      let (step2, name2) = (step.clone(), name.clone());
      let exists = spawn_blocking(move || exists_as_dir(&root, &step2, &name2))
        .await
        .map_err(join_error)??;
      if exists {
        continue;
      }
    }
    let root = get_root(&state.borrow(), rid, Needs::Write, api)?;
    let (step2, name2) = (step.clone(), name.clone());
    let pre = Pre::one("mkdir", &step, Needs::Write.check());
    run_async(
      state.clone(),
      rid,
      Needs::Write,
      api,
      pre,
      move |root| resolve_mkdir(root, &step),
      move |parent| match mkdir_leaf(parent, &name2, mode) {
        Err(_)
          if recursive
            && exists_as_dir(&root, &step2, &name2).unwrap_or(false) =>
        {
          Ok(())
        }
        result => result,
      },
    )
    .await?;
  }
  Ok(())
}

// --- Root.remove() ---

fn remove_check(recursive: bool) -> Check {
  if recursive {
    Check::WriteStrict
  } else {
    Needs::Write.check()
  }
}

fn resolve_remove(
  root: &FsRoot,
  name: &Path,
  recursive: bool,
) -> Result<Resolved<Parent>, FsOpsError> {
  resolve_parent(root, name, "remove", remove_check(recursive))
}

fn remove_leaf(
  parent: Parent,
  name: &Path,
  recursive: bool,
) -> Result<(), FsOpsError> {
  parent
    .dir
    .remove(Path::new(&parent.leaf), recursive)
    .map_err(named("remove", name))
}

#[op2(fast, stack_trace)]
pub fn op_fs_root_remove_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
  recursive: bool,
) -> Result<(), FsOpsError> {
  let name = Path::new(name);
  run_sync(
    state,
    rid,
    Needs::Write,
    "Ende.Root.removeSync()",
    Pre::one("remove", name, remove_check(recursive)),
    |root| resolve_remove(root, name, recursive),
    |parent| remove_leaf(parent, name, recursive),
  )
}

#[op2(stack_trace)]
pub async fn op_fs_root_remove_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
  recursive: bool,
) -> Result<(), FsOpsError> {
  let name = PathBuf::from(name);
  let name2 = name.clone();
  run_async(
    state,
    rid,
    Needs::Write,
    "Ende.Root.remove()",
    Pre::one("remove", &name, remove_check(recursive)),
    move |root| resolve_remove(root, &name, recursive),
    move |parent| remove_leaf(parent, &name2, recursive),
  )
  .await
}

// --- Root.stat(), Root.lstat() ---

enum StatTarget {
  File(std::fs::File),
  /// Read through the parent: `name` couldn't be opened, and isn't a link.
  Stat(FsStat),
}

/// Opens `name` (following a final symlink inside the root) and checks it at
/// its real path. A file the OS won't open even for metadata (no read
/// access, a socket, a file another process holds on Windows) is stat'ed
/// through its parent instead, as `Ende.stat()` does without opening; that
/// only works when it isn't a symlink, whose target can't be checked
/// without opening it.
fn resolve_stat(
  root: &FsRoot,
  name: &Path,
) -> Result<Resolved<StatTarget>, FsOpsError> {
  match root.open_for_metadata(name) {
    Ok(file) => {
      let real = real_path_of_file(&file).map_err(io_error("stat", name))?;
      Ok((StatTarget::File(file), vec![(real, Needs::Read.check())]))
    }
    Err(FsRootError::Io(err)) if cannot_open_for_metadata(&err) => {
      let leaf_stat = resolve_parent(root, name, "stat", Needs::Read.check())
        .ok()
        .and_then(|(parent, paths)| {
          let stat = parent.dir.lstat(Path::new(&parent.leaf)).ok()?;
          (!stat.is_symlink).then_some((stat, paths))
        });
      match leaf_stat {
        Some((stat, paths)) => Ok((StatTarget::Stat(stat), paths)),
        None => Err(named("stat", name)(FsRootError::Io(err))),
      }
    }
    Err(err) => Err(named("stat", name)(err)),
  }
}

/// Errors from opening a file that exists but can't be opened for reading.
fn cannot_open_for_metadata(err: &std::io::Error) -> bool {
  #[cfg(unix)]
  let codes = [libc::EACCES, libc::EPERM, libc::ENXIO, libc::EOPNOTSUPP];
  #[cfg(windows)]
  let codes = [
    windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED as i32,
    windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32,
  ];
  err.raw_os_error().is_some_and(|code| codes.contains(&code))
}

fn stat_file(
  target: StatTarget,
  name: &Path,
) -> Result<SerializableStat, FsOpsError> {
  match target {
    StatTarget::File(file) => {
      let metadata = file.metadata().map_err(io_error("stat", name))?;
      Ok(SerializableStat::from(FsStat::from_std(metadata)))
    }
    StatTarget::Stat(stat) => Ok(SerializableStat::from(stat)),
  }
}

fn resolve_lstat(
  root: &FsRoot,
  name: &Path,
) -> Result<Resolved<Parent>, FsOpsError> {
  resolve_parent(root, name, "lstat", Needs::Read.check())
}

fn lstat_leaf(
  parent: Parent,
  name: &Path,
) -> Result<SerializableStat, FsOpsError> {
  let stat = parent
    .dir
    .lstat(Path::new(&parent.leaf))
    .map_err(named("lstat", name))?;
  Ok(SerializableStat::from(stat))
}

#[op2(stack_trace)]
pub fn op_fs_root_stat_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
) -> Result<SerializableStat, FsOpsError> {
  let name = Path::new(name);
  run_sync(
    state,
    rid,
    Needs::Read,
    "Ende.Root.statSync()",
    Pre::one("stat", name, Needs::Read.check()),
    |root| resolve_stat(root, name),
    |file| stat_file(file, name),
  )
}

#[op2(stack_trace)]
pub async fn op_fs_root_stat_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
) -> Result<SerializableStat, FsOpsError> {
  let name = PathBuf::from(name);
  let name2 = name.clone();
  run_async(
    state,
    rid,
    Needs::Read,
    "Ende.Root.stat()",
    Pre::one("stat", &name, Needs::Read.check()),
    move |root| resolve_stat(root, &name),
    move |file| stat_file(file, &name2),
  )
  .await
}

#[op2(stack_trace)]
pub fn op_fs_root_lstat_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
) -> Result<SerializableStat, FsOpsError> {
  let name = Path::new(name);
  run_sync(
    state,
    rid,
    Needs::Read,
    "Ende.Root.lstatSync()",
    Pre::one("lstat", name, Needs::Read.check()),
    |root| resolve_lstat(root, name),
    |parent| lstat_leaf(parent, name),
  )
}

#[op2(stack_trace)]
pub async fn op_fs_root_lstat_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
) -> Result<SerializableStat, FsOpsError> {
  let name = PathBuf::from(name);
  let name2 = name.clone();
  run_async(
    state,
    rid,
    Needs::Read,
    "Ende.Root.lstat()",
    Pre::one("lstat", &name, Needs::Read.check()),
    move |root| resolve_lstat(root, &name),
    move |parent| lstat_leaf(parent, &name2),
  )
  .await
}

// --- Root.readDir() ---

fn read_dir_of(
  dir: FsRoot,
  name: &Path,
) -> Result<Vec<FsDirEntry>, FsOpsError> {
  dir.read_dir(Path::new(".")).map_err(named("readdir", name))
}

#[op2(stack_trace)]
pub fn op_fs_root_read_dir_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] name: &str,
) -> Result<Vec<FsDirEntry>, FsOpsError> {
  let name = Path::new(name);
  run_sync(
    state,
    rid,
    Needs::Read,
    "Ende.Root.readDirSync()",
    Pre::one("readdir", name, Needs::Read.check()),
    |root| resolve_sub_root(root, name, Needs::Read),
    |dir| read_dir_of(dir, name),
  )
}

#[op2(stack_trace)]
pub async fn op_fs_root_read_dir_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] name: String,
) -> Result<Vec<FsDirEntry>, FsOpsError> {
  let name = PathBuf::from(name);
  let name2 = name.clone();
  run_async(
    state,
    rid,
    Needs::Read,
    "Ende.Root.readDir()",
    Pre::one("readdir", &name, Needs::Read.check()),
    move |root| resolve_sub_root(root, &name, Needs::Read),
    move |dir| read_dir_of(dir, &name2),
  )
  .await
}

// --- Root.rename() ---

/// `from` needs read and write, and a denied path inside it (a directory
/// carries what's in it along) blocks the rename too, as for a recursive
/// remove.
const RENAME_FROM: [Check; 2] = [
  Check::Open(OpenAccessKind::ReadWriteNoFollow),
  Check::WriteStrict,
];

fn rename_pre(from: &Path, to: &Path) -> Pre {
  let mut names: Vec<_> = RENAME_FROM
    .iter()
    .map(|check| (from.to_path_buf(), *check))
    .collect();
  names.push((to.to_path_buf(), Needs::Write.check()));
  Pre {
    operation: "rename",
    names,
  }
}

fn resolve_rename(
  root: &FsRoot,
  from: &Path,
  to: &Path,
) -> Result<Resolved<(Parent, Parent)>, FsOpsError> {
  let (from_parent, from_paths) =
    resolve_parent(root, from, "rename", RENAME_FROM[0])?;
  let mut paths = Vec::new();
  for (path, _) in from_paths {
    for check in RENAME_FROM {
      paths.push((path.clone(), check));
    }
  }
  let (to_parent, to_paths) =
    resolve_parent(root, to, "rename", Needs::Write.check())?;
  paths.extend(to_paths);
  Ok(((from_parent, to_parent), paths))
}

fn rename_leaves(
  (from_parent, to_parent): (Parent, Parent),
  from: &Path,
) -> Result<(), FsOpsError> {
  from_parent
    .dir
    .rename_leaf(&from_parent.leaf, &to_parent.dir, &to_parent.leaf)
    .map_err(named("rename", from))
}

#[op2(fast, stack_trace)]
pub fn op_fs_root_rename_sync(
  state: &mut OpState,
  #[smi] rid: ResourceId,
  #[string] from: &str,
  #[string] to: &str,
) -> Result<(), FsOpsError> {
  let (from, to) = (Path::new(from), Path::new(to));
  run_sync(
    state,
    rid,
    Needs::ReadWrite,
    "Ende.Root.renameSync()",
    rename_pre(from, to),
    |root| resolve_rename(root, from, to),
    |parents| rename_leaves(parents, from),
  )
}

#[op2(stack_trace)]
pub async fn op_fs_root_rename_async(
  state: Rc<RefCell<OpState>>,
  #[smi] rid: ResourceId,
  #[string] from: String,
  #[string] to: String,
) -> Result<(), FsOpsError> {
  let (from, to) = (PathBuf::from(from), PathBuf::from(to));
  let from2 = from.clone();
  run_async(
    state,
    rid,
    Needs::ReadWrite,
    "Ende.Root.rename()",
    rename_pre(&from, &to),
    move |root| resolve_rename(root, &from, &to),
    move |parents| rename_leaves(parents, &from2),
  )
  .await
}
