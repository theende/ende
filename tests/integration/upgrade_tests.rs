// Copyright 2018-2026 the Deno authors. MIT license.

use std::process::Command;
use std::process::Stdio;
use std::time::Instant;

use test_util as util;
use util::TestContext;
use util::TestContextBuilder;
use util::assert_starts_with;
use util::test;

#[test(flaky)]
fn upgrade_invalid_lockfile() {
  let context = upgrade_context();
  let temp_dir = context.temp_dir();
  temp_dir.write("deno.deno", r#"{ \"lock\": true }"#);
  temp_dir.write(
    "ende.lock",
    r#"{
  "version": "invalid",
}"#,
  );
  let exe_path = temp_dir.path().join("deno");
  util::ende_exe_path().copy(&exe_path);
  assert!(exe_path.exists());
  exe_path.mark_executable();
  let output = Command::new(&exe_path)
    .arg("upgrade")
    .arg("--version")
    .arg("foobar")
    .arg("--dry-run")
    .stderr(Stdio::piped())
    .spawn()
    .unwrap()
    .wait_with_output()
    .unwrap();
  assert!(!output.status.success());
  // should make it here instead of erroring on an invalid lockfile
  assert_starts_with!(
    &util::strip_ansi_codes(&String::from_utf8(output.stderr.clone()).unwrap())
      .to_string(),
    "error: Invalid version passed (foobar)"
  );
}

#[test(flaky)]
fn upgrade_prompt() {
  let context = upgrade_context();
  let temp_dir = context.temp_dir();
  // start a task that goes indefinitely in order to allow
  // the upgrade check to occur
  temp_dir.write("main.js", "setInterval(() => {}, 1_000)");
  let cmd = context
    .new_command()
    .args("run --log-level=debug main.js")
    .env_remove("ENDE_NO_UPDATE_CHECK");
  // run once and wait for the version to be stored
  cmd.with_pty(|mut pty| {
    pty.expect("Finished upgrade checker.");
  });
  // now check that the upgrade prompt is shown the next time this is run
  temp_dir.write("main.js", "");
  cmd.with_pty(|mut pty| {
    // - We need to use a pty here because the upgrade prompt
    //   doesn't occur except when there's a pty.
    // - Version comes from the test server.
    pty.expect(" 9999.12.31 Run `ende upgrade` to install it.");
  });
}

#[test(flaky)]
fn upgrade_lsp_repl_sleeps() {
  let context = TestContextBuilder::new()
    .use_http_server()
    .use_temp_cwd()
    .env(
      "ENDE_DONT_USE_INTERNAL_BASE_UPGRADE_URL",
      "http://localhost:4545/upgrade/sleep",
    )
    .build();
  let start_instant = Instant::now();
  // ensure this works even though the upgrade check is taking
  // a long time to complete
  context
    .new_command()
    .args("repl")
    .env_remove("ENDE_NO_UPDATE_CHECK")
    .with_pty(|mut pty| {
      pty.write_line("123 + 456\n");
      pty.expect("579");
    });

  // the test server will sleep for 95 seconds, so ensure this is less
  let elapsed_secs = start_instant.elapsed().as_secs();
  assert!(elapsed_secs < 94, "elapsed_secs: {}", elapsed_secs);
}

fn upgrade_context() -> TestContext {
  TestContextBuilder::new()
    .use_http_server()
    .use_temp_cwd()
    .env(
      "ENDE_DONT_USE_INTERNAL_BASE_UPGRADE_URL",
      "http://localhost:4545",
    )
    .build()
}

/// A fake `gh` for `upgrade --pr` / `--branch`: it finds PR #1 and CI run 1,
/// and `gh run download` writes an `ende` that passes the `-V` check.
#[cfg(unix)]
fn write_fake_gh(dir: &util::PathRef) {
  let gh = dir.join("gh");
  gh.write(
    r#"#!/bin/sh
case "$1 $2" in
  "--version "*) echo "gh version 0.0.0" ;;
  "pr view") printf 'Title\tOPEN\tbranch\tabc123\n' ;;
  "run list") echo 1 ;;
  "run download")
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "--dir" ]; then dir="$2"; fi
      shift
    done
    printf '#!/bin/sh\nexit 0\n' > "$dir/ende"
    ;;
  *) exit 1 ;;
esac
"#,
  );
  gh.mark_executable();
}

/// `upgrade --pr` and `--branch` unpack and run the new binary next to the
/// output path, not in the system temp dir, which is often mounted `noexec`
/// (here it doesn't exist at all).
#[cfg(unix)]
#[test]
fn upgrade_from_pr_or_branch_skips_system_temp_dir() {
  let context = TestContextBuilder::new().use_temp_cwd().build();
  let temp_dir = context.temp_dir();
  let bin_dir = temp_dir.path().join("bin");
  bin_dir.create_dir_all();
  write_fake_gh(&bin_dir);
  let path = format!("{}:{}", bin_dir, std::env::var("PATH").unwrap());
  let output_path = temp_dir.path().join("out/ende");
  output_path.parent().create_dir_all();

  for source in ["--pr 1", "--branch main"] {
    let output = context
      .new_command()
      .args(format!("upgrade {source} --dry-run --output {output_path}"))
      .env("PATH", &path)
      .env("TMPDIR", temp_dir.path().join("missing").to_string())
      .run();
    output.assert_exit_code(0);
    assert!(
      output
        .combined_output()
        .contains("Upgraded successfully (dry run)"),
      "{source}: {}",
      output.combined_output()
    );
  }
}
