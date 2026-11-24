// Copyright 2018-2026 the Deno authors. MIT license.

//! Traversal-resistant file access under a directory handle.
//!
//! An [`FsRoot`] holds an open handle to a directory. Every name passed to
//! it is resolved relative to that handle, and resolution never leaves the
//! directory: absolute names, `..` that climbs above it, and symlinks that
//! point outside it all fail with [`FsRootError::Escape`]. `..` and symlinks
//! that stay inside are allowed. This follows Go's `os.Root`.
//!
//! Resolution is done by `cap-std`, which walks names relative to the handle
//! (`openat2` with `RESOLVE_BENEATH` on Linux where available, a
//! component-by-component walk elsewhere), so a directory swapped for a
//! symlink between two operations can't redirect the second one outside.
//!
//! Permission checks are not done here. A caller that must check a name
//! against the permission system before acting on it splits the name with
//! [`split_parent`], opens the parent with [`FsRoot::open_root`], asks the OS
//! where that handle really is with [`FsRoot::real_path`], checks
//! `real_path + leaf`, and then acts on `leaf` through the parent handle.

use std::ffi::OsStr;
use std::ffi::OsString;
use std::io;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use ende_io::fs::FsStat;

use crate::FsDirEntry;
use crate::OpenOptions;

/// Errors from operations on an [`FsRoot`].
#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum FsRootError {
  /// The name would resolve outside the root: an absolute name, `..` above
  /// the root, a symlink pointing out of it, or (on Windows) a reserved
  /// device name such as `NUL` or `COM1`.
  #[class("PathEscape")]
  #[error("path {} escapes from its root", .0.display())]
  Escape(PathBuf),
  #[class(inherit)]
  #[error("{0}")]
  Io(#[source] io::Error),
}

/// The one error cap-std gives for every escape: `..` above the root, an
/// absolute name, a symlink pointing out, and `EXDEV` from Linux's `openat2`
/// with `RESOLVE_BENEATH`. `escape_error_names_the_path_and_its_class` pins
/// it (that `..` reaches cap-std unchecked), so a cap-std
/// upgrade that rewords it fails a test instead of turning escapes into
/// plain I/O errors.
const CAP_STD_ESCAPE_MESSAGE: &str = "a path led outside of the filesystem";

impl FsRootError {
  fn from_io(name: &Path, err: io::Error) -> Self {
    if is_cap_std_escape(&err) {
      Self::Escape(name.to_path_buf())
    } else {
      Self::Io(err)
    }
  }

  fn escape(name: &Path) -> Self {
    Self::Escape(name.to_path_buf())
  }
}

/// An escape is `PermissionDenied` with no OS error code and cap-std's
/// message; a real `EACCES` always carries its code.
fn is_cap_std_escape(err: &io::Error) -> bool {
  err.kind() == io::ErrorKind::PermissionDenied
    && err.raw_os_error().is_none()
    && err
      .get_ref()
      .is_some_and(|inner| inner.to_string() == CAP_STD_ESCAPE_MESSAGE)
}

/// Refuses names that can't be resolved under a root before cap-std sees
/// them: absolute names (Windows drive and root prefixes included) and, on
/// Windows, reserved device names and NTFS stream names (`a:b`,
/// `a::$DATA`) in any component. A stream name reaches a file under a name
/// that a permission rule on the file doesn't match.
fn check_name(name: &Path) -> Result<(), FsRootError> {
  for component in name.components() {
    match component {
      Component::Prefix(_) | Component::RootDir => {
        return Err(FsRootError::escape(name));
      }
      Component::Normal(part)
        if cfg!(windows)
          && (is_windows_device_name(part) || is_windows_stream_name(part)) =>
      {
        return Err(FsRootError::escape(name));
      }
      _ => {}
    }
  }
  Ok(())
}

/// `name` with `.` dropped and `..` applied to the components before it,
/// without looking at the file system: for checking a name against the
/// permission system before it is resolved. Fails with
/// [`FsRootError::Escape`] for an absolute name or one whose `..` climbs above
/// the root.
pub fn lexical_name(name: &Path) -> Result<PathBuf, FsRootError> {
  check_name(name)?;
  let mut parts: Vec<&OsStr> = Vec::new();
  for component in name.components() {
    match component {
      Component::Normal(part) => parts.push(part),
      Component::ParentDir => {
        parts.pop().ok_or_else(|| FsRootError::escape(name))?;
      }
      _ => {}
    }
  }
  Ok(parts.iter().collect())
}

/// A directory that names are resolved under without escaping it.
#[derive(Debug)]
pub struct FsRoot {
  dir: cap_std::fs::Dir,
}

impl FsRoot {
  /// Opens `path` as a root. `path` itself is an ordinary path, resolved
  /// the usual way: it is the caller's job to have checked it.
  pub fn open_ambient(path: &Path) -> Result<Self, FsRootError> {
    let dir =
      cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())
        .map_err(FsRootError::Io)?;
    Ok(Self { dir })
  }

  /// Where the root's directory really is now, as the OS reports it for the
  /// open handle (symlinks resolved; follows the directory if it was moved).
  pub fn real_path(&self) -> io::Result<PathBuf> {
    real_path_of(&self.dir)
  }

  /// Opens a file under the root. A final symlink is followed while it
  /// stays inside the root.
  pub fn open(
    &self,
    name: &Path,
    options: &OpenOptions,
  ) -> Result<std::fs::File, FsRootError> {
    check_name(name)?;
    let file = self
      .dir
      .open_with(name, &cap_open_options(options))
      .map_err(|err| FsRootError::from_io(name, err))?;
    Ok(file.into_std())
  }

  /// Opens `leaf`, a single name in this directory, without following it
  /// if it is a symlink: the open then fails instead. For acting on a name
  /// whose parent was checked by its real path (see the module docs), so
  /// the file acted on is the one that was checked.
  pub fn open_leaf(
    &self,
    leaf: &OsStr,
    options: &OpenOptions,
  ) -> Result<std::fs::File, FsRootError> {
    use cap_fs_ext::OpenOptionsFollowExt;
    let name = Path::new(leaf);
    check_name(name)?;
    let mut cap = cap_open_options(options);
    cap.follow(cap_fs_ext::FollowSymlinks::No);
    let file = self
      .dir
      .open_with(name, &cap)
      .map_err(|err| FsRootError::from_io(name, err))?;
    Ok(file.into_std())
  }

  /// Opens `name`, file or directory, read-only, following a final symlink
  /// while it stays inside the root, so its metadata and real path can be
  /// read from the handle. Doesn't wait for the other end of a FIFO.
  pub fn open_for_metadata(
    &self,
    name: &Path,
  ) -> Result<std::fs::File, FsRootError> {
    use cap_fs_ext::OpenOptionsMaybeDirExt;
    check_name(name)?;
    let mut cap = cap_std::fs::OpenOptions::new();
    cap.read(true).maybe_dir(true);
    #[cfg(unix)]
    {
      use cap_std::fs::OpenOptionsExt;
      cap.custom_flags(libc::O_NONBLOCK);
    }
    let file = self
      .dir
      .open_with(name, &cap)
      .map_err(|err| FsRootError::from_io(name, err))?;
    Ok(file.into_std())
  }

  /// Opens `leaf`, a single name in this directory, without following it if
  /// it is a symlink (the open then fails), with as little access as the OS
  /// allows, so the handle's real path can be checked before the name is
  /// acted on. On Windows that real path is the file's long name, whatever
  /// short (8.3) name `leaf` used.
  pub fn open_leaf_for_check(
    &self,
    leaf: &OsStr,
  ) -> Result<std::fs::File, FsRootError> {
    use cap_fs_ext::OpenOptionsFollowExt;
    use cap_fs_ext::OpenOptionsMaybeDirExt;
    let name = Path::new(leaf);
    check_name(name)?;
    let mut cap = cap_std::fs::OpenOptions::new();
    cap
      .read(true)
      .maybe_dir(true)
      .follow(cap_fs_ext::FollowSymlinks::No);
    #[cfg(unix)]
    {
      use cap_std::fs::OpenOptionsExt;
      cap.custom_flags(libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
      use cap_std::fs::OpenOptionsExt;
      use windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES;
      use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;
      use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
      use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE;
      cap
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let file = self
      .dir
      .open_with(name, &cap)
      .map_err(|err| FsRootError::from_io(name, err))?;
    Ok(file.into_std())
  }

  /// Renames `from`, a single name in this directory, to `to`, a single
  /// name in `to_dir`. Neither is followed if it is a symlink.
  pub fn rename_leaf(
    &self,
    from: &OsStr,
    to_dir: &FsRoot,
    to: &OsStr,
  ) -> Result<(), FsRootError> {
    let (from, to) = (Path::new(from), Path::new(to));
    check_name(from)?;
    check_name(to)?;
    self
      .dir
      .rename(from, &to_dir.dir, to)
      .map_err(|err| FsRootError::from_io(from, err))
  }

  /// Opens a directory under the root as a root of its own: names under it
  /// can't escape it, even into the rest of this root.
  pub fn open_root(&self, name: &Path) -> Result<FsRoot, FsRootError> {
    check_name(name)?;
    let dir = self
      .dir
      .open_dir(name)
      .map_err(|err| FsRootError::from_io(name, err))?;
    Ok(FsRoot { dir })
  }

  /// Creates a directory; with `recursive`, its missing parents too.
  pub fn mkdir(
    &self,
    name: &Path,
    recursive: bool,
    mode: Option<u32>,
  ) -> Result<(), FsRootError> {
    check_name(name)?;
    let mut builder = cap_std::fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    if let Some(mode) = mode {
      use cap_std::fs::DirBuilderExt;
      builder.mode(mode & 0o777);
    }
    #[cfg(not(unix))]
    let _ = mode;
    self
      .dir
      .create_dir_with(name, &builder)
      .map_err(|err| FsRootError::from_io(name, err))
  }

  /// Removes a file, a symlink (not its target) or an empty directory;
  /// with `recursive`, a directory and everything in it.
  pub fn remove(
    &self,
    name: &Path,
    recursive: bool,
  ) -> Result<(), FsRootError> {
    check_name(name)?;
    let map = |err| FsRootError::from_io(name, err);
    let metadata = self.dir.symlink_metadata(name).map_err(map)?;
    let file_type = metadata.file_type();
    if file_type.is_dir() {
      if recursive {
        self.dir.remove_dir_all(name).map_err(map)
      } else {
        self.dir.remove_dir(name).map_err(map)
      }
    } else if file_type.is_symlink() && is_directory_symlink(&metadata) {
      self.dir.remove_dir(name).map_err(map)
    } else {
      self.dir.remove_file(name).map_err(map)
    }
  }

  /// Metadata of `name`, following a final symlink while it stays inside.
  pub fn stat(&self, name: &Path) -> Result<FsStat, FsRootError> {
    check_name(name)?;
    let metadata = self
      .dir
      .metadata(name)
      .map_err(|err| FsRootError::from_io(name, err))?;
    Ok(fs_stat(&metadata))
  }

  /// Metadata of `name` itself, not following a final symlink.
  pub fn lstat(&self, name: &Path) -> Result<FsStat, FsRootError> {
    check_name(name)?;
    let metadata = self
      .dir
      .symlink_metadata(name)
      .map_err(|err| FsRootError::from_io(name, err))?;
    Ok(fs_stat(&metadata))
  }

  /// The entries of a directory under the root. Names that aren't UTF-8
  /// are decoded lossily, as `RealFs` does.
  pub fn read_dir(&self, name: &Path) -> Result<Vec<FsDirEntry>, FsRootError> {
    check_name(name)?;
    let entries = self
      .dir
      .read_dir(name)
      .map_err(|err| FsRootError::from_io(name, err))?;
    let mut result = Vec::new();
    for entry in entries {
      let entry = entry.map_err(FsRootError::Io)?;
      let file_type = entry.file_type().map_err(FsRootError::Io)?;
      result.push(FsDirEntry {
        name: entry.file_name().to_string_lossy().into_owned(),
        is_file: file_type.is_file(),
        is_directory: file_type.is_dir(),
        is_symlink: file_type.is_symlink(),
      });
    }
    Ok(result)
  }

  /// Renames `from` to `to`; both names are under this root.
  pub fn rename(&self, from: &Path, to: &Path) -> Result<(), FsRootError> {
    check_name(from)?;
    check_name(to)?;
    self.dir.rename(from, &self.dir, to).map_err(|err| {
      if is_cap_std_escape(&err) {
        FsRootError::escape(self.escaping_name(from, to))
      } else {
        FsRootError::Io(err)
      }
    })
  }

  /// Which of two names escaped, for the error message: the first whose
  /// parent directory can't be opened under the root, else `from`.
  fn escaping_name<'a>(&self, from: &'a Path, to: &'a Path) -> &'a Path {
    [from, to]
      .into_iter()
      .find(|name| {
        let parent = match name.parent() {
          Some(parent) if !parent.as_os_str().is_empty() => parent,
          _ => Path::new("."),
        };
        let leaf_climbs =
          matches!(name.components().next_back(), Some(Component::ParentDir));
        leaf_climbs
          || self
            .dir
            .open_dir(parent)
            .is_err_and(|err| is_cap_std_escape(&err))
      })
      .unwrap_or(from)
  }
}

fn cap_open_options(options: &OpenOptions) -> cap_std::fs::OpenOptions {
  let mut cap = cap_std::fs::OpenOptions::new();
  cap
    .read(options.read)
    .write(options.write)
    .create(options.create)
    .truncate(options.truncate)
    .append(options.append)
    .create_new(options.create_new);
  #[cfg(unix)]
  {
    use cap_std::fs::OpenOptionsExt;
    if let Some(mode) = options.mode {
      // Only used when the file is created; 0o666 (before umask) otherwise.
      cap.mode(mode & 0o777);
    }
    if let Some(flags) = options.custom_flags {
      cap.custom_flags(flags);
    }
  }
  cap
}

/// On Windows a symlink to a directory is removed as a directory.
#[cfg(windows)]
fn is_directory_symlink(metadata: &cap_std::fs::Metadata) -> bool {
  use cap_std::fs::MetadataExt;
  use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
  metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

#[cfg(not(windows))]
fn is_directory_symlink(_metadata: &cap_std::fs::Metadata) -> bool {
  false
}

/// `FsStat::from_std` for cap-std's metadata, which can't be turned into a
/// `std::fs::Metadata`. Keep the two in step.
fn fs_stat(metadata: &cap_std::fs::Metadata) -> FsStat {
  fn to_msec(time: io::Result<cap_std::time::SystemTime>) -> Option<i64> {
    let time = time.ok()?.into_std();
    Some(match time.duration_since(std::time::UNIX_EPOCH) {
      Ok(duration) => duration.as_millis() as i64,
      // Pre-epoch: negate the duration
      Err(err) => -(err.duration().as_millis() as i64),
    })
  }

  let file_type = metadata.file_type();
  #[cfg(unix)]
  let stat = {
    use cap_std::fs::FileTypeExt;
    use cap_std::fs::MetadataExt;
    FsStat {
      is_file: metadata.is_file(),
      is_directory: metadata.is_dir(),
      is_symlink: file_type.is_symlink(),
      size: metadata.len(),
      mtime: to_msec(metadata.modified()),
      atime: to_msec(metadata.accessed()),
      birthtime: to_msec(metadata.created()),
      // ctime is seconds since epoch; 0 means unknown.
      ctime: (metadata.ctime() != 0).then(|| metadata.ctime() * 1000),
      dev: metadata.dev(),
      ino: Some(metadata.ino()),
      mode: metadata.mode(),
      nlink: Some(metadata.nlink()),
      uid: metadata.uid(),
      gid: metadata.gid(),
      rdev: metadata.rdev(),
      blksize: metadata.blksize(),
      blocks: Some(metadata.blocks()),
      is_block_device: file_type.is_block_device(),
      is_char_device: file_type.is_char_device(),
      is_fifo: file_type.is_fifo(),
      is_socket: file_type.is_socket(),
    }
  };
  #[cfg(not(unix))]
  let stat = FsStat {
    is_file: metadata.is_file(),
    is_directory: metadata.is_dir(),
    is_symlink: file_type.is_symlink(),
    size: metadata.len(),
    mtime: to_msec(metadata.modified()),
    atime: to_msec(metadata.accessed()),
    birthtime: to_msec(metadata.created()),
    ctime: None,
    dev: 0,
    ino: None,
    mode: 0,
    nlink: None,
    uid: 0,
    gid: 0,
    rdev: 0,
    blksize: 0,
    blocks: None,
    is_block_device: false,
    is_char_device: false,
    is_fifo: false,
    is_socket: false,
  };
  stat
}

/// Where an open file really is, as the OS reports it for the handle. For a
/// check after opening through a root: a final symlink that leaves the
/// file's own directory but stays in the root can't be acted on through the
/// parent handle (see `leaf_symlink_out_of_its_parent_needs_the_root`).
pub fn real_path_of_file(file: &std::fs::File) -> io::Result<PathBuf> {
  real_path_of(file)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn real_path_of(fd: impl std::os::fd::AsFd) -> io::Result<PathBuf> {
  let mut path = PathBuf::new();
  nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETPATH(&mut path))?;
  Ok(path)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn real_path_of(fd: impl std::os::fd::AsFd) -> io::Result<PathBuf> {
  use std::os::fd::AsRawFd;
  // Fails where /proc isn't mounted; callers then fail closed.
  let link = format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd());
  let path = PathBuf::from(nix::fcntl::readlink(link.as_str())?);
  // Linux names a file whose last link is gone `<path> (deleted)`, which no
  // permission rule matches: report it as gone instead.
  if path.as_os_str().as_encoded_bytes().ends_with(b" (deleted)")
    && nix::sys::stat::fstat(fd.as_fd())?.st_nlink == 0
  {
    return Err(io::Error::new(
      io::ErrorKind::NotFound,
      format!("{} was deleted", path.display()),
    ));
  }
  Ok(path)
}

#[cfg(all(
  unix,
  not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
  ))
))]
fn real_path_of(_fd: impl std::os::fd::AsFd) -> io::Result<PathBuf> {
  Err(io::Error::new(
    io::ErrorKind::Unsupported,
    "the path of an open directory is not available on this platform",
  ))
}

#[cfg(windows)]
fn real_path_of(
  handle: impl std::os::windows::io::AsHandle,
) -> io::Result<PathBuf> {
  use std::os::windows::ffi::OsStringExt;
  use std::os::windows::io::AsRawHandle;

  use windows_sys::Win32::Storage::FileSystem::FILE_NAME_NORMALIZED;
  use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;
  use windows_sys::Win32::Storage::FileSystem::VOLUME_NAME_DOS;

  let raw = handle.as_handle().as_raw_handle();
  let mut buf = vec![0u16; 512];
  loop {
    // SAFETY: `raw` is an open handle borrowed for the duration of the call,
    // and `buf` is writable for `buf.len()` UTF-16 units.
    let len = unsafe {
      GetFinalPathNameByHandleW(
        raw as _,
        buf.as_mut_ptr(),
        buf.len() as u32,
        FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
      )
    } as usize;
    if len == 0 {
      return Err(io::Error::last_os_error());
    }
    if len < buf.len() {
      buf.truncate(len);
      break;
    }
    // Too small: `len` is the size needed, terminating null included.
    buf.resize(len, 0);
  }
  Ok(deno_path_util::strip_unc_prefix(PathBuf::from(
    OsString::from_wide(&buf),
  )))
}

/// Splits `name` into the directory part to open with
/// [`FsRoot::open_root`] and the final component to act on through it.
/// `"a/b/c"` gives `("a/b", "c")`, `"c"` gives `(".", "c")`. Fails with
/// [`FsRootError::Escape`] for an absolute name or one that climbs above the
/// root (`".."`, `"a/../.."`), and with `InvalidInput` for a name with no
/// final component (`""`, `"."`, `"a/.."`).
pub fn split_parent(name: &Path) -> Result<(PathBuf, OsString), FsRootError> {
  check_name(name)?;
  let mut depth = 0usize;
  for component in name.components() {
    match component {
      Component::Normal(_) => depth += 1,
      Component::ParentDir => {
        depth = depth
          .checked_sub(1)
          .ok_or_else(|| FsRootError::escape(name))?;
      }
      _ => {}
    }
  }
  match name.components().next_back() {
    Some(Component::Normal(leaf)) => {
      let parent = match name.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
      };
      Ok((parent.to_path_buf(), leaf.to_os_string()))
    }
    _ => Err(FsRootError::Io(io::Error::new(
      io::ErrorKind::InvalidInput,
      format!("{} names no file or directory", name.display()),
    ))),
  }
}

/// Whether `component` names an NTFS alternate data stream (`a:b`,
/// `a::$DATA`). Roots refuse these on Windows.
pub fn is_windows_stream_name(component: &OsStr) -> bool {
  component.as_encoded_bytes().contains(&b':')
}

/// Whether `component` is a name Windows reserves for a device (`NUL`,
/// `COM1`, `LPT1`, ...), with or without an extension and trailing spaces.
/// Roots refuse these on Windows, where opening one reaches the device
/// instead of a file.
pub fn is_windows_device_name(component: &OsStr) -> bool {
  let Some(name) = component.to_str() else {
    return false;
  };
  let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
  let stem = stem.to_uppercase();
  match stem.as_str() {
    "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
    _ => ["COM", "LPT"].iter().any(|prefix| {
      stem.strip_prefix(prefix).is_some_and(|digit| {
        matches!(
          digit,
          "0"
            | "1"
            | "2"
            | "3"
            | "4"
            | "5"
            | "6"
            | "7"
            | "8"
            | "9"
            | "¹"
            | "²"
            | "³"
        )
      })
    }),
  }
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;

  struct Fixture {
    // Holds the directories; removed on drop.
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root_path: PathBuf,
    root: FsRoot,
  }

  /// `<tmp>/root` (the root, holding `file.txt` and `dir/inner.txt`) and
  /// `<tmp>/outside.txt` next to it.
  fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();
    let root_path = base.join("root");
    mkdir(&root_path);
    mkdir(&root_path.join("dir"));
    write(&root_path.join("file.txt"), "inside");
    write(&root_path.join("dir").join("inner.txt"), "inner");
    write(&base.join("outside.txt"), "outside");
    let root = FsRoot::open_ambient(&root_path).unwrap();
    Fixture {
      _tmp: tmp,
      base,
      root_path,
      root,
    }
  }

  #[allow(clippy::disallowed_methods, reason = "test fixture setup")]
  fn mkdir(path: &Path) {
    fs::create_dir(path).unwrap();
  }

  #[allow(clippy::disallowed_methods, reason = "test fixture setup")]
  fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
  }

  #[allow(clippy::disallowed_methods, reason = "test assertion")]
  fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
  }

  #[allow(clippy::disallowed_methods, reason = "test assertion")]
  fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
  }

  #[allow(clippy::disallowed_methods, reason = "test assertion")]
  fn canonical(path: &Path) -> PathBuf {
    deno_path_util::strip_unc_prefix(fs::canonicalize(path).unwrap())
  }

  #[cfg(unix)]
  fn symlink(target: impl AsRef<Path>, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
  }

  fn read_through(root: &FsRoot, name: &str) -> Result<String, FsRootError> {
    use std::io::Read;
    let mut file = root.open(Path::new(name), &OpenOptions::read())?;
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    Ok(text)
  }

  #[track_caller]
  fn assert_escape<T>(result: Result<T, FsRootError>) {
    match result {
      Err(FsRootError::Escape(_)) => {}
      Err(err) => panic!("expected an escape, got {err:?}"),
      Ok(_) => panic!("expected an escape, got Ok"),
    }
  }

  fn write_options() -> OpenOptions {
    OpenOptions {
      write: true,
      create: true,
      truncate: true,
      ..Default::default()
    }
  }

  // Resolution

  #[test]
  fn dot_dot_that_stays_inside_is_allowed() {
    let f = fixture();
    assert_eq!(read_through(&f.root, "dir/../file.txt").unwrap(), "inside");
  }

  #[test]
  fn dot_dot_above_the_root_escapes() {
    let f = fixture();
    assert_escape(read_through(&f.root, "../outside.txt"));
    assert_escape(read_through(&f.root, "dir/../../outside.txt"));
  }

  #[test]
  fn absolute_name_escapes() {
    let f = fixture();
    let absolute = f.base.join("outside.txt");
    assert_escape(read_through(&f.root, absolute.to_str().unwrap()));
    // Even when it points inside the root.
    let inside = f.root_path.join("file.txt");
    assert_escape(read_through(&f.root, inside.to_str().unwrap()));
  }

  #[cfg(unix)]
  #[test]
  fn symlink_that_stays_inside_is_followed() {
    let f = fixture();
    symlink("file.txt", &f.root_path.join("link"));
    symlink("../file.txt", &f.root_path.join("dir").join("up"));
    assert_eq!(read_through(&f.root, "link").unwrap(), "inside");
    assert_eq!(read_through(&f.root, "dir/up").unwrap(), "inside");
  }

  #[cfg(unix)]
  #[test]
  fn symlink_pointing_outside_escapes() {
    let f = fixture();
    symlink("../outside.txt", &f.root_path.join("link"));
    assert_escape(read_through(&f.root, "link"));
  }

  #[cfg(unix)]
  #[test]
  fn absolute_symlink_target_escapes() {
    let f = fixture();
    // Absolute targets escape even when they name a file inside the root.
    symlink(f.root_path.join("file.txt"), &f.root_path.join("abs"));
    assert_escape(read_through(&f.root, "abs"));
  }

  #[cfg(unix)]
  #[test]
  fn symlink_loop_is_an_error_but_not_an_escape() {
    let f = fixture();
    symlink("b", &f.root_path.join("a"));
    symlink("a", &f.root_path.join("b"));
    match read_through(&f.root, "a") {
      Err(FsRootError::Io(_)) => {}
      other => panic!("expected an I/O error, got {other:?}"),
    }
  }

  #[cfg(unix)]
  #[test]
  fn directory_swapped_for_a_symlink_after_opening_escapes() {
    let f = fixture();
    let dir = f.root_path.join("dir");
    #[allow(clippy::disallowed_methods, reason = "test fixture setup")]
    fs::remove_dir_all(&dir).unwrap();
    mkdir(&f.base.join("elsewhere"));
    write(&f.base.join("elsewhere").join("inner.txt"), "elsewhere");
    symlink("../elsewhere", &dir);
    assert_escape(read_through(&f.root, "dir/inner.txt"));
  }

  // On Windows cap-std opens the directory without `FILE_SHARE_DELETE`, so
  // it can't be renamed while a root holds it (see the Windows test below).
  #[cfg(unix)]
  #[test]
  fn root_is_a_handle_not_a_path() {
    let f = fixture();
    let moved = f.base.join("moved");
    #[allow(clippy::disallowed_methods, reason = "test fixture setup")]
    fs::rename(&f.root_path, &moved).unwrap();
    // A new directory at the old path is not what the root refers to.
    mkdir(&f.root_path);
    assert_eq!(read_through(&f.root, "file.txt").unwrap(), "inside");
    f.root.open(Path::new("new.txt"), &write_options()).unwrap();
    assert!(exists(&moved.join("new.txt")));
    assert!(!exists(&f.root_path.join("new.txt")));
  }

  #[test]
  fn real_path_is_where_the_directory_is() {
    let f = fixture();
    assert_eq!(f.root.real_path().unwrap(), canonical(&f.root_path));
  }

  #[cfg(unix)]
  #[test]
  fn real_path_is_where_the_directory_is_now() {
    let f = fixture();
    let moved = f.base.join("moved");
    #[allow(clippy::disallowed_methods, reason = "test fixture setup")]
    fs::rename(&f.root_path, &moved).unwrap();
    assert_eq!(f.root.real_path().unwrap(), canonical(&moved));
  }

  #[cfg(windows)]
  #[test]
  fn root_directory_cannot_be_renamed_while_open() {
    let f = fixture();
    #[allow(clippy::disallowed_methods, reason = "test fixture setup")]
    let renamed = fs::rename(&f.root_path, f.base.join("moved"));
    assert!(renamed.is_err());
    assert_eq!(read_through(&f.root, "file.txt").unwrap(), "inside");
  }

  #[test]
  fn escape_error_names_the_path_and_its_class() {
    let f = fixture();
    let err = read_through(&f.root, "../outside.txt").unwrap_err();
    assert_eq!(err.to_string(), "path ../outside.txt escapes from its root");
    assert_eq!(deno_error::JsErrorClass::get_class(&err), "PathEscape");
  }

  #[test]
  fn not_found_stays_an_io_error() {
    let f = fixture();
    match read_through(&f.root, "missing.txt") {
      Err(FsRootError::Io(err)) => {
        assert_eq!(err.kind(), io::ErrorKind::NotFound)
      }
      other => panic!("expected NotFound, got {other:?}"),
    }
  }

  // Windows device names

  #[test]
  fn device_names_are_recognised() {
    for name in ["NUL", "nul", "COM1", "com1.txt", "LPT9", "CON", "AUX.log"] {
      assert!(is_windows_device_name(OsStr::new(name)), "{name}");
    }
    for name in ["NULL", "COM10", "console", "file.txt", "nul-file"] {
      assert!(!is_windows_device_name(OsStr::new(name)), "{name}");
    }
  }

  #[cfg(windows)]
  #[test]
  fn device_names_escape_on_windows() {
    let f = fixture();
    assert_escape(read_through(&f.root, "NUL"));
    assert_escape(read_through(&f.root, "dir/COM1.txt"));
    assert_escape(f.root.mkdir(Path::new("LPT1"), false, None));
  }

  // Operations: each once inside, once escaping

  #[test]
  fn open_creates_and_writes_inside() {
    use std::io::Write;
    let f = fixture();
    let mut file = f
      .root
      .open(Path::new("dir/new.txt"), &write_options())
      .unwrap();
    file.write_all(b"written").unwrap();
    drop(file);
    assert_eq!(read(&f.root_path.join("dir").join("new.txt")), "written");
    assert_escape(f.root.open(Path::new("../new.txt"), &write_options()));
    assert!(!exists(&f.base.join("new.txt")));
  }

  #[test]
  fn open_root_scopes_to_the_subdirectory() {
    let f = fixture();
    let sub = f.root.open_root(Path::new("dir")).unwrap();
    assert_eq!(read_through(&sub, "inner.txt").unwrap(), "inner");
    // The sub-root's own boundary applies, not the parent's.
    assert_escape(read_through(&sub, "../file.txt"));
    assert_escape(f.root.open_root(Path::new("..")));
  }

  #[test]
  fn mkdir_inside_and_escaping() {
    let f = fixture();
    f.root.mkdir(Path::new("made"), false, None).unwrap();
    f.root
      .mkdir(Path::new("deep/er/still"), true, None)
      .unwrap();
    assert!(exists(&f.root_path.join("made")));
    assert!(exists(&f.root_path.join("deep").join("er").join("still")));
    assert_escape(f.root.mkdir(Path::new("../made"), false, None));
    assert!(!exists(&f.base.join("made")));
  }

  #[cfg(unix)]
  #[test]
  fn mkdir_applies_the_mode() {
    let f = fixture();
    f.root
      .mkdir(Path::new("private"), false, Some(0o700))
      .unwrap();
    let mode = f.root.stat(Path::new("private")).unwrap().mode;
    assert_eq!(mode & 0o777, 0o700);
  }

  #[test]
  fn remove_inside_and_escaping() {
    let f = fixture();
    f.root.remove(Path::new("file.txt"), false).unwrap();
    assert!(!exists(&f.root_path.join("file.txt")));
    f.root.remove(Path::new("dir"), true).unwrap();
    assert!(!exists(&f.root_path.join("dir")));
    assert_escape(f.root.remove(Path::new("../outside.txt"), false));
    assert!(exists(&f.base.join("outside.txt")));
  }

  #[cfg(unix)]
  #[test]
  fn remove_takes_away_a_symlink_not_its_target() {
    let f = fixture();
    symlink("file.txt", &f.root_path.join("link"));
    f.root.remove(Path::new("link"), false).unwrap();
    assert!(!exists(&f.root_path.join("link")));
    assert!(exists(&f.root_path.join("file.txt")));
  }

  #[test]
  fn remove_refuses_a_non_empty_directory_without_recursive() {
    let f = fixture();
    assert!(matches!(
      f.root.remove(Path::new("dir"), false),
      Err(FsRootError::Io(_))
    ));
    assert!(exists(&f.root_path.join("dir").join("inner.txt")));
  }

  #[test]
  fn stat_inside_and_escaping() {
    let f = fixture();
    let stat = f.root.stat(Path::new("file.txt")).unwrap();
    assert!(stat.is_file);
    assert_eq!(stat.size, 6);
    assert!(stat.mtime.is_some());
    assert!(f.root.stat(Path::new("dir")).unwrap().is_directory);
    assert_escape(f.root.stat(Path::new("../outside.txt")));
  }

  #[cfg(unix)]
  #[test]
  fn stat_follows_a_symlink_and_lstat_does_not() {
    let f = fixture();
    symlink("file.txt", &f.root_path.join("link"));
    let stat = f.root.stat(Path::new("link")).unwrap();
    assert!(stat.is_file && !stat.is_symlink);
    let lstat = f.root.lstat(Path::new("link")).unwrap();
    assert!(lstat.is_symlink && !lstat.is_file);
    // lstat of a link pointing out is fine: the link itself is inside.
    symlink("../outside.txt", &f.root_path.join("out"));
    assert!(f.root.lstat(Path::new("out")).unwrap().is_symlink);
    assert_escape(f.root.stat(Path::new("out")));
  }

  #[test]
  fn lstat_escaping() {
    let f = fixture();
    assert_escape(f.root.lstat(Path::new("../outside.txt")));
  }

  #[test]
  fn read_dir_inside_and_escaping() {
    let f = fixture();
    let mut names = f
      .root
      .read_dir(Path::new("."))
      .unwrap()
      .into_iter()
      .map(|entry| (entry.name, entry.is_directory))
      .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
      names,
      vec![("dir".to_string(), true), ("file.txt".to_string(), false)]
    );
    assert_eq!(f.root.read_dir(Path::new("dir")).unwrap().len(), 1);
    assert_escape(f.root.read_dir(Path::new("..")));
  }

  #[test]
  fn rename_inside_and_escaping() {
    let f = fixture();
    f.root
      .rename(Path::new("file.txt"), Path::new("dir/moved.txt"))
      .unwrap();
    assert_eq!(read(&f.root_path.join("dir").join("moved.txt")), "inside");
    assert_escape(
      f.root
        .rename(Path::new("dir/moved.txt"), Path::new("../moved.txt")),
    );
    assert!(!exists(&f.base.join("moved.txt")));
    assert_escape(
      f.root
        .rename(Path::new("../outside.txt"), Path::new("taken.txt")),
    );
    assert!(exists(&f.base.join("outside.txt")));
  }

  // Splitting a name for the permission check

  #[test]
  fn split_parent_gives_the_directory_and_the_leaf() {
    let (parent, leaf) = split_parent(Path::new("a/b/c.txt")).unwrap();
    assert_eq!(parent, Path::new("a/b"));
    assert_eq!(leaf, "c.txt");
    let (parent, leaf) = split_parent(Path::new("c.txt")).unwrap();
    assert_eq!(parent, Path::new("."));
    assert_eq!(leaf, "c.txt");
    let (parent, leaf) = split_parent(Path::new("a/../c.txt")).unwrap();
    assert_eq!(parent, Path::new("a/.."));
    assert_eq!(leaf, "c.txt");
  }

  #[test]
  fn split_parent_refuses_names_without_a_leaf_or_escaping() {
    assert_escape(split_parent(Path::new("..")));
    assert_escape(split_parent(Path::new("a/../../x")));
    let absolute = if cfg!(windows) { "C:\\x" } else { "/x" };
    assert_escape(split_parent(Path::new(absolute)));
    for name in ["", ".", "a/.."] {
      match split_parent(Path::new(name)) {
        Err(FsRootError::Io(err)) => {
          assert_eq!(err.kind(), io::ErrorKind::InvalidInput)
        }
        other => panic!("expected InvalidInput for {name:?}, got {other:?}"),
      }
    }
  }

  #[test]
  fn real_path_of_an_opened_file() {
    let f = fixture();
    let file = f
      .root
      .open(Path::new("dir/../file.txt"), &OpenOptions::read())
      .unwrap();
    assert_eq!(
      real_path_of_file(&file).unwrap(),
      canonical(&f.root_path.join("file.txt"))
    );
  }

  /// Through the parent handle, a final symlink is resolved against the
  /// parent, so one that climbs out of its directory is refused even though
  /// it stays in the root. Through the root it works, and the opened file's
  /// real path can be checked afterwards.
  #[cfg(unix)]
  #[test]
  fn leaf_symlink_out_of_its_parent_needs_the_root() {
    let f = fixture();
    symlink("../file.txt", &f.root_path.join("dir").join("up"));
    let (parent, leaf) = split_parent(Path::new("dir/up")).unwrap();
    let parent_root = f.root.open_root(&parent).unwrap();
    assert_escape(read_through(&parent_root, leaf.to_str().unwrap()));
    let file = f
      .root
      .open(Path::new("dir/up"), &OpenOptions::read())
      .unwrap();
    assert_eq!(
      real_path_of_file(&file).unwrap(),
      canonical(&f.root_path.join("file.txt"))
    );
  }

  #[test]
  fn parent_handle_real_path_plus_leaf_is_the_target() {
    let f = fixture();
    let (parent, leaf) = split_parent(Path::new("dir/inner.txt")).unwrap();
    let parent_root = f.root.open_root(&parent).unwrap();
    assert_eq!(
      parent_root.real_path().unwrap().join(&leaf),
      canonical(&f.root_path.join("dir").join("inner.txt"))
    );
    assert_eq!(
      read_through(&parent_root, leaf.to_str().unwrap()).unwrap(),
      "inner"
    );
  }

  #[test]
  fn open_leaf_creates_and_writes_a_plain_name() {
    use std::io::Write;
    let f = fixture();
    let dir = f.root.open_root(Path::new("dir")).unwrap();
    let mut file = dir
      .open_leaf(OsStr::new("new.txt"), &write_options())
      .unwrap();
    file.write_all(b"new").unwrap();
    assert_eq!(read(&f.root_path.join("dir").join("new.txt")), "new");
  }

  /// A symlink as the leaf is refused, even one pointing inside: what was
  /// checked is the link's own path.
  #[cfg(unix)]
  #[test]
  fn open_leaf_does_not_follow_a_symlink() {
    let f = fixture();
    symlink("file.txt", &f.root_path.join("link"));
    assert!(
      f.root
        .open_leaf(OsStr::new("link"), &write_options())
        .is_err()
    );
    assert!(
      f.root
        .open_leaf(OsStr::new("link"), &OpenOptions::read())
        .is_err()
    );
    assert_eq!(read(&f.root_path.join("file.txt")), "inside");
  }

  #[test]
  fn open_leaf_refuses_names_that_climb() {
    let f = fixture();
    assert_escape(f.root.open_leaf(OsStr::new(".."), &OpenOptions::read()));
  }

  #[test]
  fn open_for_metadata_opens_files_and_directories() {
    let f = fixture();
    let file = f.root.open_for_metadata(Path::new("file.txt")).unwrap();
    assert!(file.metadata().unwrap().is_file());
    let dir = f.root.open_for_metadata(Path::new("dir")).unwrap();
    assert!(dir.metadata().unwrap().is_dir());
    assert_eq!(
      real_path_of_file(&dir).unwrap(),
      canonical(&f.root_path.join("dir"))
    );
    assert_escape(f.root.open_for_metadata(Path::new("../outside.txt")));
  }

  #[cfg(unix)]
  #[test]
  fn open_for_metadata_follows_a_symlink_inside_and_refuses_one_out() {
    let f = fixture();
    symlink("file.txt", &f.root_path.join("in"));
    symlink("../outside.txt", &f.root_path.join("out"));
    let file = f.root.open_for_metadata(Path::new("in")).unwrap();
    assert_eq!(
      real_path_of_file(&file).unwrap(),
      canonical(&f.root_path.join("file.txt"))
    );
    assert_escape(f.root.open_for_metadata(Path::new("out")));
  }

  #[cfg(unix)]
  #[test]
  fn open_for_metadata_does_not_wait_on_a_fifo() {
    let f = fixture();
    nix::unistd::mkfifo(
      &f.root_path.join("fifo"),
      nix::sys::stat::Mode::S_IRWXU,
    )
    .unwrap();
    let file = f.root.open_for_metadata(Path::new("fifo")).unwrap();
    use std::os::unix::fs::FileTypeExt;
    assert!(file.metadata().unwrap().file_type().is_fifo());
  }

  #[test]
  fn rename_leaf_moves_between_directories() {
    let f = fixture();
    let dir = f.root.open_root(Path::new("dir")).unwrap();
    f.root
      .rename_leaf(OsStr::new("file.txt"), &dir, OsStr::new("moved.txt"))
      .unwrap();
    assert_eq!(read(&f.root_path.join("dir").join("moved.txt")), "inside");
    assert!(!exists(&f.root_path.join("file.txt")));
    assert_escape(f.root.rename_leaf(
      OsStr::new("dir"),
      &dir,
      OsStr::new(".."),
    ));
  }

  // Checks before resolving (`lexical_name`, `open_leaf_for_check`)

  #[test]
  fn lexical_name_drops_dots_without_resolving() {
    for (name, want) in [
      ("a/../b", "b"),
      ("./a/./b", "a/b"),
      ("a/b/..", "a"),
      ("a/..", ""),
      ("", ""),
    ] {
      assert_eq!(lexical_name(Path::new(name)).unwrap(), Path::new(want));
    }
    for name in ["..", "../x", "a/../..", "/etc/passwd"] {
      assert_escape(lexical_name(Path::new(name)));
    }
  }

  #[test]
  fn stream_names_are_recognised() {
    for name in ["a:b", "file.txt::$DATA", ":x"] {
      assert!(is_windows_stream_name(OsStr::new(name)), "{name}");
    }
    for name in ["file.txt", "a-b", "$DATA"] {
      assert!(!is_windows_stream_name(OsStr::new(name)), "{name}");
    }
  }

  /// NTFS opens `file.txt::$DATA` as `file.txt` and `file.txt:x` as a stream
  /// of it, under a name a deny rule on `file.txt` doesn't match.
  #[cfg(windows)]
  #[test]
  fn stream_names_escape_on_windows() {
    let f = fixture();
    assert_escape(read_through(&f.root, "file.txt::$DATA"));
    assert_escape(read_through(&f.root, "file.txt:x"));
    assert_escape(f.root.mkdir(Path::new("dir/a:b"), false, None));
    assert_escape(split_parent(Path::new("dir/file.txt::$DATA")));
  }

  #[test]
  fn open_leaf_for_check_gives_the_real_path() {
    let f = fixture();
    let dir = f.root.open_root(Path::new("dir")).unwrap();
    let file = dir.open_leaf_for_check(OsStr::new("inner.txt")).unwrap();
    assert_eq!(
      real_path_of_file(&file).unwrap(),
      canonical(&f.root_path.join("dir").join("inner.txt"))
    );
    let file = f.root.open_leaf_for_check(OsStr::new("dir")).unwrap();
    assert_eq!(
      real_path_of_file(&file).unwrap(),
      canonical(&f.root_path.join("dir"))
    );
  }

  #[cfg(unix)]
  #[test]
  fn open_leaf_for_check_does_not_follow_a_symlink() {
    let f = fixture();
    symlink("file.txt", &f.root_path.join("link"));
    assert!(f.root.open_leaf_for_check(OsStr::new("link")).is_err());
  }

  /// Linux names an open file whose last link is gone `<path> (deleted)`,
  /// which no permission rule matches.
  #[cfg(any(target_os = "linux", target_os = "android"))]
  #[test]
  #[allow(clippy::disallowed_methods, reason = "test fixture setup")]
  fn real_path_of_a_deleted_file_is_not_found() {
    let f = fixture();
    let file = f
      .root
      .open(Path::new("file.txt"), &OpenOptions::read())
      .unwrap();
    fs::remove_file(f.root_path.join("file.txt")).unwrap();
    let err = real_path_of_file(&file).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
  }
}
