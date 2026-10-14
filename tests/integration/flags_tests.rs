// Copyright 2018-2026 the Deno authors. MIT license.

use test_util as util;
use test_util::test;
use util::assert_contains;
use util::assert_not_contains;

/// The product version, which `cli/lib/version.txt` holds.
fn ende_version() -> String {
  util::root_path()
    .join("cli/lib/version.txt")
    .read_to_string()
    .trim()
    .to_string()
}

/// `ende --version` prints the ChronVer product version, not a crate version.
#[test]
fn version_output_is_chronver() {
  let version = ende_version();
  let is_chronver = |v: &str| {
    let parts = v.split('.').collect::<Vec<_>>();
    matches!(parts.len(), 3 | 4)
      && parts[0].len() == 4
      && parts[1].len() == 2
      && parts[2].len() == 2
      && parts.iter().all(|p| p.bytes().all(|b| b.is_ascii_digit()))
  };
  assert!(is_chronver(&version), "{version}");

  let output = util::ende_cmd().arg("-V").split_output().run();
  output.assert_exit_code(0);
  assert_eq!(output.stdout(), format!("ende {version}\n"));

  let output = util::ende_cmd().arg("--version").split_output().run();
  output.assert_exit_code(0);
  let first_line = output.stdout().lines().next().unwrap().to_string();
  assert!(
    first_line.starts_with(&format!("ende {version} (")),
    "{first_line}"
  );
}

#[test]
fn help_output() {
  let output = util::ende_cmd()
    .current_dir(util::testdata_path())
    .arg("--help")
    .run();

  let stdout = output.combined_output();
  let subcommand_descriptions = vec![
    "Run a JavaScript or TypeScript program, or a task",
    "Run a server",
    "Run a task defined in the configuration file",
    "Start an interactive Read-Eval-Print Loop (REPL) for Ende",
    "Evaluate a script from the command line",
    "Add dependencies",
    "Installs dependencies either in the local project or globally to a bin directory",
    "Uninstalls a dependency or an executable script in the installation root's bin directory",
    "Run benchmarks",
    "Type-check the dependencies",
    "Compile the script into a self contained executable",
    "Print coverage reports",
    "Generate and show documentation for a module or built-ins",
    "Format source files",
    "Show info about cache or info related to source file",
    "Lint source files",
    "Initialize a new project",
    "Run tests",
    "Publish the current working directory's package or workspace",
    #[cfg(feature = "upgrade")]
    "Upgrade ende executable to given version",
  ];

  for description in subcommand_descriptions {
    assert_contains!(stdout, description);
  }
  // Removed in Ende.
  assert_not_contains!(stdout.to_lowercase(), "jupyter");
  assert_not_contains!(stdout.to_lowercase(), "deploy");
}

/// `ende --help` and `ende <sub> --help` speak "ende". The only remaining
/// mentions of "deno" are third-party host names, the deprecated `DENO_*`
/// env var note.
#[test]
fn help_output_says_ende() {
  const ALLOWED: &[&str] = &[
    "deno.land",
    "The deprecated DENO_* names are still accepted.",
  ];
  for args in [
    &["--help"][..],
    &["--help=full"][..],
    &["run", "--help"][..],
    &["vendor", "--help"][..],
  ] {
    let output = util::ende_cmd()
      .current_dir(util::testdata_path())
      .args_vec(args)
      .run();
    let stdout = output.combined_output();
    assert_contains!(stdout, "Usage: ende");
    for line in stdout.lines() {
      if ALLOWED.iter().any(|allowed| line.contains(allowed)) {
        continue;
      }
      assert!(
        !line.to_lowercase().contains("deno"),
        "`ende {}` help mentions deno: {line}",
        args.join(" ")
      );
    }
  }
}
