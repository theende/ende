// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use color_print::cstr;
use deno_graph::GraphKind;
use deno_path_util::normalize_path;
use deno_path_util::resolve_url_or_path;
use deno_path_util::url_to_file_path;
use ende_cli_parser::CliError;
pub use ende_cli_parser::flags::*;
use ende_config::glob::FilePatterns;
use ende_config::glob::PathOrPatternSet;
use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_lib::version::ENDE_VERSION_INFO;
pub use ende_runtime::ende_inspector_server::InspectPublishUid;
use ende_telemetry::OtelConfig;
use ende_telemetry::OtelConsoleConfig;
use ende_telemetry::OtelPropagators;

use crate::util::env::resolve_cwd;
use crate::util::fs::canonicalize_path;

// ---------------------------------------------------------------------------
// CLI-only helpers. These stay in the CLI because they depend on deno_graph /
// ende_telemetry / the cli build env, which the parser crate intentionally
// does not.
// ---------------------------------------------------------------------------

pub static UPGRADE_USAGE: &str = cstr!(
  "<g>Latest</>
  <bold>ende upgrade</>

<g>Specific version</>
  <bold>ende upgrade</> <p(245)>2026.10.14</>

<g>From a pull request</> <p(245)>(requires gh CLI)</>
  <bold>ende upgrade</> <p(245)>pr 12345</>"
);

fn join_paths(allowlist: &[String], d: &str) -> String {
  allowlist
    .iter()
    .map(|path| path.to_string())
    .collect::<Vec<String>>()
    .join(d)
}

/// Resolve a subcommand's include/ignore globs against `base`.
pub fn resolve_file_patterns(
  files: &FileFlags,
  base: &Path,
) -> Result<FilePatterns, AnyError> {
  Ok(FilePatterns {
    include: if files.include.is_empty() {
      None
    } else {
      Some(PathOrPatternSet::from_include_relative_path_or_patterns(
        base,
        &files.include,
      )?)
    },
    exclude: PathOrPatternSet::from_exclude_relative_path_or_patterns(
      base,
      &files.ignore,
    )?,
    base: base.to_path_buf(),
  })
}

/// The `--target` triple for `ende compile`, defaulting to the triple this
/// binary was built for.
pub fn resolve_compile_target(flags: &CompileFlags) -> String {
  flags
    .target
    .clone()
    .unwrap_or_else(|| env!("TARGET").to_string())
}

/// The module `GraphKind` that should be created for a `TypeCheckMode`.
pub fn graph_kind(mode: TypeCheckMode) -> GraphKind {
  match mode.is_true() {
    true => GraphKind::All,
    false => GraphKind::CodeOnly,
  }
}

pub trait FlagsExt {
  fn to_permission_args(&self) -> Vec<String>;
  fn no_legacy_abort(&self) -> bool;
  fn otel_config(&self) -> OtelConfig;
  fn config_path_args(&self, current_dir: &Path) -> Option<Vec<PathBuf>>;
  fn resolve_watch_exclude_set(&self) -> Result<PathOrPatternSet, AnyError>;
}

impl FlagsExt for Flags {
  /// Return list of permission arguments that are equivalent
  /// to the ones used to create `self`.
  fn to_permission_args(&self) -> Vec<String> {
    let mut args = vec![];

    if self.permissions.allow_all {
      args.push("--allow-all".to_string());
      return args;
    }

    match &self.permissions.allow_read {
      Some(read_allowlist) if read_allowlist.is_empty() => {
        args.push("--allow-read".to_string());
      }
      Some(read_allowlist) => {
        let s = format!("--allow-read={}", join_paths(read_allowlist, ","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.deny_read {
      Some(read_denylist) if read_denylist.is_empty() => {
        args.push("--deny-read".to_string());
      }
      Some(read_denylist) => {
        let s = format!("--deny-read={}", join_paths(read_denylist, ","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.allow_write {
      Some(write_allowlist) if write_allowlist.is_empty() => {
        args.push("--allow-write".to_string());
      }
      Some(write_allowlist) => {
        let s = format!("--allow-write={}", join_paths(write_allowlist, ","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.deny_write {
      Some(write_denylist) if write_denylist.is_empty() => {
        args.push("--deny-write".to_string());
      }
      Some(write_denylist) => {
        let s = format!("--deny-write={}", join_paths(write_denylist, ","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.allow_net {
      Some(net_allowlist) if net_allowlist.is_empty() => {
        args.push("--allow-net".to_string());
      }
      Some(net_allowlist) => {
        let s = format!("--allow-net={}", net_allowlist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.deny_net {
      Some(net_denylist) if net_denylist.is_empty() => {
        args.push("--deny-net".to_string());
      }
      Some(net_denylist) => {
        let s = format!("--deny-net={}", net_denylist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.unsafely_ignore_certificate_errors {
      Some(ic_allowlist) if ic_allowlist.is_empty() => {
        args.push("--unsafely-ignore-certificate-errors".to_string());
      }
      Some(ic_allowlist) => {
        let s = format!(
          "--unsafely-ignore-certificate-errors={}",
          ic_allowlist.join(",")
        );
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.allow_env {
      Some(env_allowlist) if env_allowlist.is_empty() => {
        args.push("--allow-env".to_string());
      }
      Some(env_allowlist) => {
        let s = format!("--allow-env={}", env_allowlist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.deny_env {
      Some(env_denylist) if env_denylist.is_empty() => {
        args.push("--deny-env".to_string());
      }
      Some(env_denylist) => {
        let s = format!("--deny-env={}", env_denylist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.ignore_env {
      Some(ignorelist) if ignorelist.is_empty() => {
        args.push("--ignore-env".to_string());
      }
      Some(ignorelist) => {
        let s = format!("--ignore-env={}", ignorelist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.ignore_read {
      Some(ignorelist) if ignorelist.is_empty() => {
        args.push("--ignore-read".to_string());
      }
      Some(ignorelist) => {
        let s = format!("--ignore-read={}", ignorelist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.allow_run {
      Some(run_allowlist) if run_allowlist.is_empty() => {
        args.push("--allow-run".to_string());
      }
      Some(run_allowlist) => {
        let s = format!("--allow-run={}", run_allowlist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.deny_run {
      Some(run_denylist) if run_denylist.is_empty() => {
        args.push("--deny-run".to_string());
      }
      Some(run_denylist) => {
        let s = format!("--deny-run={}", run_denylist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.allow_sys {
      Some(sys_allowlist) if sys_allowlist.is_empty() => {
        args.push("--allow-sys".to_string());
      }
      Some(sys_allowlist) => {
        let s = format!("--allow-sys={}", sys_allowlist.join(","));
        args.push(s)
      }
      _ => {}
    }

    match &self.permissions.deny_sys {
      Some(sys_denylist) if sys_denylist.is_empty() => {
        args.push("--deny-sys".to_string());
      }
      Some(sys_denylist) => {
        let s = format!("--deny-sys={}", sys_denylist.join(","));
        args.push(s)
      }
      _ => {}
    }

    match &self.permissions.allow_ffi {
      Some(ffi_allowlist) if ffi_allowlist.is_empty() => {
        args.push("--allow-ffi".to_string());
      }
      Some(ffi_allowlist) => {
        let s = format!("--allow-ffi={}", join_paths(ffi_allowlist, ","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.deny_ffi {
      Some(ffi_denylist) if ffi_denylist.is_empty() => {
        args.push("--deny-ffi".to_string());
      }
      Some(ffi_denylist) => {
        let s = format!("--deny-ffi={}", join_paths(ffi_denylist, ","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.allow_import {
      Some(allowlist) if allowlist.is_empty() => {
        args.push("--allow-import".to_string());
      }
      Some(allowlist) => {
        let s = format!("--allow-import={}", allowlist.join(","));
        args.push(s);
      }
      _ => {}
    }

    match &self.permissions.deny_import {
      Some(denylist) if denylist.is_empty() => {
        args.push("--deny-import".to_string());
      }
      Some(denylist) => {
        let s = format!("--deny-import={}", denylist.join(","));
        args.push(s);
      }
      _ => {}
    }

    args
  }

  fn no_legacy_abort(&self) -> bool {
    self
      .unstable_config
      .features
      .contains(&String::from("no-legacy-abort"))
  }

  fn otel_config(&self) -> OtelConfig {
    // `OTEL_ENDE*` names fall back to the deprecated `OTEL_DENO*` names.
    let read_var = |name: &str| {
      if name.starts_with("OTEL_ENDE") {
        ende_env::var(name)
      } else {
        std::env::var(name)
      }
    };
    let otel_var = |name: &str| match read_var(name) {
      Ok(s) if s.eq_ignore_ascii_case("true") => Some(true),
      Ok(s) if s.eq_ignore_ascii_case("false") => Some(false),
      Ok(_) => {
        log::warn!(
          "'{name}' env var value not recognized, only 'true' and 'false' are accepted"
        );
        None
      }
      Err(_) => None,
    };

    let disabled = otel_var("OTEL_SDK_DISABLED").unwrap_or(false);
    let default = !disabled && otel_var("OTEL_ENDE").unwrap_or(false);

    let propagators = if default {
      if let Ok(propagators) = std::env::var("OTEL_PROPAGATORS") {
        propagators
          .split(',')
          .filter_map(|p| match p.trim() {
            "tracecontext" => Some(OtelPropagators::TraceContext),
            "baggage" => Some(OtelPropagators::Baggage),
            _ => None,
          })
          .collect()
      } else {
        HashSet::from([OtelPropagators::TraceContext, OtelPropagators::Baggage])
      }
    } else {
      HashSet::default()
    };

    OtelConfig {
      tracing_enabled: !disabled
        && otel_var("OTEL_ENDE_TRACING").unwrap_or(default),
      metrics_enabled: !disabled
        && otel_var("OTEL_ENDE_METRICS").unwrap_or(default),
      propagators,
      console: match read_var("OTEL_ENDE_CONSOLE").as_deref() {
        Ok(_) if disabled => OtelConsoleConfig::Ignore,
        Ok("ignore") => OtelConsoleConfig::Ignore,
        Ok("capture") => OtelConsoleConfig::Capture,
        Ok("replace") => OtelConsoleConfig::Replace,
        res => {
          if res.is_ok() {
            log::warn!("'OTEL_ENDE_CONSOLE' env var value not recognized, only 'ignore', 'capture', or 'replace' are accepted");
          }
          if default {
            OtelConsoleConfig::Capture
          } else {
            OtelConsoleConfig::Ignore
          }
        }
      },
      deterministic_prefix: ende_env::var("ENDE_UNSTABLE_OTEL_DETERMINISTIC")
        .as_deref()
        .map(u8::from_str)
        .map(|x| match x {
          Ok(x) => Some(x),
          Err(_) => {
            log::warn!("'ENDE_UNSTABLE_OTEL_DETERMINISTIC' env var value not recognized, only integers are accepted");
            None
          }
        })
        .ok()
        .flatten(),
    }
  }

  /// Extract the paths the config file should be discovered from.
  ///
  /// Returns `None` if the config file should not be auto-discovered.
  fn config_path_args(&self, current_dir: &Path) -> Option<Vec<PathBuf>> {
    fn resolve_multiple_files(
      files_or_dirs: &[String],
      current_dir: &Path,
    ) -> Vec<PathBuf> {
      let mut seen = HashSet::with_capacity(files_or_dirs.len());
      let result = files_or_dirs
        .iter()
        .filter_map(|p| {
          let path = normalize_path(Cow::Owned(current_dir.join(p)));
          if seen.insert(path.clone()) {
            Some(path.into_owned())
          } else {
            None
          }
        })
        .collect::<Vec<_>>();
      if result.is_empty() {
        vec![current_dir.to_path_buf()]
      } else {
        result
      }
    }

    fn resolve_single_folder_path(
      arg: &str,
      current_dir: &Path,
      maybe_resolve_directory: impl FnOnce(PathBuf) -> Option<PathBuf>,
    ) -> Option<PathBuf> {
      if let Ok(module_specifier) = resolve_url_or_path(arg, current_dir) {
        if module_specifier.scheme() == "file"
          || module_specifier.scheme() == "npm"
          || module_specifier.scheme() == "jsr"
        {
          if let Ok(p) = url_to_file_path(&module_specifier) {
            maybe_resolve_directory(p)
          } else {
            Some(current_dir.to_path_buf())
          }
        } else {
          // When the entrypoint is a remote script (e.g. https:), then we
          // don't auto discover the config file. Package entrypoints (npm:,
          // jsr:) resolve within the project context, so they do.
          None
        }
      } else {
        Some(current_dir.to_path_buf())
      }
    }

    use EndeSubcommand::*;
    match &self.subcommand {
      Fmt(FmtFlags { files, .. }) => {
        Some(resolve_multiple_files(&files.include, current_dir))
      }
      Lint(LintFlags { files, .. }) => {
        Some(resolve_multiple_files(&files.include, current_dir))
      }
      Run(RunFlags { script, .. })
      | Compile(CompileFlags {
        source_file: script,
        ..
      }) => resolve_single_folder_path(script, current_dir, |mut p| {
        if p.pop() { Some(p) } else { None }
      })
      .map(|p| vec![p]),
      Desktop(DesktopFlags {
        source_file: script,
        ..
      }) => resolve_single_folder_path(script, current_dir, |p| {
        // The desktop entrypoint is a directory for bare `ende desktop` (which
        // defaults to `.`), in which case the config lives in that directory
        // itself — don't pop up to its parent. Only pop when the entrypoint is
        // a file.
        if p.is_dir() {
          Some(p)
        } else {
          p.parent().map(|parent| parent.to_path_buf())
        }
      })
      .map(|p| vec![p]),
      Task(TaskFlags {
        cwd: Some(path), ..
      }) => {
        // todo(dsherret): Why is this canonicalized? Document why.
        // attempt to resolve the config file from the task subcommand's
        // `--cwd` when specified
        match canonicalize_path(Path::new(path)) {
          Ok(path) => Some(vec![path]),
          Err(_) => Some(vec![current_dir.to_path_buf()]),
        }
      }
      Cache(CacheFlags { files, .. })
      | Install(InstallFlags::Local(InstallFlagsLocal::Entrypoints(
        InstallEntrypointsFlags {
          entrypoints: files, ..
        },
      ))) => Some(vec![
        files
          .iter()
          .filter_map(|file| {
            resolve_single_folder_path(file, current_dir, |mut p| {
              if p.is_dir() {
                return Some(p);
              }
              if p.pop() { Some(p) } else { None }
            })
          })
          .next()
          .unwrap_or_else(|| current_dir.to_path_buf()),
      ]),
      _ => Some(vec![current_dir.to_path_buf()]),
    }
  }

  fn resolve_watch_exclude_set(&self) -> Result<PathOrPatternSet, AnyError> {
    match &self.watch {
      Some(WatchFlagsWithPaths {
        exclude: excluded_paths,
        ..
      }) => {
        let cwd = resolve_cwd(self.initial_cwd.as_deref())?;
        PathOrPatternSet::from_exclude_relative_path_or_patterns(
          &cwd,
          excluded_paths,
        )
        .context("Failed resolving watch exclude patterns.")
      }
      _ => Ok(PathOrPatternSet::default()),
    }
  }
}

pub fn flags_from_vec(args: Vec<OsString>) -> Result<Flags, CliError> {
  flags_from_vec_with_initial_cwd(args, None)
}

/// Strip a single trailing carriage return from an argument.
///
/// When a script with CRLF line endings starts with a shebang like
/// `#!/usr/bin/env -S ende run --allow-net`, the kernel passes the trailing
/// `\r` as part of the final argument (e.g. `--allow-net\r`), which then
/// fails clap parsing with a confusing "isn't valid in this context" error.
/// Trimming a trailing CR here makes such scripts run as the author intended.
fn strip_trailing_cr(arg: OsString) -> OsString {
  let bytes = arg.as_encoded_bytes();
  if bytes.ends_with(b"\r") {
    let stripped = bytes[..bytes.len() - 1].to_vec();
    // SAFETY: `\r` is the single ASCII byte 0x0D; removing an ASCII byte from
    // the end of a valid OS-string byte sequence (UTF-8 on Unix, WTF-8 on
    // Windows) cannot split a multi-byte code point and keeps the encoding
    // valid.
    unsafe { OsString::from_encoded_bytes_unchecked(stripped) }
  } else {
    arg
  }
}

/// Fast path for `ende run <file> [script args...]` with no ende-level flags
/// before the file. Building the parse tree / walking the parser costs a couple
/// microseconds; for the overwhelmingly common bare-run case we skip it and
/// build `RunFlags` directly. Any ende flag before the file (anything starting
/// with `-`, or `-` itself) falls through to the full parser. `args` must be
/// already `\r`-stripped and dx-shimmed. Returns `None` when the fast path does
/// not apply. Must stay output-identical to the full parse (see the
/// `fast_path_matches_full_parse` test).
fn bare_run_fast_path(args: &[OsString]) -> Option<Flags> {
  if args.len() >= 3
    && args[1] == "run"
    && args[2] != "-"
    && args[2].as_encoded_bytes().first() != Some(&b'-')
    && let Some(script) = args[2].to_str()
  {
    let argv = args[3..]
      .iter()
      .map(|a| a.to_string_lossy().into_owned())
      .collect::<Vec<_>>();
    let flags = Flags {
      subcommand: EndeSubcommand::Run(RunFlags {
        script: script.to_string(),
        bare: false,
        coverage_dir: None,
        print_task_list: false,
      }),
      argv,
      // Matches the full `run_parse`: no `--no-code-cache` flag before the
      // file resolves code caching to enabled.
      code_cache_enabled: true,
      ..Default::default()
    };
    Some(flags)
  } else {
    None
  }
}

/// Main entry point for parsing ende's command line flags. Routes through the
/// hand-written `ende_cli_parser`; the `Flags` type is shared between this
/// crate and the parser crate (`pub use ende_cli_parser::flags::*`), so no
/// conversion is needed.
pub fn flags_from_vec_with_initial_cwd(
  args: Vec<OsString>,
  initial_cwd: Option<PathBuf>,
) -> Result<Flags, CliError> {
  // Strip a trailing `\r` from each arg so a CRLF-shebang script isn't poisoned
  // by a stray carriage return. The hand-written parser also does this
  // internally; doing it here first keeps the fast path below consistent with
  // the full parse. (`ende_cli_parser::convert::flags_from_vec` re-strips, which
  // is a no-op on already-stripped args.)
  let args: Vec<OsString> = args.into_iter().map(strip_trailing_cr).collect();

  // dx/endex/dnx shim: rewrite the binary name into an explicit `x` subcommand
  // before parsing (the hand-written parser doesn't special-case argv[0]).
  let args = if !args.is_empty()
    && (args[0].as_encoded_bytes().ends_with(b"dx")
      || args[0].as_encoded_bytes().ends_with(b"endex")
      || args[0].as_encoded_bytes().ends_with(b"dnx"))
  {
    let mut new_args = Vec::with_capacity(args.len() + 1);
    new_args.push(args[0].clone());
    new_args.push(OsString::from("x"));
    new_args.extend(args.into_iter().skip(1));
    new_args
  } else {
    args
  };

  // Fast path for the overwhelmingly common `ende run <file> [args...]` with no
  // ende-level flags before the file: build `RunFlags` directly and skip the
  // parser entirely. `bare_run_fast_path` produces output identical to the full
  // parse (asserted by `fast_path_matches_full_parse`).
  if let Some(mut flags) = bare_run_fast_path(&args) {
    flags.initial_cwd = initial_cwd;
    return Ok(flags);
  }

  // The hand-written parser works on `String`s, whereas clap consumed
  // `OsString` natively. Args are almost always UTF-8; the lossy conversion is
  // a deliberate behavior change for the rare non-UTF-8 arg (e.g. a script path
  // with invalid bytes), which becomes mangled (replacement chars) here instead
  // of being carried through verbatim. Accepted as a Deno 3 simplification.
  let string_args: Vec<String> = args
    .iter()
    .map(|a| a.to_string_lossy().into_owned())
    .collect();

  match ende_cli_parser::convert::flags_from_vec(string_args) {
    Ok(mut flags) => {
      // Set (and, for compile/desktop, canonicalize) the initial cwd — the
      // parser crate has no filesystem access, so this stays on the CLI side.
      flags.initial_cwd = match &flags.subcommand {
        EndeSubcommand::Compile(_) | EndeSubcommand::Desktop(_) => {
          initial_cwd.map(|cwd| canonicalize_path(&cwd).ok().unwrap_or(cwd))
        }
        _ => initial_cwd,
      };
      Ok(flags)
    }
    Err(e) => Err(e),
  }
}

/// Render the text printed by `ende --version` (`long`) and `ende -V` (short).
/// Both forms are prefixed with the binary name and terminated by a newline,
/// matching what clap's `render_long_version` / `render_version` used to emit.
pub fn render_version(long: bool) -> String {
  if long {
    debug_assert_eq!(ENDE_VERSION_INFO.typescript, ende_snapshots::TS_VERSION);
    format!(
      "ende {} ({}, {})\nv8 {}\ntypescript {}\n",
      ENDE_VERSION_INFO.ende,
      env!("PROFILE"),
      env!("TARGET"),
      ende_core::v8::VERSION_STRING,
      ENDE_VERSION_INFO.typescript,
    )
  } else {
    format!("ende {}\n", ENDE_VERSION_INFO.ende)
  }
}

// originally copied from clap, https://github.com/clap-rs/clap/blob/4e1a565b8adb4f2ad74a9631565574767fdc37ae/clap_builder/src/parser/features/suggestions.rs#L11-L26
pub fn did_you_mean<T, I>(v: &str, possible_values: I) -> Vec<String>
where
  T: AsRef<str>,
  I: IntoIterator<Item = T>,
{
  let mut candidates: Vec<(f64, String)> = possible_values
    .into_iter()
    // GH #4660: using `jaro` because `jaro_winkler` implementation in `strsim-rs` is wrong
    // causing strings with common prefix >=10 to be considered perfectly similar
    .map(|pv| (strsim::jaro(v, pv.as_ref()), pv.as_ref().to_owned()))
    // Confidence of 0.7 so that bar -> baz is suggested
    .filter(|(confidence, _)| *confidence > 0.8)
    .collect();
  candidates
    .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
  candidates.into_iter().map(|(_, pv)| pv).collect()
}

/// Handle a dynamic-completion callback: the shell invoked us with
/// `COMPLETE=<shell>` set and the words being completed passed after a `--`
/// separator. Candidates are computed from the hand-written command tree via
/// `ende_cli_parser::completions` (no clap), with `ende task` names supplied by
/// [`dynamic_task_completer`].
pub fn handle_shell_completion(_cwd: &Path) -> Result<(), AnyError> {
  let shell = std::env::var("COMPLETE").unwrap_or_default();
  let words = shell_completion_words();
  ende_cli_parser::completions::try_complete(
    &ende_cli_parser::defs::ENDE_ROOT,
    &words,
    &shell,
    &dynamic_task_completer,
  );
  Ok(())
}

/// Emit the dynamic shell-completion registration script for `shell`
/// (bash/fish/zsh). The parser records only the target shell
/// (`CompletionsFlags::Dynamic { shell }`) and defers this CLI-only generation
/// here. The emitted script calls back into this binary with `COMPLETE` set,
/// where [`handle_shell_completion`] computes the candidates.
pub fn handle_dynamic_shell_completion(shell: &str) -> Result<(), AnyError> {
  let completer = std::env::args_os()
    .next()
    .map(|s| s.to_string_lossy().into_owned())
    .unwrap_or_else(|| "ende".to_string());
  let completer = shlex::try_quote(&completer)
    .map(|c| c.into_owned())
    .unwrap_or(completer);
  let script =
    ende_cli_parser::completions::generate_dynamic(shell, "ende", &completer);
  ende_print::drop_write_stdout(&script);
  Ok(())
}

/// The words being completed, extracted from the callback invocation. The
/// registration scripts invoke `<ende> -- <typed words...>`, so everything
/// after the first `--` is the command line (starting with the command name).
fn shell_completion_words() -> Vec<String> {
  let mut words = Vec::new();
  let mut seen_sep = false;
  for a in std::env::args_os() {
    if seen_sep {
      words.push(a.to_string_lossy().into_owned());
    } else if a == "--" {
      seen_sep = true;
    }
  }
  words
}

/// Positional-argument completer injected into the parser's completion engine.
/// Completes `ende task <TAB>` with the available task names read from the
/// resolved config (the only dynamic completion Ende offers). Kept CLI-side
/// because the parser crate has no config/workspace access.
fn dynamic_task_completer(
  active_cmd: &ende_cli_parser::CommandDef,
  _current: &str,
) -> Vec<(String, Option<String>)> {
  if active_cmd.name != "task" {
    return Vec::new();
  }
  let Some(flags) = task_flags_for_completion() else {
    return Vec::new();
  };
  match crate::tools::task::get_available_tasks_for_completion(
    std::sync::Arc::new(flags),
  ) {
    Ok(tasks) => tasks
      .into_iter()
      .map(|t| (t.name, t.task.description))
      .collect(),
    Err(e) => {
      log::debug!("Error during available tasks completion: {e}");
      Vec::new()
    }
  }
}

/// Best-effort parse of the completion command line into `Flags`, used to pick
/// up `ende task`'s `--config`/`--recursive`/`--filter` for task resolution.
fn task_flags_for_completion() -> Option<Flags> {
  let words = shell_completion_words();
  // The last word is the partial being completed; if the full line doesn't
  // parse, retry without it (mirrors clap's `ignore_errors`).
  let flags = ende_cli_parser::convert::flags_from_vec(words.clone())
    .or_else(|_| {
      let mut w = words;
      w.pop();
      ende_cli_parser::convert::flags_from_vec(w)
    })
    .ok()?;
  matches!(flags.subcommand, EndeSubcommand::Task(_)).then_some(flags)
}

#[cfg(test)]
mod tests {
  use ende_cli_parser::defs::ENDE_ROOT;
  use pretty_assertions::assert_eq;

  use super::*;

  /// Creates vector of strings, Vec<String>
  macro_rules! svec {
    ($($x:expr),* $(,)?) => (vec![$($x.to_string().into()),*]);
  }

  fn flags_from_vec(args: Vec<OsString>) -> Result<Flags, CliError> {
    flags_from_vec_with_initial_cwd(args, None)
  }

  /// Ende has a single release channel, so the version output doesn't name
  /// one.
  #[test]
  fn render_version_has_no_release_channel() {
    assert_eq!(
      render_version(false),
      format!("ende {}\n", ENDE_VERSION_INFO.ende)
    );
    assert_eq!(
      render_version(true).lines().next().unwrap(),
      format!(
        "ende {} ({}, {})",
        ENDE_VERSION_INFO.ende,
        env!("PROFILE"),
        env!("TARGET")
      )
    );
  }

  /// Invoking the binary through an `x` shim (argv[0] ending in a built-in
  /// shim name) runs `ende x`; other names keep the normal parse.
  #[test]
  fn x_shim_argv0() {
    for argv0 in ["dx", "endex", "dnx", "/usr/local/bin/endex", "./endex"] {
      let flags = flags_from_vec(svec![argv0, "npm:foo"]).unwrap();
      assert!(
        matches!(flags.subcommand, EndeSubcommand::X(_)),
        "{argv0} should run `x`, got {:?}",
        flags.subcommand
      );
    }
    for argv0 in ["ende", "/usr/local/bin/ende", "denox", "index", "endexe"] {
      let flags = flags_from_vec(svec![argv0, "run", "main.ts"]).unwrap();
      assert!(
        matches!(flags.subcommand, EndeSubcommand::Run(_)),
        "{argv0} should not run `x`, got {:?}",
        flags.subcommand
      );
    }
  }

  /// The bare-run fast path in `flags_from_vec_with_initial_cwd` bypasses the
  /// parser and builds `RunFlags` by hand, so it must produce output identical
  /// to running the args through the full hand-written parser. Assert that for
  /// every case where the fast path fires, and assert it correctly declines
  /// (returns `None`) when any ende-level flag precedes the script.
  #[test]
  fn fast_path_matches_full_parse() {
    // Cases where the fast path SHOULD fire; its output must equal the full
    // parse (neither sets `initial_cwd`, so both default to `None`).
    let fires: &[&[&str]] = &[
      &["ende", "run", "script.ts"],
      &["ende", "run", "./path/to/mod.ts"],
      &["ende", "run", "script.ts", "a", "b"],
      &["ende", "run", "script.ts", "--", "--foo", "-x"],
      &["ende", "run", "https://example.com/mod.ts", "arg"],
    ];
    for case in fires {
      let os_args: Vec<OsString> = case.iter().map(OsString::from).collect();
      let fast = bare_run_fast_path(&os_args).unwrap_or_else(|| {
        panic!("fast path should fire for {case:?}");
      });
      let string_args: Vec<String> =
        case.iter().map(|s| s.to_string()).collect();
      let full = ende_cli_parser::convert::flags_from_vec(string_args)
        .unwrap_or_else(|e| panic!("full parse failed for {case:?}: {e:?}"));
      assert_eq!(
        fast, full,
        "fast path diverged from full parse for {case:?}"
      );
    }

    // Cases where the fast path must decline and fall through to the parser.
    let declines: &[&[&str]] = &[
      &["ende", "run"],                    // no script
      &["ende", "run", "-"],               // stdin
      &["ende", "run", "-A", "script.ts"], // flag before script
      &["ende", "run", "--watch", "s.ts"], // flag before script
      &["ende", "test", "script.ts"],      // not `run`
    ];
    for case in declines {
      let os_args: Vec<OsString> = case.iter().map(OsString::from).collect();
      assert!(
        bare_run_fast_path(&os_args).is_none(),
        "fast path should decline for {case:?}"
      );
    }
  }

  #[test]
  fn minimum_dependency_age_pm_subcommands() {
    // The package management subcommands that resolve versions must accept the
    // flag so users can act on the "blocked by minimum dependency age" hint.
    let cases = [
      svec!["ende", "add", "--min-dep-age=0", "jsr:@std/path"],
      svec!["ende", "add", "--minimum-dependency-age=0", "jsr:@std/path"],
      svec!["ende", "remove", "--min-dep-age=0", "@std/path"],
    ];
    for args in cases {
      let flags = flags_from_vec(args.clone())
        .unwrap_or_else(|e| panic!("failed to parse {args:?}: {e}"));
      assert_eq!(
        flags.minimum_dependency_age,
        Some(NewestDependencyDate::Disabled),
        "{args:?}"
      );
    }
  }

  #[test]
  fn test_config_path_args() {
    let flags = flags_from_vec(svec!["ende", "run", "foo.js"]).unwrap();
    let cwd = resolve_cwd(None).unwrap().into_owned();

    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.clone()]));

    let flags = flags_from_vec(svec!["ende", "run", "sub_dir/foo.js"]).unwrap();
    let cwd = resolve_cwd(None).unwrap().into_owned();
    assert_eq!(
      flags.config_path_args(&cwd),
      Some(vec![cwd.join("sub_dir").clone()])
    );

    let flags =
      flags_from_vec(svec!["ende", "https://example.com/foo.js"]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), None);

    let flags = flags_from_vec(svec!["ende", "run", "jsr:@scope/foo"]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.clone()]));

    let flags = flags_from_vec(svec!["ende", "run", "npm:@scope/foo"]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.clone()]));

    let flags =
      flags_from_vec(svec!["ende", "lint", "dir/a/a.js", "dir/b/b.js"])
        .unwrap();
    assert_eq!(
      flags.config_path_args(&cwd),
      Some(vec![cwd.join("dir/a/a.js"), cwd.join("dir/b/b.js")])
    );

    let flags = flags_from_vec(svec!["ende", "lint"]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.clone()]));

    let flags = flags_from_vec(svec!["ende", "cache", "sub/test.js"]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.join("sub")]));

    let flags = flags_from_vec(svec!["ende", "cache", "."]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.clone()]));

    let flags =
      flags_from_vec(svec!["ende", "install", "-e", "sub/test.js"]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.join("sub")]));

    let flags = flags_from_vec(svec![
      "ende",
      "fmt",
      "dir/a/a.js",
      "dir/a/a2.js",
      "dir/b.js"
    ])
    .unwrap();
    assert_eq!(
      flags.config_path_args(&cwd),
      Some(vec![
        cwd.join("dir/a/a.js"),
        cwd.join("dir/a/a2.js"),
        cwd.join("dir/b.js")
      ])
    );

    // For `ende desktop .` (and bare `ende desktop`) config discovery must
    // start in the current directory rather than its parent. See issue #35653.
    let flags = flags_from_vec(svec!["ende", "desktop", "."]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.clone()]));

    let flags = flags_from_vec(svec!["ende", "desktop"]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.clone()]));

    // An explicit file entrypoint resolves to its containing directory.
    let flags =
      flags_from_vec(svec!["ende", "desktop", "sub/main.ts"]).unwrap();
    assert_eq!(flags.config_path_args(&cwd), Some(vec![cwd.join("sub")]));
  }
  /// `ende <sub> --help`, `ende <sub> -h` and `ende help <sub>` must all render
  /// the same text for every subcommand.
  #[test]
  fn equal_help_output() {
    for command in ENDE_ROOT.subcommands {
      // `help` has no help of its own.
      if command.name == "help" {
        continue;
      }

      let render =
        |args: Vec<OsString>| match flags_from_vec(args).unwrap().subcommand {
          EndeSubcommand::Help(help) => help.help.to_string(),
          _ => unreachable!("{} did not render help", command.name),
        };

      let long_flag = render(svec!["ende", command.name, "--help"]);
      let short_flag = render(svec!["ende", command.name, "-h"]);
      let subcommand = render(svec!["ende", "help", command.name]);

      assert_eq!(long_flag, short_flag, "{} subcommand", command.name);
      assert_eq!(long_flag, subcommand, "{} subcommand", command.name);
    }
  }

  /// A `value_name` containing a `:` breaks shell completions for zsh, which
  /// uses `:` to separate a completion candidate from its description.
  #[test]
  fn no_colon_in_value_name() {
    fn check(cmd: &'static ende_cli_parser::CommandDef) {
      for arg in cmd.all_args() {
        if let Some(value_name) = arg.value_name {
          assert!(
            !value_name.contains(':'),
            "`{}` arg `{}` has a value_name containing ':' ({value_name}), \
             which breaks zsh completions",
            cmd.name,
            arg.name,
          );
        }
      }
      for sub in cmd.subcommands {
        check(sub);
      }
    }
    check(&ENDE_ROOT);
  }
}
