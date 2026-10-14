// Copyright 2018-2026 the Deno authors. MIT license.

//! Framework detection for `ende compile .` and `ende desktop .`.
//!
//! Detects Fresh projects and generates the appropriate entrypoint and include
//! paths so that `ende compile .` just works.

use std::path::Path;
use std::path::PathBuf;

use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_core::serde_json;

/// Result of framework detection.
#[derive(Debug)]
pub struct FrameworkDetection {
  /// Name of the detected framework (for display).
  pub name: &'static str,
  /// Generated entrypoint TypeScript/JavaScript code (production).
  pub entrypoint_code: String,
  /// Directories to include in the compiled binary.
  pub include_paths: Vec<String>,
  /// Optional build command to run before compilation (e.g. `ende task build`).
  /// The command is run with the detected directory as cwd.
  pub build_command: Option<Vec<String>>,
  /// Command to launch the framework's HMR server for desktop
  pub hmr_command: Option<Vec<String>>,
}

impl FrameworkDetection {
  /// Directories (relative to the project root) where the framework keeps
  /// static assets like favicons. Used to auto-detect a desktop app icon
  /// when the user didn't supply `--icon`.
  pub fn static_asset_dirs(&self) -> &'static [&'static str] {
    &["static"]
  }
}

/// Search a framework's static asset directories for a favicon that can
/// double as the desktop app icon. Returns the first match in priority
/// order, restricted to file formats supported by `target_os` (so we
/// don't pick a `.png` for a Windows build where bundling would silently
/// drop it).
pub fn find_framework_favicon(
  dir: &Path,
  detection: &FrameworkDetection,
  target_os: &str,
) -> Option<PathBuf> {
  let exts: &[&str] = match target_os {
    "macos" => &["icns", "png"],
    "windows" => &["ico"],
    _ => &["png"],
  };
  let names = ["icon", "favicon", "apple-touch-icon", "logo"];
  for sub in detection.static_asset_dirs() {
    let base = dir.join(sub);
    if !base.is_dir() {
      continue;
    }
    for name in names {
      for ext in exts {
        let candidate = base.join(format!("{name}.{ext}"));
        if candidate.is_file() {
          return Some(candidate);
        }
      }
    }
  }
  None
}

/// Run the framework's build step (e.g. `ende task build`) with `dir` as the
/// working directory, if the detection specifies one. This must run before the
/// framework's `include_paths` are bundled, since they point at the build
/// output (e.g. `_fresh`) which doesn't exist until the build runs.
///
/// Shared by `ende compile .` and `ende desktop .` so the two can't drift.
pub fn run_build_command(
  detection: &FrameworkDetection,
  dir: &Path,
) -> Result<(), AnyError> {
  let Some(build_cmd) = &detection.build_command else {
    return Ok(());
  };
  log::info!(
    "{} {} project...",
    deno_terminal::colors::green("Building"),
    detection.name,
  );
  let status = std::process::Command::new(&build_cmd[0])
    .args(&build_cmd[1..])
    .current_dir(dir)
    .status()
    .with_context(|| {
      format!("Failed to run build command: {}", build_cmd.join(" "))
    })?;
  if !status.success() {
    ende_core::anyhow::bail!(
      "{} build failed (exit code: {})",
      detection.name,
      status.code().unwrap_or(-1)
    );
  }
  Ok(())
}

/// Detect a web framework in the given directory.
pub fn detect_framework(
  dir: &Path,
) -> Result<Option<FrameworkDetection>, AnyError> {
  // Fresh: fresh.gen.ts or _fresh/
  if dir.join("fresh.gen.ts").exists() || dir.join("_fresh").is_dir() {
    return Ok(Some(detect_fresh(dir)));
  }

  // ende.json import-based detection
  if let Some(imports) = read_ende_json_imports(dir)
    && imports
      .iter()
      .any(|i| i.starts_with("fresh") || i.starts_with("@fresh/core"))
  {
    return Ok(Some(detect_fresh(dir)));
  }

  Ok(None)
}

// --- Framework-specific detection ---

fn ende_exe() -> String {
  std::env::current_exe()
    .map(|p| p.display().to_string())
    .unwrap_or_else(|_| "ende".into())
}

fn ende_task_build() -> Vec<String> {
  vec![ende_exe(), "task".into(), "build".into()]
}

fn ende_task_dev() -> Vec<String> {
  vec![ende_exe(), "task".into(), "dev".into()]
}

fn detect_fresh(dir: &Path) -> FrameworkDetection {
  // Fresh 2.x uses _fresh/server.js (build output) or imports @fresh/core
  // in ende.json. We intentionally do NOT use `fresh.gen.ts + ende.json`
  // as a heuristic because Fresh 1 projects also have both of those files.
  let is_fresh2 = dir.join("_fresh/server.js").exists()
    || read_ende_json_imports(dir)
      .map(|imports| imports.iter().any(|i| i.starts_with("@fresh/core")))
      .unwrap_or(false);
  if is_fresh2 {
    // `_fresh/snapshot.js` records static assets as `filePath:
    // "static/foo.png"` and `_fresh/server.js` constructs the
    // ProdBuildCache with `root = path.join(import.meta.dirname, "..")`,
    // so the runtime reads them via `<root>/static/...`. The `static/`
    // directory must therefore land in the VFS alongside `_fresh/` or
    // every image / font / video 404s. `static/` is conventional for
    // Fresh; if it doesn't exist the include is a harmless no-op.
    let mut include_paths = vec!["_fresh".into()];
    if dir.join("static").is_dir() {
      include_paths.push("static".into());
    }
    FrameworkDetection {
      name: "Fresh",
      entrypoint_code: r#"// @ts-nocheck
const mod = await import("./_fresh/server.js");
Ende.serve(mod.default.fetch);
"#
      .into(),
      include_paths,
      build_command: Some(ende_task_build()),
      hmr_command: Some(ende_task_dev()),
    }
  } else {
    // Fresh 1.x — no build step needed, server-rendered
    FrameworkDetection {
      name: "Fresh",
      entrypoint_code: "// @ts-nocheck\nimport \"./main.ts\";\n".into(),
      include_paths: vec![],
      build_command: None,
      hmr_command: None,
    }
  }
}

/// Read the `imports` keys from ende.json / ende.jsonc.
///
/// Uses JSONC-aware parsing so that commented ende.jsonc files are
/// handled correctly instead of silently failing detection.
fn read_ende_json_imports(dir: &Path) -> Option<Vec<String>> {
  let content = ende_config::ende_json::CONFIG_FILE_NAMES
    .iter()
    .find_map(|name| std::fs::read_to_string(dir.join(name)).ok())?;
  let config: serde_json::Value =
    jsonc_parser::parse_to_serde_value(&content, &Default::default())
      .ok()
      .flatten()?;
  let imports = config.get("imports")?.as_object()?;
  Some(imports.keys().cloned().collect())
}

#[cfg(test)]
mod tests {
  use std::fs;

  use super::*;

  fn setup_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
  }

  fn fresh_detection() -> FrameworkDetection {
    FrameworkDetection {
      name: "Fresh",
      entrypoint_code: String::new(),
      include_paths: vec![],
      build_command: None,
      hmr_command: None,
    }
  }

  #[test]
  fn no_framework_empty_dir() {
    let dir = setup_dir();
    let result = detect_framework(dir.path()).unwrap();
    assert!(result.is_none());
  }

  #[test]
  fn detects_fresh_gen_ts() {
    let dir = setup_dir();
    fs::write(dir.path().join("fresh.gen.ts"), "").unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
    assert!(det.include_paths.is_empty());
    // no _fresh/server.js => Fresh 1.x
    assert!(det.entrypoint_code.contains("main.ts"));
  }

  #[test]
  fn detects_fresh2_with_server_js() {
    let dir = setup_dir();
    fs::create_dir_all(dir.path().join("_fresh")).unwrap();
    fs::write(dir.path().join("_fresh/server.js"), "").unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
    assert!(det.entrypoint_code.contains("_fresh/server.js"));
    let cmd = det.build_command.unwrap();
    assert_eq!(cmd[1..], vec!["task", "build"]);
  }

  #[test]
  fn fresh1_has_no_build_command() {
    let dir = setup_dir();
    fs::write(dir.path().join("fresh.gen.ts"), "").unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
    assert!(det.build_command.is_none());
  }

  #[test]
  fn fresh1_with_ende_json_stays_fresh1() {
    // Regression: fresh.gen.ts + ende.json (without @fresh/core import)
    // should be treated as Fresh 1, not Fresh 2.
    let dir = setup_dir();
    fs::write(dir.path().join("fresh.gen.ts"), "").unwrap();
    fs::write(
      dir.path().join("ende.json"),
      r#"{"tasks":{"start":"deno run -A main.ts"}}"#,
    )
    .unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
    // Fresh 1.x uses main.ts, not _fresh/server.js
    assert!(det.entrypoint_code.contains("main.ts"));
    assert!(det.build_command.is_none());
  }

  #[test]
  fn fresh2_detected_via_fresh_core_import() {
    let dir = setup_dir();
    fs::write(dir.path().join("fresh.gen.ts"), "").unwrap();
    fs::write(
      dir.path().join("ende.json"),
      r#"{"imports":{"@fresh/core":"jsr:@fresh/core@^2"}}"#,
    )
    .unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
    // Should be Fresh 2 because @fresh/core is in imports
    assert!(det.entrypoint_code.contains("_fresh/server.js"));
    assert!(det.build_command.is_some());
  }

  #[test]
  fn detects_fresh_from_ende_json_imports() {
    let dir = setup_dir();
    fs::write(
      dir.path().join("ende.json"),
      r#"{"imports":{"fresh":"jsr:@fresh/core"}}"#,
    )
    .unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
  }

  #[test]
  fn detects_fresh_from_ende_json_fresh_core() {
    let dir = setup_dir();
    fs::write(
      dir.path().join("ende.json"),
      r#"{"imports":{"@fresh/core":"jsr:@fresh/core@^2"}}"#,
    )
    .unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
  }

  #[test]
  fn detects_fresh_from_ende_jsonc() {
    let dir = setup_dir();
    fs::write(
      dir.path().join("ende.jsonc"),
      r#"{"imports":{"fresh":"jsr:@fresh/core"}}"#,
    )
    .unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
  }

  #[test]
  fn detects_fresh_from_commented_ende_jsonc() {
    // Regression: ende.jsonc with comments should still be parsed correctly.
    let dir = setup_dir();
    fs::write(
      dir.path().join("ende.jsonc"),
      "{\n  // This is a comment\n  \"imports\": {\n    \"fresh\": \"jsr:@fresh/core\"\n  }\n}\n",
    )
    .unwrap();
    let det = detect_framework(dir.path()).unwrap().unwrap();
    assert_eq!(det.name, "Fresh");
  }

  #[test]
  fn read_ende_json_imports_returns_keys() {
    let dir = setup_dir();
    fs::write(
      dir.path().join("ende.json"),
      r#"{"imports":{"foo":"jsr:@foo/bar","baz":"npm:baz"}}"#,
    )
    .unwrap();
    let mut imports = read_ende_json_imports(dir.path()).unwrap();
    imports.sort();
    assert_eq!(imports, vec!["baz", "foo"]);
  }

  #[test]
  fn read_ende_json_imports_no_file() {
    let dir = setup_dir();
    assert!(read_ende_json_imports(dir.path()).is_none());
  }

  #[test]
  fn read_ende_json_imports_no_imports_key() {
    let dir = setup_dir();
    fs::write(dir.path().join("ende.json"), r#"{"tasks":{}}"#).unwrap();
    assert!(read_ende_json_imports(dir.path()).is_none());
  }

  #[test]
  fn finds_favicon_in_static_for_linux() {
    let dir = setup_dir();
    fs::create_dir_all(dir.path().join("static")).unwrap();
    fs::write(dir.path().join("static/favicon.png"), "").unwrap();
    let det = fresh_detection();
    let p = find_framework_favicon(dir.path(), &det, "linux").unwrap();
    assert_eq!(p, dir.path().join("static/favicon.png"));
  }

  #[test]
  fn finds_ico_for_windows_but_not_png() {
    let dir = setup_dir();
    fs::create_dir_all(dir.path().join("static")).unwrap();
    fs::write(dir.path().join("static/favicon.png"), "").unwrap();
    let det = fresh_detection();
    assert!(find_framework_favicon(dir.path(), &det, "windows").is_none());
    fs::write(dir.path().join("static/favicon.ico"), "").unwrap();
    let p = find_framework_favicon(dir.path(), &det, "windows").unwrap();
    assert_eq!(p, dir.path().join("static/favicon.ico"));
  }

  #[test]
  fn macos_prefers_icns_over_png() {
    let dir = setup_dir();
    fs::create_dir_all(dir.path().join("static")).unwrap();
    fs::write(dir.path().join("static/icon.png"), "").unwrap();
    fs::write(dir.path().join("static/icon.icns"), "").unwrap();
    let det = fresh_detection();
    let p = find_framework_favicon(dir.path(), &det, "macos").unwrap();
    assert_eq!(p, dir.path().join("static/icon.icns"));
  }

  #[test]
  fn icon_name_preferred_over_favicon() {
    let dir = setup_dir();
    fs::create_dir_all(dir.path().join("static")).unwrap();
    fs::write(dir.path().join("static/favicon.png"), "").unwrap();
    fs::write(dir.path().join("static/icon.png"), "").unwrap();
    let det = fresh_detection();
    let p = find_framework_favicon(dir.path(), &det, "linux").unwrap();
    assert_eq!(p, dir.path().join("static/icon.png"));
  }

  #[test]
  fn fresh_uses_static_dir() {
    let dir = setup_dir();
    fs::create_dir_all(dir.path().join("static")).unwrap();
    fs::write(dir.path().join("static/favicon.png"), "").unwrap();
    let det = fresh_detection();
    let p = find_framework_favicon(dir.path(), &det, "linux").unwrap();
    assert_eq!(p, dir.path().join("static/favicon.png"));
  }

  #[test]
  fn no_favicon_returns_none() {
    let dir = setup_dir();
    fs::create_dir_all(dir.path().join("static")).unwrap();
    let det = fresh_detection();
    assert!(find_framework_favicon(dir.path(), &det, "linux").is_none());
  }

  #[test]
  fn run_build_command_none_is_noop() {
    let dir = setup_dir();
    let mut det = fresh_detection();
    det.build_command = None;
    // No command to run, so this must succeed without doing anything.
    run_build_command(&det, dir.path()).unwrap();
  }

  #[test]
  fn run_build_command_runs_in_dir() {
    // The build command runs with the detected directory as cwd. Use a
    // shell command that writes the framework's build output so we can assert
    // it ran in the right place (a regression test for #35535, where the
    // build step was skipped and the include path didn't exist).
    let dir = setup_dir();
    let mut det = fresh_detection();
    det.include_paths = vec!["dist".into()];
    det.build_command = if cfg!(windows) {
      Some(vec!["cmd".into(), "/C".into(), "mkdir dist".into()])
    } else {
      Some(vec!["sh".into(), "-c".into(), "mkdir dist".into()])
    };
    run_build_command(&det, dir.path()).unwrap();
    assert!(dir.path().join("dist").is_dir());
  }

  #[test]
  fn run_build_command_propagates_failure() {
    let dir = setup_dir();
    let mut det = fresh_detection();
    det.build_command = if cfg!(windows) {
      Some(vec!["cmd".into(), "/C".into(), "exit 1".into()])
    } else {
      Some(vec!["sh".into(), "-c".into(), "exit 1".into()])
    };
    let err = run_build_command(&det, dir.path()).unwrap_err();
    assert!(err.to_string().contains("build failed"));
  }
}
