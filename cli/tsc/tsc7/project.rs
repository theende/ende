// Copyright 2026 the Ende authors. MIT license.

//! Lays out what TypeScript 7's `tsc` reads for one check, in a project
//! directory in Ende's cache (never in the user's project).
//!
//! `tsc` resolves imports by looking at files on disk, so everything Ende's
//! module graph resolved has to be there under a name `tsc` finds:
//!
//! - Local files with an extension that matches their media type are read in
//!   place. Their non-relative specifiers (import map aliases, `jsr:`,
//!   `https:`) become exact `paths` entries pointing at what the graph
//!   resolved.
//! - Every other module (remote, JSR, `data:`, extensionless, in-memory) is
//!   copied into `remote/` or `local/` with an extension (extensionless files
//!   crash tsc: denoland/deno#36173). Each import specifier in a copy is
//!   rewritten to the absolute path of its target, and the column shifts are
//!   kept so diagnostics map back to the original text.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;

use deno_ast::MediaType;
use deno_graph::ImportKind;
use deno_graph::Module;
use deno_graph::ModuleGraph;
use ende_core::ModuleSpecifier;
use ende_core::serde_json::Map;
use ende_core::serde_json::Value;
use ende_lib::util::checksum;
use regex::Regex;

/// A file handed to tsc, and what it stands for.
#[derive(Debug)]
pub struct MappedFile {
  /// The module specifier diagnostics report.
  pub specifier: String,
  /// The original module text, for the source line under a diagnostic.
  pub text: Option<Arc<str>>,
  /// Column shifts from rewriting the specifiers of a copy.
  pub shifts: LineShifts,
}

/// The project for one check.
#[derive(Debug, Default)]
pub struct ProjectPlan {
  /// tsc `files`: the check roots, as tsc paths.
  pub root_files: Vec<String>,
  /// tsc `paths` for non-relative specifiers in local files.
  pub paths: Map<String, Value>,
  /// Copies to write: path and rewritten text.
  pub copies: Vec<(PathBuf, String)>,
  /// tsc path key (see [`path_key`]) to the file it stands for.
  pub files: HashMap<String, MappedFile>,
  /// Ende libs that `/// <reference lib="..." />` directives name.
  pub referenced_ende_libs: BTreeSet<String>,
  /// Module names of `declare module "..."` declarations.
  pub ambient_modules: Vec<String>,
}

/// Plans the project in `project_dir` for checking `roots`. `ende_lib` maps a
/// lowercase lib name to its `ende.*` name if Ende has that lib.
pub fn plan(
  graph: &ModuleGraph,
  roots: &[ModuleSpecifier],
  project_dir: &Path,
  ende_lib: impl Fn(&str) -> Option<String>,
) -> ProjectPlan {
  let modules = walk(graph, roots);

  // Where each module is on disk for tsc, and whether it is a copy.
  let mut disk: HashMap<&ModuleSpecifier, (String, bool)> = HashMap::new();
  for module in &modules {
    let (specifier, media_type) = match module {
      Module::Js(m) => (&m.specifier, m.media_type),
      Module::Json(m) => (&m.specifier, m.media_type),
      _ => continue,
    };
    if let Some(path) = in_place_path(specifier, media_type) {
      disk.insert(specifier, (to_tsc_path(&path), false));
    } else if let Some(relative) = mirror_path(specifier, media_type) {
      disk.insert(specifier, (to_tsc_path(&project_dir.join(relative)), true));
    }
  }

  let mut plan = ProjectPlan::default();
  let mut ambient_modules = BTreeSet::new();
  for module in &modules {
    let (specifier, text, js) = match module {
      Module::Js(m) => (&m.specifier, m.source.text.clone(), Some(m)),
      Module::Json(m) => (&m.specifier, m.source.text.clone(), None),
      _ => continue,
    };
    let Some((path, is_copy)) = disk.get(specifier) else {
      continue;
    };
    let mut edits = Vec::new();
    if let Some(js) = js {
      for (key, dep) in &js.dependencies {
        let Some(target) = dep
          .get_type()
          .or_else(|| dep.get_code())
          .and_then(|s| crate::tsc::resolve_graph_specifier_types(s, graph))
          .and_then(|(s, _)| disk.get(&s).map(|(p, _)| p.clone()))
        else {
          continue;
        };
        for import in &dep.imports {
          if import.kind == ImportKind::JsxImportSource {
            // tsc appends `/jsx-runtime` itself, so map the module it asks
            // for (the dependency's key) instead of rewriting the pragma.
            add_path(&mut plan.paths, key, &target);
          } else if *is_copy {
            edits.push(Edit {
              line: import.specifier_range.range.start.line,
              character: import.specifier_range.range.start.character,
              old: import.specifier.clone(),
              new: target.clone(),
            });
          } else if !is_relative_or_absolute(&import.specifier) {
            add_path(&mut plan.paths, &import.specifier, &target);
          }
        }
      }
      plan
        .referenced_ende_libs
        .extend(lib_references(&text).filter_map(|name| ende_lib(&name)));
      if js.media_type.is_typed() {
        ambient_modules.extend(declared_modules(&text));
      }
    }
    let shifts = if *is_copy {
      let (rewritten, shifts) = apply_edits(&text, edits);
      plan.copies.push((PathBuf::from(path), rewritten));
      shifts
    } else {
      LineShifts::default()
    };
    plan.files.insert(
      path_key(path),
      MappedFile {
        specifier: specifier.to_string(),
        text: Some(text),
        shifts,
      },
    );
  }

  for root in roots {
    let root = graph.resolve(root);
    match disk.get(root) {
      Some((path, _)) => plan.root_files.push(path.clone()),
      // Wasm modules aren't mirrored yet (6e).
      None => log::debug!("tsc7: no file for check root {root}"),
    }
  }
  plan.ambient_modules = ambient_modules.into_iter().collect();
  plan
}

/// The modules reachable from `roots`, in breadth-first order.
fn walk<'a>(
  graph: &'a ModuleGraph,
  roots: &'a [ModuleSpecifier],
) -> Vec<&'a Module> {
  let mut seen = HashSet::new();
  let mut pending = VecDeque::new();
  for root in roots {
    let root = graph.resolve(root);
    if seen.insert(root) {
      pending.push_back(root);
    }
  }
  let mut modules = Vec::new();
  while let Some(specifier) = pending.pop_front() {
    let Ok(Some(module)) = graph.try_get(specifier) else {
      continue;
    };
    modules.push(module);
    let Module::Js(js) = module else {
      continue;
    };
    let types_dep = js
      .maybe_types_dependency
      .as_ref()
      .and_then(|d| d.dependency.maybe_specifier());
    let deps = js
      .dependencies
      .values()
      .flat_map(|d| [d.get_type(), d.get_code()])
      .chain([types_dep])
      .flatten();
    for dep in deps {
      let dep = graph.resolve(dep);
      if seen.insert(dep) {
        pending.push_back(dep);
      }
    }
  }
  modules
}

fn add_path(paths: &mut Map<String, Value>, specifier: &str, target: &str) {
  // A `*` would make the key a pattern.
  if specifier.contains('*') {
    return;
  }
  match paths.get(specifier) {
    None => {
      paths.insert(specifier.to_string(), Value::from(vec![target]));
    }
    Some(existing) if existing[0] != target => {
      // The same specifier resolves differently in different files (import
      // map scopes); tsc `paths` can only hold one target.
      log::debug!(
        "tsc7: '{specifier}' resolves to both {existing} and {target}; using \
         the first"
      );
    }
    Some(_) => {}
  }
}

fn is_relative_or_absolute(specifier: &str) -> bool {
  specifier.starts_with("./")
    || specifier.starts_with("../")
    || specifier.starts_with('/')
}

/// The path of a local module tsc can read in place: an existing file whose
/// extension says what the graph says it is.
fn in_place_path(
  specifier: &ModuleSpecifier,
  media_type: MediaType,
) -> Option<PathBuf> {
  if specifier.scheme() != "file" {
    return None;
  }
  let path = deno_path_util::url_to_file_path(specifier).ok()?;
  (ts_extension(media_type).is_some()
    && MediaType::from_path(&path) == media_type
    && path.is_file())
  .then_some(path)
}

/// The extension tsc derives `media_type` from, if it reads such files.
fn ts_extension(media_type: MediaType) -> Option<&'static str> {
  Some(match media_type {
    MediaType::TypeScript => ".ts",
    MediaType::Tsx => ".tsx",
    MediaType::Mts => ".mts",
    MediaType::Cts => ".cts",
    MediaType::Dts => ".d.ts",
    MediaType::Dmts => ".d.mts",
    MediaType::Dcts => ".d.cts",
    MediaType::JavaScript => ".js",
    MediaType::Jsx => ".jsx",
    MediaType::Mjs => ".mjs",
    MediaType::Cjs => ".cjs",
    MediaType::Json => ".json",
    _ => return None,
  })
}

/// Where a copy of `specifier` goes, relative to the project directory:
/// `remote/<scheme>/<host>[_<port>]/<path>` for http(s), `local/<hash>/<name>`
/// for file:, `remote/<scheme>/<hash>` otherwise. The name always ends in the
/// extension for `media_type`; when the URL's doesn't (or it has a query),
/// a hash of the URL keeps the name unique.
fn mirror_path(
  specifier: &ModuleSpecifier,
  media_type: MediaType,
) -> Option<String> {
  let ext = ts_extension(media_type)?;
  let hash = &checksum::r#gen(&[specifier.as_str()])[..16];
  let mut parts = Vec::new();
  let name = match specifier.scheme() {
    "http" | "https" => {
      parts.push("remote".to_string());
      parts.push(specifier.scheme().to_string());
      let host = specifier.host_str().unwrap_or("_");
      parts.push(match specifier.port() {
        Some(port) => sanitize_segment(&format!("{host}_{port}")),
        None => sanitize_segment(host),
      });
      let mut segments = specifier
        .path_segments()
        .map(|s| s.collect::<Vec<_>>())
        .unwrap_or_default();
      let name = segments.pop().unwrap_or_default();
      parts.extend(
        segments
          .into_iter()
          .filter(|s| !s.is_empty())
          .map(sanitize_segment),
      );
      if name.is_empty() { "index" } else { name }.to_string()
    }
    "file" => {
      parts.push("local".to_string());
      parts.push(hash[..8].to_string());
      specifier
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .unwrap_or("index")
        .to_string()
    }
    scheme => {
      parts.push("remote".to_string());
      parts.push(sanitize_segment(scheme));
      String::new()
    }
  };
  let name = sanitize_segment(&name);
  let keeps_name = !name.is_empty()
    && specifier.query().is_none()
    && name.ends_with(ext)
    && MediaType::from_path(Path::new(&name)) == media_type
    // tsc takes any `.d.*.ts` file for a declaration file.
    && (media_type.is_declaration() || !name.contains(".d."));
  parts.push(if keeps_name {
    name
  } else if name.is_empty() {
    format!("{hash}{ext}")
  } else {
    format!("{}.{}{ext}", name.replace(".d.", "_d_"), &hash[..8])
  });
  Some(parts.join("/"))
}

/// Makes one URL path segment safe as a file name on every platform.
fn sanitize_segment(segment: &str) -> String {
  const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9",
  ];
  let mut out: String = segment
    .chars()
    .map(|c| match c {
      '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\' | '/' => '_',
      c if c.is_control() => '_',
      c => c,
    })
    .collect();
  if out == "." || out == ".." {
    out = "_".repeat(out.len());
  }
  if out.ends_with('.') || out.ends_with(' ') {
    out.push('_');
  }
  let stem = out.split('.').next().unwrap_or_default();
  if WINDOWS_RESERVED.contains(&stem.to_ascii_uppercase().as_str()) {
    out.insert(0, '_');
  }
  if out.chars().count() > 80 {
    let hash = &checksum::r#gen(&[segment])[..8];
    out = format!("{}_{hash}", out.chars().take(40).collect::<String>());
  }
  out
}

/// `path` as tsc and tsconfig files spell it: forward slashes, no `\\?\`.
pub fn to_tsc_path(path: &Path) -> String {
  let path = path.to_string_lossy().replace('\\', "/");
  if let Some(rest) = path.strip_prefix("//?/UNC/") {
    format!("//{rest}")
  } else if let Some(rest) = path.strip_prefix("//?/") {
    rest.to_string()
  } else {
    path
  }
}

fn is_absolute(path: &str) -> bool {
  let bytes = path.as_bytes();
  path.starts_with('/')
    || path.starts_with('\\')
    || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// The key a path is looked up by: `path` joined onto `cwd` unless absolute,
/// forward slashes, `.` and `..` resolved. Windows paths (a drive letter or
/// UNC) compare without case, like the file system.
pub fn path_key(path: &str) -> String {
  normalize(path)
}

/// [`path_key`] for a path tsc reported, which is relative to `cwd`.
pub fn reported_path_key(cwd: &str, reported: &str) -> String {
  if is_absolute(reported) {
    normalize(reported)
  } else {
    normalize(&format!("{cwd}/{reported}"))
  }
}

fn normalize(path: &str) -> String {
  let path = path.replace('\\', "/");
  let is_unc = path.starts_with("//");
  let mut parts: Vec<&str> = Vec::new();
  for part in path.split('/') {
    match part {
      "" | "." => {}
      ".." => {
        // Never pop the root (a drive letter).
        if parts.len() > 1 || parts.first().is_some_and(|p| !p.ends_with(':')) {
          parts.pop();
        }
      }
      part => parts.push(part),
    }
  }
  let is_windows = is_unc
    || parts
      .first()
      .is_some_and(|p| p.len() == 2 && p.ends_with(':'));
  let joined = parts.join("/");
  let key = if is_unc {
    format!("//{joined}")
  } else if is_windows {
    joined
  } else {
    format!("/{joined}")
  };
  if is_windows { key.to_lowercase() } else { key }
}

static LIB_REFERENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r#"(?m)^\s*///\s*<reference\s+lib\s*=\s*["']([^"']+)["']"#)
    .unwrap()
});

static DECLARE_MODULE_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r#"declare\s+module\s+["']([^"']+)["']"#).unwrap()
});

/// The lowercase lib names `/// <reference lib="..." />` directives name.
fn lib_references(text: &str) -> impl Iterator<Item = String> + '_ {
  LIB_REFERENCE_RE
    .captures_iter(text)
    .map(|c| c[1].to_ascii_lowercase())
}

/// Module names of `declare module "..."` declarations. Ende's TS 6 checker
/// gets these from tsc to drop "module not found" errors for them; tsc 7's
/// CLI doesn't report them, so they're found textually.
fn declared_modules(text: &str) -> impl Iterator<Item = String> + '_ {
  DECLARE_MODULE_RE
    .captures_iter(text)
    .map(|c| c[1].to_string())
}

/// A specifier to replace in a copy. `character` is the 0-based char index
/// of the specifier's range on `line` (its opening quote, for deno_graph).
#[derive(Debug)]
struct Edit {
  line: usize,
  character: usize,
  old: String,
  new: String,
}

#[derive(Debug, Clone, PartialEq)]
struct Shift {
  /// UTF-16 columns of the replacement in the rewritten line.
  new_start: u64,
  new_end: u64,
  /// UTF-16 column and length of the replaced text in the original line.
  original_start: u64,
  original_len: u64,
}

/// Column shifts per line from rewriting specifiers in a copy.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct LineShifts(HashMap<u64, Vec<Shift>>);

impl LineShifts {
  /// Maps a UTF-16 column on `line` of the copy to the original text. A
  /// column inside a rewritten specifier maps to the original's start.
  pub fn to_original(&self, line: u64, character: u64) -> u64 {
    let Some(shifts) = self.0.get(&line) else {
      return character;
    };
    let mut delta: i64 = 0;
    for shift in shifts {
      if character < shift.new_start {
        break;
      }
      if character < shift.new_end {
        return shift.original_start;
      }
      delta +=
        (shift.new_end - shift.new_start) as i64 - shift.original_len as i64;
    }
    (character as i64 - delta).max(0) as u64
  }
}

/// Applies `edits` to `text`. An edit whose specifier isn't found on its line
/// (escape sequences, say) is skipped: tsc then reports the import as
/// unresolved instead of checking the wrong module.
fn apply_edits(text: &str, mut edits: Vec<Edit>) -> (String, LineShifts) {
  if edits.is_empty() {
    return (text.to_string(), LineShifts::default());
  }
  edits.sort_by_key(|e| (e.line, e.character));
  let mut out = String::with_capacity(text.len() + edits.len() * 64);
  let mut shifts = LineShifts::default();
  let mut edits = edits.into_iter().peekable();
  for (line_index, line) in text.split_inclusive('\n').enumerate() {
    let mut line_edits = Vec::new();
    while let Some(edit) = edits.next_if(|e| e.line == line_index) {
      line_edits.push(edit);
    }
    if line_edits.is_empty() {
      out.push_str(line);
      continue;
    }
    let mut copied_to = 0;
    let mut line_shifts = Vec::new();
    let mut delta: i64 = 0;
    for edit in line_edits {
      let Some(start) = find_specifier(line, copied_to, &edit) else {
        log::debug!("tsc7: '{}' not found on line {}", edit.old, edit.line);
        continue;
      };
      out.push_str(&line[copied_to..start]);
      out.push_str(&edit.new);
      copied_to = start + edit.old.len();
      let original_start = utf16_len(&line[..start]);
      let original_len = utf16_len(&edit.old);
      let new_start = (original_start as i64 + delta) as u64;
      let new_end = new_start + utf16_len(&edit.new);
      delta += utf16_len(&edit.new) as i64 - original_len as i64;
      line_shifts.push(Shift {
        new_start,
        new_end,
        original_start,
        original_len,
      });
    }
    out.push_str(&line[copied_to..]);
    if !line_shifts.is_empty() {
      shifts.0.insert(line_index as u64, line_shifts);
    }
  }
  (out, shifts)
}

/// The byte offset of the occurrence of `edit.old` at or after `from` that's
/// closest to the edit's char index.
fn find_specifier(line: &str, from: usize, edit: &Edit) -> Option<usize> {
  if edit.old.is_empty() {
    return None;
  }
  let target = line
    .char_indices()
    .nth(edit.character)
    .map(|(i, _)| i)
    .unwrap_or(line.len());
  line[from..]
    .match_indices(edit.old.as_str())
    .map(|(i, _)| from + i)
    .min_by_key(|i| i.abs_diff(target))
}

fn utf16_len(s: &str) -> u64 {
  s.encode_utf16().count() as u64
}

#[cfg(test)]
mod tests {
  use super::*;

  fn url(s: &str) -> ModuleSpecifier {
    ModuleSpecifier::parse(s).unwrap()
  }

  #[test]
  fn mirror_paths() {
    let cases = [
      (
        "https://jsr.io/@std/assert/1.0.0/mod.ts",
        MediaType::TypeScript,
        "remote/https/jsr.io/@std/assert/1.0.0/mod.ts",
      ),
      (
        "http://localhost:4545/subdir/type_error.ts",
        MediaType::TypeScript,
        "remote/http/localhost_4545/subdir/type_error.ts",
      ),
      (
        "https://example.com/types.d.ts",
        MediaType::Dts,
        "remote/https/example.com/types.d.ts",
      ),
    ];
    for (specifier, media_type, expected) in cases {
      assert_eq!(
        mirror_path(&url(specifier), media_type).unwrap(),
        expected,
        "{specifier}"
      );
    }
  }

  #[test]
  fn mirror_paths_always_get_an_extension() {
    let hashed = |specifier: &str, media_type| {
      let path = mirror_path(&url(specifier), media_type).unwrap();
      let hash = &checksum::r#gen(&[specifier])[..8];
      path.replace(hash, "HASH")
    };
    assert_eq!(
      hashed("http://localhost:4545/subdir/no_ext", MediaType::TypeScript),
      "remote/http/localhost_4545/subdir/no_ext.HASH.ts"
    );
    assert_eq!(
      hashed("https://example.com/mod.ts?target=x", MediaType::TypeScript),
      "remote/https/example.com/mod.ts.HASH.ts"
    );
    // Served as JavaScript despite the name.
    assert_eq!(
      hashed("https://example.com/mismatch_ext.ts", MediaType::JavaScript),
      "remote/https/example.com/mismatch_ext.ts.HASH.js"
    );
    // TypeScript content in a `.d.` name would be read as a declaration.
    assert_eq!(
      hashed("https://example.com/a.d.ts", MediaType::TypeScript),
      "remote/https/example.com/a_d_ts.HASH.ts"
    );
    assert_eq!(
      hashed("https://example.com/dir/", MediaType::TypeScript),
      "remote/https/example.com/dir/index.HASH.ts"
    );
    assert_eq!(
      hashed("file:///project/script", MediaType::TypeScript),
      "local/HASH/script.HASH.ts"
    );
    let data = "data:application/typescript,export%20{}";
    let path = mirror_path(&url(data), MediaType::TypeScript).unwrap();
    let hash = &checksum::r#gen(&[data])[..16];
    assert_eq!(path, format!("remote/data/{hash}.ts"));
    assert_eq!(mirror_path(&url(data), MediaType::Css), None);
  }

  #[test]
  fn sanitizes_segments() {
    assert_eq!(sanitize_segment("a:b*c?d|e\"f<g>h"), "a_b_c_d_e_f_g_h");
    assert_eq!(sanitize_segment(".."), "__");
    assert_eq!(sanitize_segment("trailing."), "trailing._");
    assert_eq!(sanitize_segment("con.ts"), "_con.ts");
    assert_eq!(sanitize_segment("Lpt1"), "_Lpt1");
    let long = "x".repeat(100);
    let hash = &checksum::r#gen(&[long.as_str()])[..8];
    assert_eq!(
      sanitize_segment(&long),
      format!("{}_{hash}", "x".repeat(40))
    );
  }

  #[test]
  fn tsc_paths() {
    assert_eq!(to_tsc_path(Path::new("/a/b.ts")), "/a/b.ts");
    assert_eq!(
      to_tsc_path(Path::new(r"C:\Users\a\b.ts")),
      "C:/Users/a/b.ts"
    );
    assert_eq!(to_tsc_path(Path::new(r"\\?\C:\a\b.ts")), "C:/a/b.ts");
    assert_eq!(
      to_tsc_path(Path::new(r"\\?\UNC\server\share\b.ts")),
      "//server/share/b.ts"
    );
  }

  #[test]
  fn path_keys() {
    assert_eq!(path_key("/a/./b/../c.ts"), "/a/c.ts");
    assert_eq!(
      reported_path_key("/cache/tsc7/abc", "../../../proj/main.ts"),
      "/proj/main.ts"
    );
    assert_eq!(
      reported_path_key("/cache/tsc7/abc", "remote/https/x.ts"),
      "/cache/tsc7/abc/remote/https/x.ts"
    );
    assert_eq!(reported_path_key("/cache", "/abs/x.ts"), "/abs/x.ts");
    // Windows: drive letters, backslashes, case.
    assert_eq!(
      reported_path_key("C:/Users/me/cache/abc", "../../proj/Main.ts"),
      "c:/users/me/proj/main.ts"
    );
    assert_eq!(
      reported_path_key("C:/cache", r"D:\Proj\main.ts"),
      "d:/proj/main.ts"
    );
    assert_eq!(path_key(r"c:\proj\..\..\x.ts"), "c:/x.ts");
    assert_eq!(
      reported_path_key("//server/share/cache", "../proj/a.ts"),
      "//server/share/proj/a.ts"
    );
    assert_eq!(path_key("C:/Proj/a.ts"), path_key(r"c:\proj\A.ts"));
    // Unix paths keep their case.
    assert_ne!(path_key("/Proj/a.ts"), path_key("/proj/a.ts"));
  }

  #[test]
  fn edits_and_shifts() {
    let text = "import { a } from \"./a.ts\"; const x: string = a;\n\
                // 😀 import \"./b\"\n\
                import b from './b'; import c from \"./c.ts\"; b + c;\n";
    let edit = |line, character, old: &str, new: &str| Edit {
      line,
      character,
      old: old.into(),
      new: new.into(),
    };
    let (out, shifts) = apply_edits(
      text,
      vec![
        edit(2, 35, "./c.ts", "/cache/c.ts"),
        edit(0, 18, "./a.ts", "/cache/remote/a.ts"),
        edit(2, 14, "./b", "/cache/b.ts"),
        edit(1, 12, "./b", "/x/b.ts"),
        edit(0, 0, "./missing.ts", "/never.ts"),
      ],
    );
    assert_eq!(
      out,
      "import { a } from \"/cache/remote/a.ts\"; const x: string = a;\n\
       // 😀 import \"/x/b.ts\"\n\
       import b from '/cache/b.ts'; import c from \"/cache/c.ts\"; b + c;\n"
    );
    // Before the specifier: unchanged.
    assert_eq!(shifts.to_original(0, 9), 9);
    // Inside the rewritten specifier: its original start.
    assert_eq!(shifts.to_original(0, 25), 19);
    // After it: shifted back by the growth (18 - 6 = 12).
    assert_eq!(shifts.to_original(0, 46), 34);
    // The emoji is 2 UTF-16 units, so the specifier starts at 14, and the
    // closing quote moves from 17 to 21.
    assert_eq!(shifts.to_original(1, 13), 13);
    assert_eq!(shifts.to_original(1, 15), 14);
    assert_eq!(shifts.to_original(1, 21), 17);
    // Two edits on one line: +8 then +5.
    assert_eq!(shifts.to_original(2, 62), 49);
    assert_eq!(shifts.to_original(2, 45), 36);
    assert_eq!(shifts.to_original(3, 4), 4);
  }

  #[test]
  fn lib_references_and_declared_modules() {
    let text = "/// <reference lib=\"Ende.Unstable\" />\n\
                ///<reference lib='dom' />\n\
                // /// <reference lib=\"ende.ns\" /> is not at line start\n\
                declare module \"foo\" {}\ndeclare   module 'bar/*' {}\n";
    assert_eq!(
      lib_references(text).collect::<Vec<_>>(),
      ["ende.unstable", "dom"]
    );
    assert_eq!(declared_modules(text).collect::<Vec<_>>(), ["foo", "bar/*"]);
  }
}
