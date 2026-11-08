// Copyright 2018-2026 the Deno authors. MIT license.

//! Type-checks with TypeScript 7 (`ende check --unstable-tsc7`).
//!
//! Ende drives the `tsc` binary through its CLI, not its unstable API: it
//! writes a project into Ende's cache (see [`super::project`]), runs
//! `tsc -p <tsconfig> --pretty true` on it, and maps the diagnostics back to
//! the modules they're about, so they print like the TypeScript 6 checker's.
//!
//! Based on denoland/deno#36104 and denoland/deno#35938.

use std::cell::OnceCell;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::LazyLock;

use deno_graph::ModuleGraph;
use deno_terminal::colors;
use ende_core::ModuleSpecifier;
use ende_core::anyhow::Context;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::parking_lot::Mutex;
use ende_core::serde_json;
use ende_core::serde_json::json;
use ende_core::url::Url;
use ende_lib::util::checksum;
use ende_lib::util::hash::FastInsecureHasher;
use ende_resolver::ende_json::CompilerOptionsData;
use regex::Regex;

use super::libs;
use super::options;
use super::output;
use super::output::Category;
use super::output::RawDiagnostic;
use super::project;
use super::project::LineShifts;
use super::project::MappedFile;
use super::project::path_key;
use super::project::reported_path_key;
use super::project::to_tsc_path;
use crate::args::CompilerOptions;
use crate::cache::CACHE_PERM;
use crate::cache::CacheDBHash;
use crate::sys::CliSys;
use crate::tsc::Diagnostic;
use crate::tsc::DiagnosticCategory;
use crate::tsc::Diagnostics;
use crate::tsc::IGNORED_DIAGNOSTIC_CODES;
use crate::tsc::LAZILY_LOADED_STATIC_ASSETS;
use crate::tsc::Position;

/// Where `ende check --unstable-tsc7` finds `tsc` and keeps its project.
#[derive(Debug)]
pub struct Tsc7Env {
  tsc: PathBuf,
  project_dir: PathBuf,
  /// Config files already warned about, so each warning prints once.
  warned: Mutex<HashSet<String>>,
  /// Which tsc this is, for the check cache: its path, size and mtime.
  identity: u64,
}

impl Tsc7Env {
  /// One project directory per workspace, under `<ende_dir>/tsc7_projects/`.
  pub fn new(tsc: PathBuf, ende_dir_root: &Path, workspace_root: &Url) -> Self {
    let hash = &checksum::r#gen(&[workspace_root.as_str()])[..16];
    let metadata = std::fs::metadata(&tsc)
      .ok()
      .map(|m| (m.len(), m.modified().ok()));
    let identity = FastInsecureHasher::new_ende_versioned()
      .write_hashable(("tsc7", &tsc, metadata))
      .finish();
    Self {
      tsc,
      project_dir: ende_dir_root.join("tsc7_projects").join(hash),
      warned: Default::default(),
      identity,
    }
  }

  /// The check cache key for a check whose TS 6 key is `hash`. TS 6 and
  /// TS 7 results differ, and so can two tsc builds'.
  pub fn check_hash(&self, hash: CacheDBHash) -> CacheDBHash {
    CacheDBHash::from_hashable((hash, self.identity))
  }

  /// Warns, once per config file, about options TypeScript 7 removed.
  pub fn warn_removed_options(&self, data: &CompilerOptionsData) {
    for removed in options::removed_options(
      data
        .sources
        .iter()
        .map(|s| (s.specifier.as_ref(), s.compiler_options.as_ref())),
    ) {
      let key = format!("{}#{}", removed.specifier, removed.name);
      if self.warned.lock().insert(key) {
        log::warn!("{} {removed}", colors::yellow("Warning:"));
      }
    }
  }
}

pub struct Request<'a> {
  pub graph: &'a ModuleGraph,
  /// The modules to check, as the graph walk found them.
  pub roots: Vec<ModuleSpecifier>,
  /// Ende's resolved check options for these roots.
  pub compiler_options: &'a CompilerOptions,
  pub compiler_options_data: &'a CompilerOptionsData,
}

pub struct Response {
  pub diagnostics: Diagnostics,
  /// Module names of `declare module "..."` declarations in the program.
  pub ambient_modules: Vec<String>,
}

pub fn exec(env: &Tsc7Env, request: Request) -> Result<Response, AnyError> {
  let data = request.compiler_options_data;
  let project_dir = &env.project_dir;
  let types_dir = project_dir.join("types");
  write_ende_libs(&types_dir)?;
  let plan =
    project::plan(request.graph, &request.roots, project_dir, ende_lib_name);
  for (path, text) in &plan.copies {
    write_if_changed(path, text.as_bytes())?;
  }

  let root_dirs = data
    .root_dirs()
    .iter()
    .filter_map(|url| deno_path_util::url_to_file_path(url).ok())
    .map(|path| to_tsc_path(&path))
    .collect();
  // The stock libs, with Ende's additions where it has some.
  let tsc_dir = env.tsc.parent().unwrap_or(Path::new("."));
  let lib_paths =
    libs::write_replacements(tsc_dir, &types_dir, write_if_changed)?;
  let mut paths = plan.paths.clone();
  let replaces_libs = !lib_paths.is_empty();
  paths.extend(lib_paths);
  let mut translated = options::translate(
    &request.compiler_options.0,
    root_dirs,
    paths,
    ende_lib_name,
  );
  if replaces_libs {
    translated
      .compiler_options
      .insert("libReplacement".into(), true.into());
  }
  let ende_libs: BTreeSet<&String> = translated
    .ende_libs
    .iter()
    .chain(&plan.referenced_ende_libs)
    .collect();
  let files = ende_libs
    .into_iter()
    .map(|name| to_tsc_path(&types_dir.join(format!("lib.{name}.d.ts"))))
    .chain(plan.root_files.iter().cloned())
    .collect::<Vec<_>>();
  let tsconfig = json!({
    "compilerOptions": translated.compiler_options,
    "files": files,
  });

  // A fresh name per run, so concurrent checks of one workspace don't share
  // a config; removed when dropped.
  let tsconfig_file = tempfile::Builder::new()
    .prefix("tsconfig.")
    .suffix(".json")
    .tempfile_in(project_dir)
    .with_context(|| {
      format!("failed to create a file in {}", project_dir.display())
    })?;
  std::fs::write(
    tsconfig_file.path(),
    serde_json::to_string_pretty(&tsconfig)?,
  )
  .with_context(|| {
    format!("failed to write {}", tsconfig_file.path().display())
  })?;

  let output = run_tsc(&env.tsc, tsconfig_file.path(), project_dir)?;
  let stdout = String::from_utf8_lossy(&output.stdout);
  log::debug!("tsc7 output:\n{stdout}");
  let raw = output::parse(&stdout);
  if !output.status.success() && raw.is_empty() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let excerpt = format!("{stderr}{stdout}")
      .lines()
      .take(20)
      .collect::<Vec<_>>()
      .join("\n");
    bail!(
      "the TypeScript 7 compiler at {} failed ({}):\n{}",
      env.tsc.display(),
      output.status,
      excerpt.trim_end()
    );
  }

  let mapper = Mapper {
    cwd: to_tsc_path(project_dir),
    files: &plan.files,
    types_dir: path_key(&to_tsc_path(&types_dir)),
    tsc_dir: env
      .tsc
      .parent()
      .map(|p| path_key(&to_tsc_path(p)))
      .unwrap_or_default(),
    tsconfig: path_key(&to_tsc_path(tsconfig_file.path())),
    canonical_files: OnceCell::new(),
    read_files: Default::default(),
  };
  let mut dropped = 0;
  let diagnostics = raw
    .into_iter()
    .filter_map(|d| {
      let mapped = mapper.map(d);
      dropped += usize::from(mapped.is_none());
      mapped
    })
    .filter(|d| {
      // Ende's module graph decides which imports are missing, and reports
      // them itself, so the TS 6 checker drops these codes; so does this.
      // tsc 7 also can't find what 6d doesn't mirror yet (Wasm, CSS, bytes
      // and text imports), which type-check as `any` here.
      let ignored = IGNORED_DIAGNOSTIC_CODES.contains(&d.code);
      if ignored {
        log::debug!("tsc7: ignored {d}");
      }
      !ignored && !is_provided_lib_not_found(d)
    })
    .map(|mut d| {
      d.message_text = d.message_text.map(|m| format_message(m, d.code));
      d
    })
    .collect::<Vec<_>>();
  if dropped > 0 {
    log::debug!("tsc7: dropped {dropped} diagnostics in lib files");
  }

  Ok(Response {
    diagnostics: Diagnostics::from(diagnostics),
    ambient_modules: plan.ambient_modules,
  })
}

fn run_tsc(
  tsc: &Path,
  tsconfig: &Path,
  project_dir: &Path,
) -> Result<std::process::Output, AnyError> {
  log::debug!("tsc7: {} -p {}", tsc.display(), tsconfig.display());
  std::process::Command::new(tsc)
    .arg("-p")
    .arg(tsconfig)
    .arg("--pretty")
    .arg("true")
    .current_dir(project_dir)
    // tsc is written in Go, whose `os.Getwd` trusts `PWD` over `getcwd()`;
    // an inherited `PWD` would make it print paths relative to the wrong
    // directory (from denoland/deno#36104).
    .env("PWD", project_dir)
    .stdin(Stdio::null())
    .output()
    .with_context(|| {
      format!(
        "failed to run the TypeScript 7 compiler at {}",
        tsc.display()
      )
    })
}

/// The `ende.*` name of an Ende lib, given a lowercase `ende.*` or `deno.*`
/// lib name.
fn ende_lib_name(name: &str) -> Option<String> {
  let rest = name
    .strip_prefix("ende.")
    .or_else(|| name.strip_prefix("deno."))?;
  let file = format!("lib.ende.{rest}.d.ts");
  LAZILY_LOADED_STATIC_ASSETS
    .contains_key(file.as_str())
    .then(|| format!("ende.{rest}"))
}

static ENDE_LIB_REFERENCE_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r#"(?m)^(\s*///\s*<reference\s+)lib\s*=\s*["']ende\.([\w.]+)["']"#)
    .unwrap()
});

/// Writes Ende's libs into `types_dir`. tsc only knows its own lib names, so
/// their `/// <reference lib="ende.*" />` directives become path references.
fn write_ende_libs(types_dir: &Path) -> Result<(), AnyError> {
  for (name, asset) in LAZILY_LOADED_STATIC_ASSETS.iter() {
    if !name.starts_with("lib.ende.") {
      continue;
    }
    let text = ENDE_LIB_REFERENCE_RE
      .replace_all(asset.source.as_str(), "${1}path=\"./lib.ende.${2}.d.ts\"");
    write_if_changed(&types_dir.join(name), text.as_bytes())?;
  }
  Ok(())
}

fn write_if_changed(path: &Path, contents: &[u8]) -> Result<(), AnyError> {
  if std::fs::read(path).is_ok_and(|existing| existing == contents) {
    return Ok(());
  }
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent)
      .with_context(|| format!("failed to create {}", parent.display()))?;
  }
  deno_path_util::fs::atomic_write_file_with_retries(
    &CliSys::default(),
    path,
    contents,
    CACHE_PERM,
  )
  .with_context(|| format!("failed to write {}", path.display()))
}

/// The unstable `Ende` members, as `unstableEndeProps` in
/// cli/tsc/97_ts_host.js lists them (a test keeps the two in sync).
const UNSTABLE_ENDE_PROPS: &[&str] = &[
  "DatagramConn",
  "UnixConnectOptions",
  "UnixListenOptions",
  "listen",
  "listenDatagram",
  "connectQuic",
  "listenQuic",
  "QuicBidirectionalStream",
  "QuicConn",
  "QuicListener",
  "QuicReceiveStream",
  "QuicSendStream",
];

const UNSTABLE_SUGGESTION: &str = "If not, try changing the 'lib' compiler \
  option to include 'ende.unstable' or add a triple-slash directive to the \
  top of your entrypoint (main file): /// <reference lib=\"ende.unstable\" />";

static CANNOT_FIND_NAME_RE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"^(Cannot find name '[^']+'\.)").unwrap());

static UNSTABLE_PROPERTY_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(
    r"Property '([^']+)' does not exist on type 'typeof (?:Ende|Deno)'",
  )
  .unwrap()
});

static DID_YOU_MEAN_RE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r" Did you mean '([^']+)'\?").unwrap());

static ARBITRARY_EXTENSIONS_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(
    r"^(Module '[^']*' was resolved to '[^']*'), but '--allowArbitraryExtensions' is not set\.",
  )
  .unwrap()
});

/// Rewrites tsc's message for `code` the way the TS 6 checker does
/// (`formatMessage` and the localized messages in cli/tsc/97_ts_host.js):
/// Ende hints instead of npm ones.
fn format_message(message: String, code: u64) -> String {
  match code {
    // "Do you need to install type definitions for node/jQuery?": Ende has no
    // npm.
    2580 | 2581 | 2591 | 2592 => CANNOT_FIND_NAME_RE
      .captures(&message)
      .map(|c| c[1].to_string())
      .unwrap_or(message),
    6263 => ARBITRARY_EXTENSIONS_RE
      .replace(
        &message,
        "$1, but importing these modules is not supported.",
      )
      .into_owned(),
    2304
      if message == "Cannot find name 'Ende'."
        || message == "Cannot find name 'Deno'." =>
    {
      format!(
        "{message} Do you need to change your target library? Try changing \
         the 'lib' compiler option to include 'ende.ns' or add a \
         triple-slash directive to the top of your entrypoint (main file): \
         /// <reference lib=\"ende.ns\" />"
      )
    }
    _ => {
      let Some(property) = UNSTABLE_PROPERTY_RE
        .captures(&message)
        .map(|c| c[1].to_string())
        .filter(|p| UNSTABLE_ENDE_PROPS.contains(&p.as_str()))
      else {
        return message;
      };
      if code == 2339 {
        return format!(
          "{message} 'Ende.{property}' is an unstable API. \
           {UNSTABLE_SUGGESTION}"
        );
      }
      match DID_YOU_MEAN_RE.captures(&message).map(|c| c[1].to_string()) {
        Some(suggestion) => format!(
          "{message} 'Ende.{property}' is an unstable API. Did you mean \
           '{suggestion}'? {UNSTABLE_SUGGESTION}"
        ),
        None => message,
      }
    }
  }
}

static LIB_NOT_FOUND_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"^Cannot find lib definition for '([^']+)'").unwrap()
});

/// Whether `d` is tsc not finding an Ende lib that a
/// `/// <reference lib="..." />` names. The lib is a root file instead.
fn is_provided_lib_not_found(d: &Diagnostic) -> bool {
  matches!(d.code, 2726 | 2727)
    && d
      .message_text
      .as_deref()
      .and_then(|m| LIB_NOT_FOUND_RE.captures(m))
      .is_some_and(|c| ende_lib_name(&c[1].to_ascii_lowercase()).is_some())
}

/// Maps diagnostics from tsc's paths and positions back to the modules.
struct Mapper<'a> {
  /// tsc's working directory, which relative paths it prints are against.
  cwd: String,
  files: &'a HashMap<String, MappedFile>,
  types_dir: String,
  tsc_dir: String,
  tsconfig: String,
  /// Keys of `files` by their canonical path, for paths tsc reports after
  /// resolving symlinks.
  canonical_files: OnceCell<HashMap<String, String>>,
  /// Lines of files tsc found that aren't in `files`.
  read_files: std::cell::RefCell<HashMap<String, Option<Vec<String>>>>,
}

impl Mapper<'_> {
  /// `None` for a diagnostic in a lib file: TS 6 doesn't report those for
  /// `ende check` either, and Ende's libs predate TS 7's stock libs. Related
  /// information in one is kept, named like TS 6 names lib files.
  fn map(&self, raw: RawDiagnostic) -> Option<Diagnostic> {
    let mut diagnostic = new_diagnostic(raw.category, raw.code, raw.message);
    match self.place(raw.location, raw.end) {
      Place::Lib(_) => return None,
      Place::Module(located) => located.apply(&mut diagnostic),
      Place::None => {}
    }
    let related = raw
      .related
      .into_iter()
      .map(|r| {
        let mut related = new_diagnostic(r.category, r.code, r.message);
        if let Place::Lib(located) | Place::Module(located) =
          self.place(r.location, r.end)
        {
          located.apply(&mut related);
        }
        related
      })
      .collect::<Vec<_>>();
    if !related.is_empty() {
      diagnostic.related_information = Some(related);
    }
    Some(diagnostic)
  }

  /// Where a location tsc printed (1-based) is, 0-based.
  fn place(
    &self,
    location: Option<(String, u64, u64)>,
    end: Option<(u64, u64)>,
  ) -> Place {
    let Some((file, line, column)) = location else {
      return Place::None;
    };
    let key = reported_path_key(&self.cwd, &file);
    if key == self.tsconfig {
      // An option error: the generated config means nothing to the user.
      return Place::None;
    }
    let start = (line.saturating_sub(1), column.saturating_sub(1));
    let end = end.map(|(l, c)| (l.saturating_sub(1), c.saturating_sub(1)));
    let position = |(line, character), shifts: Option<&LineShifts>| Position {
      line,
      character: shifts
        .map(|s| s.to_original(line, character))
        .unwrap_or(character),
    };
    if is_in_dir(&key, &self.types_dir) || is_in_dir(&key, &self.tsc_dir) {
      let name = key.rsplit('/').next().unwrap_or_default();
      return Place::Lib(Located {
        specifier: format!("asset:///{name}"),
        start: position(start, None),
        end: end.map(|end| position(end, None)),
        source_line: self.read_line(&key, start.0),
      });
    }
    Place::Module(match self.lookup(&key) {
      Some(mapped) => Located {
        specifier: mapped.specifier.clone(),
        start: position(start, Some(&mapped.shifts)),
        end: end.map(|end| position(end, Some(&mapped.shifts))),
        source_line: mapped.text.as_deref().and_then(|t| nth_line(t, start.0)),
      },
      None => Located {
        specifier: deno_path_util::url_from_file_path(Path::new(&key))
          .map(|u| u.to_string())
          .unwrap_or(file),
        start: position(start, None),
        end: end.map(|end| position(end, None)),
        source_line: self.read_line(&key, start.0),
      },
    })
  }

  fn lookup(&self, key: &str) -> Option<&MappedFile> {
    if let Some(file) = self.files.get(key) {
      return Some(file);
    }
    let canonical = canonical_key(key)?;
    let canonical_files = self.canonical_files.get_or_init(|| {
      self
        .files
        .keys()
        .filter_map(|k| Some((canonical_key(k)?, k.clone())))
        .collect()
    });
    canonical_files
      .get(&canonical)
      .and_then(|k| self.files.get(k))
  }

  fn read_line(&self, key: &str, line: u64) -> Option<String> {
    let mut read_files = self.read_files.borrow_mut();
    let lines = read_files.entry(key.to_string()).or_insert_with(|| {
      std::fs::read_to_string(key)
        .ok()
        .map(|text| text.lines().map(str::to_string).collect())
    });
    lines.as_ref()?.get(line as usize).cloned()
  }
}

enum Place {
  /// No location, or one in the generated config.
  None,
  /// A lib file (tsc's own, or in `types/`).
  Lib(Located),
  Module(Located),
}

struct Located {
  specifier: String,
  start: Position,
  end: Option<Position>,
  source_line: Option<String>,
}

impl Located {
  fn apply(self, diagnostic: &mut Diagnostic) {
    diagnostic.file_name = Some(self.specifier);
    diagnostic.start = Some(self.start);
    diagnostic.end = self.end;
    diagnostic.source_line = self.source_line;
  }
}

fn new_diagnostic(
  category: Category,
  code: u64,
  message: String,
) -> Diagnostic {
  Diagnostic {
    category: match category {
      Category::Error => DiagnosticCategory::Error,
      Category::Warning => DiagnosticCategory::Warning,
      Category::Suggestion => DiagnosticCategory::Suggestion,
      Category::Message => DiagnosticCategory::Message,
    },
    code,
    start: None,
    end: None,
    original_source_start: None,
    message_text: Some(message),
    message_chain: None,
    source: None,
    source_line: None,
    file_name: None,
    related_information: None,
    reports_deprecated: None,
    reports_unnecessary: None,
    other: Default::default(),
    missing_specifier: None,
  }
}

fn canonical_key(key: &str) -> Option<String> {
  let path = crate::util::fs::canonicalize_path(Path::new(key)).ok()?;
  Some(path_key(&to_tsc_path(&path)))
}

fn is_in_dir(key: &str, dir: &str) -> bool {
  !dir.is_empty()
    && key
      .strip_prefix(dir)
      .is_some_and(|rest| rest.starts_with('/'))
}

fn nth_line(text: &str, line: u64) -> Option<String> {
  text
    .split('\n')
    .nth(line as usize)
    .map(|l| l.trim_end_matches('\r').to_string())
}

#[cfg(test)]
mod tests {
  use super::*;

  fn diagnostic(code: u64, file: &str, line: u64, message: &str) -> Diagnostic {
    Diagnostic {
      category: DiagnosticCategory::Error,
      code,
      start: Some(Position { line, character: 0 }),
      end: None,
      original_source_start: None,
      message_text: Some(message.to_string()),
      message_chain: None,
      source: None,
      source_line: None,
      file_name: Some(file.to_string()),
      related_information: None,
      reports_deprecated: None,
      reports_unnecessary: None,
      other: Default::default(),
      missing_specifier: None,
    }
  }

  fn mapper<'a>(files: &'a HashMap<String, MappedFile>) -> Mapper<'a> {
    Mapper {
      cwd: "C:/Users/me/AppData/Local/ende/tsc7_projects/abc".into(),
      files,
      types_dir: "c:/users/me/appdata/local/ende/tsc7_projects/abc/types"
        .into(),
      tsc_dir: "c:/users/me/appdata/local/ende/tsc/7.0.2/x".into(),
      tsconfig:
        "c:/users/me/appdata/local/ende/tsc7_projects/abc/tsconfig.1.json"
          .into(),
      canonical_files: OnceCell::new(),
      read_files: Default::default(),
    }
  }

  fn raw(file: Option<&str>, line: u64, column: u64) -> RawDiagnostic {
    RawDiagnostic {
      category: Category::Error,
      code: 2322,
      location: file.map(|f| (f.to_string(), line, column)),
      end: Some((line, column + 1)),
      message: "Type 'number' is not assignable to type 'string'.".into(),
      related: Vec::new(),
    }
  }

  #[test]
  fn maps_windows_paths_back() {
    let text = "import { a } from \"./a.ts\"; const x: string = a;\n";
    let mut files = HashMap::new();
    files.insert(
      "c:/proj/main.ts".to_string(),
      MappedFile {
        specifier: "file:///C:/proj/main.ts".into(),
        text: Some(text.into()),
        shifts: Default::default(),
      },
    );
    let mapper = mapper(&files);
    let d = mapper
      .map(raw(Some("../../../../../../../../proj/Main.ts"), 1, 35))
      .unwrap();
    assert_eq!(d.file_name.as_deref(), Some("file:///C:/proj/main.ts"));
    assert_eq!(
      d.start,
      Some(Position {
        line: 0,
        character: 34
      })
    );
    assert_eq!(
      d.end,
      Some(Position {
        line: 0,
        character: 35
      })
    );
    assert_eq!(d.source_line.as_deref(), Some(text.trim_end()));

    // Lib files are dropped; tsconfig errors lose their location.
    let lib =
      "C:\\Users\\me\\AppData\\Local\\ende\\tsc\\7.0.2\\x\\lib.dom.d.ts";
    assert!(mapper.map(raw(Some(lib), 1, 1)).is_none());
    assert!(
      mapper
        .map(raw(Some("types/lib.ende.ns.d.ts"), 1, 1))
        .is_none()
    );
    let d = mapper.map(raw(Some("tsconfig.1.json"), 1, 1)).unwrap();
    assert_eq!((d.file_name, d.start), (None, None));
  }

  #[test]
  fn maps_ends_and_related_information() {
    let text = "import { a } from \"./a.ts\"; const x: string = a;\n";
    let mut files = HashMap::new();
    files.insert(
      "c:/users/me/appdata/local/ende/tsc7_projects/abc/local/1/main.ts"
        .to_string(),
      MappedFile {
        specifier: "file:///C:/proj/main.ts".into(),
        text: Some(text.into()),
        shifts: Default::default(),
      },
    );
    let mapper = mapper(&files);
    let mut d = raw(Some("local/1/main.ts"), 1, 35);
    d.end = Some((1, 36));
    d.related = vec![
      RawDiagnostic {
        category: Category::Message,
        code: 0,
        location: Some(("local/1/main.ts".into(), 1, 10)),
        end: Some((1, 11)),
        message: "'a' is declared here.".into(),
        related: Vec::new(),
      },
      RawDiagnostic {
        category: Category::Message,
        code: 0,
        location: Some((
          "C:/Users/me/AppData/Local/ende/tsc/7.0.2/x/lib.es5.d.ts".into(),
          742,
          20,
        )),
        end: Some((742, 29)),
        message: "An argument for 'y' was not provided.".into(),
        related: Vec::new(),
      },
    ];
    let d = mapper.map(d).unwrap();
    let position = |line, character| Some(Position { line, character });
    assert_eq!((d.start, d.end), (position(0, 34), position(0, 35)));
    let related = d.related_information.unwrap();
    assert_eq!(
      related[0].file_name.as_deref(),
      Some("file:///C:/proj/main.ts")
    );
    assert_eq!(
      (related[0].start.clone(), related[0].end.clone()),
      (position(0, 9), position(0, 10))
    );
    assert_eq!(related[0].category, DiagnosticCategory::Message);
    assert_eq!(
      related[1].file_name.as_deref(),
      Some("asset:///lib.es5.d.ts")
    );
    assert_eq!(related[1].start, position(741, 19));
  }

  #[test]
  fn check_hashes_depend_on_the_tsc() {
    let root = Url::parse("file:///project/").unwrap();
    let dir = Path::new("/ende_dir");
    let a = Tsc7Env::new("/a/tsc".into(), dir, &root);
    let b = Tsc7Env::new("/b/tsc".into(), dir, &root);
    let hash = CacheDBHash::new(1);
    assert_ne!(a.check_hash(hash), hash);
    assert_ne!(a.check_hash(hash), b.check_hash(hash));
    assert_eq!(
      a.check_hash(hash),
      Tsc7Env::new("/a/tsc".into(), dir, &root).check_hash(hash)
    );
  }

  #[test]
  fn formats_messages_like_ts6() {
    let format = |code, message: &str| format_message(message.into(), code);
    assert_eq!(
      format(
        2591,
        "Cannot find name 'process'. Do you need to install type definitions \
         for node? Try `npm i --save-dev @types/node`."
      ),
      "Cannot find name 'process'."
    );
    assert_eq!(
      format(
        6263,
        "Module './a.txt' was resolved to '/a.txt', but \
         '--allowArbitraryExtensions' is not set."
      ),
      "Module './a.txt' was resolved to '/a.txt', but importing these \
       modules is not supported."
    );
    assert!(
      format(2304, "Cannot find name 'Deno'.")
        .ends_with("/// <reference lib=\"ende.ns\" />")
    );
    assert_eq!(
      format(2304, "Cannot find name 'x'."),
      "Cannot find name 'x'."
    );
    let unstable =
      "Property 'listenDatagram' does not exist on type 'typeof Ende'.";
    assert_eq!(
      format(2339, unstable),
      format!(
        "{unstable} 'Ende.listenDatagram' is an unstable API. \
         {UNSTABLE_SUGGESTION}"
      )
    );
    let stable = "Property 'readFile' does not exist on type 'typeof Ende'.";
    assert_eq!(format(2339, stable), stable);
    let did_you_mean = "Property 'listen' does not exist on type 'typeof \
                        Deno'. Did you mean 'listenTls'?";
    assert!(
      format(2551, did_you_mean)
        .contains("is an unstable API. Did you mean 'listenTls'? If not,")
    );
  }

  #[test]
  fn unstable_props_match_the_ts6_host() {
    let host = include_str!("../97_ts_host.js");
    let start = host.find("const unstableEndeProps = new Set([").unwrap();
    let list = &host[start..start + host[start..].find("]);").unwrap()];
    let names = Regex::new(r#""([^"]+)""#)
      .unwrap()
      .captures_iter(list)
      .map(|c| c[1].to_string())
      .collect::<Vec<_>>();
    assert_eq!(names, UNSTABLE_ENDE_PROPS);
  }

  #[test]
  fn recognizes_provided_libs() {
    let not_found = |name: &str| {
      diagnostic(
        2726,
        "file:///a.ts",
        0,
        &format!("Cannot find lib definition for '{name}'."),
      )
    };
    assert!(is_provided_lib_not_found(&not_found("ende.unstable")));
    assert!(is_provided_lib_not_found(&not_found("Deno.NS")));
    assert!(!is_provided_lib_not_found(&not_found("ende.nope")));
    assert!(!is_provided_lib_not_found(&not_found("dom2")));
  }

  #[test]
  fn rewrites_ende_lib_references() {
    let text = "/// <reference no-default-lib=\"true\" />\n\
                /// <reference lib=\"ende.ns\" />\n\
                /// <reference lib=\"esnext\" />\n";
    assert_eq!(
      ENDE_LIB_REFERENCE_RE
        .replace_all(text, "${1}path=\"./lib.ende.${2}.d.ts\""),
      "/// <reference no-default-lib=\"true\" />\n\
       /// <reference path=\"./lib.ende.ns.d.ts\" />\n\
       /// <reference lib=\"esnext\" />\n"
    );
  }
}
