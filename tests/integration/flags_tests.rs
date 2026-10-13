// Copyright 2018-2026 the Deno authors. MIT license.

use test_util as util;
use test_util::test;
use util::assert_contains;

#[test]
fn help_output() {
  let output = util::deno_cmd()
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
    "Deno kernel for Jupyter notebooks",
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
}

/// `ende --help` and `ende <sub> --help` speak "ende". The only remaining
/// mentions of "deno" are third-party host names, the deprecated `DENO_*`
/// env var note, header names, and doomed features (Deno Deploy, Jupyter).
#[test]
fn help_output_says_ende() {
  const ALLOWED: &[&str] = &[
    "deno.land",
    "The deprecated DENO_* names are still accepted.",
    "X-deno-client-address",
    "Deno Deploy",
    "Deno kernel for Jupyter notebooks",
  ];
  for args in [
    &["--help"][..],
    &["--help=full"][..],
    &["run", "--help"][..],
    &["vendor", "--help"][..],
  ] {
    let output = util::deno_cmd()
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
