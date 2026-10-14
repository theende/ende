// Copyright 2018-2026 the Deno authors. MIT license.

use serde_json::json;
use test_util::TestContextBuilder;
use test_util::assert_contains;
use test_util::env_vars_for_jsr_npm_tests;
use test_util::pty::Pty;
use test_util::test;

#[test]
fn add_basic() {
  let starting_ende_json = json!({
    "name": "@foo/bar",
    "version": "1.0.0",
    "exports": "./mod.ts",
  });
  let context = pm_context_builder().build();
  let temp_dir = context.temp_dir().path();
  temp_dir.join("ende.json").write_json(&starting_ende_json);

  let output = context.new_command().args("add jsr:@endetest/add").run();
  output.assert_exit_code(0);
  let output = output.combined_output();
  assert_contains!(output, "Add jsr:@endetest/add");
  temp_dir.join("ende.json").assert_matches_json(json!({
    "name": "@foo/bar",
    "version": "1.0.0",
    "exports": "./mod.ts",
    "imports": {
      "@endetest/add": "jsr:@endetest/add@^1.0.0"
    }
  }));
}

#[test]
fn add_basic_no_ende_json() {
  let context = pm_context_builder().build();
  let temp_dir = context.temp_dir().path();

  let output = context.new_command().args("add jsr:@endetest/add").run();
  output.assert_exit_code(0);
  let output = output.combined_output();
  assert_contains!(output, "Add jsr:@endetest/add");
  // Don't use `assert_matches_json` to ensure the file is properly formatted.
  let expected = r#"{
  "imports": {
    "@endetest/add": "jsr:@endetest/add@^1.0.0"
  }
}
"#;
  temp_dir.join("ende.json").assert_matches_text(expected);
}

#[test]
fn add_basic_with_empty_ende_json() {
  let context = pm_context_builder().build();
  let temp_dir = context.temp_dir();
  temp_dir.write("ende.json", "");

  let output = context.new_command().args("add jsr:@endetest/add").run();
  output.assert_exit_code(0);
  let output = output.combined_output();
  assert_contains!(output, "Add jsr:@endetest/add");
  temp_dir
    .path()
    .join("ende.json")
    .assert_matches_json(json!({
      "imports": {
        "@endetest/add": "jsr:@endetest/add@^1.0.0"
      }
    }));
}

#[test]
fn add_version_contraint() {
  let context = pm_context_builder().build();
  let temp_dir = context.temp_dir().path();

  let output = context.new_command().args("add jsr:@endetest/add@1").run();
  output.assert_exit_code(0);
  let output = output.combined_output();
  assert_contains!(output, "Add jsr:@endetest/add");
  temp_dir.join("ende.json").assert_matches_json(json!({
    "imports": {
      "@endetest/add": "jsr:@endetest/add@^1.0.0"
    }
  }));
}

#[test]
fn add_tilde() {
  let context = pm_context_builder().build();
  let temp_dir = context.temp_dir().path();

  let output = context.new_command().args("add jsr:@endetest/add@~1").run();
  output.assert_exit_code(0);
  let output = output.combined_output();
  assert_contains!(output, "Add jsr:@endetest/add");
  temp_dir.join("ende.json").assert_matches_json(json!({
    "imports": {
      "@endetest/add": "jsr:@endetest/add@~1.0.0"
    }
  }));
}

#[test]
fn add_save_exact() {
  let context = pm_context_builder().build();
  let temp_dir = context.temp_dir().path();

  let output = context
    .new_command()
    .args("add jsr:@endetest/add --save-exact")
    .run();
  output.assert_exit_code(0);
  let output = output.combined_output();
  assert_contains!(output, "Add jsr:@endetest/add");
  temp_dir.join("ende.json").assert_matches_json(json!({
    "imports": {
      "@endetest/add": "jsr:@endetest/add@1.0.0"
    }
  }));
}

#[test]
fn add_exact_alias() {
  let context = pm_context_builder().build();
  let temp_dir = context.temp_dir().path();

  let output = context
    .new_command()
    .args("add jsr:@endetest/add --exact")
    .run();
  output.assert_exit_code(0);
  let output = output.combined_output();
  assert_contains!(output, "Add jsr:@endetest/add");
  temp_dir.join("ende.json").assert_matches_json(json!({
    "imports": {
      "@endetest/add": "jsr:@endetest/add@1.0.0"
    }
  }));
}

#[test]
fn add_multiple() {
  let starting_ende_json = json!({
    "name": "@foo/bar",
    "version": "1.0.0",
    "exports": "./mod.ts",
  });
  let context = pm_context_builder().build();
  let temp_dir = context.temp_dir().path();
  temp_dir.join("ende.json").write_json(&starting_ende_json);

  let output = context
    .new_command()
    .args("add jsr:@endetest/add jsr:@endetest/subset-type-graph")
    .run();
  output.assert_exit_code(0);
  let output = output.combined_output();
  assert_contains!(output, "Add jsr:@endetest/add");
  temp_dir.join("ende.json").assert_matches_json(json!({
    "name": "@foo/bar",
    "version": "1.0.0",
    "exports": "./mod.ts",
    "imports": {
      "@endetest/add": "jsr:@endetest/add@^1.0.0",
      "@endetest/subset-type-graph": "jsr:@endetest/subset-type-graph@^0.1.0"
    }
  }));
}

fn pm_context_builder() -> TestContextBuilder {
  TestContextBuilder::new()
    .use_http_server()
    .envs(env_vars_for_jsr_npm_tests())
    .use_temp_cwd()
}

#[test(flaky)]
fn update_interactive_shows_version_req() {
  if !Pty::is_supported() {
    return;
  }
  let context = pm_context_builder().build();
  // The requirement (`^0.2.0`) resolves to the latest compatible version
  // (`0.2.1`). The interactive picker must show the version requirement being
  // updated (`0.2.0 -> 0.2.1`), not the resolved version on both sides
  // (`0.2.1 -> 0.2.1`). Regression test for
  // https://github.com/denoland/deno/issues/34668
  context.temp_dir().write(
    "ende.json",
    r#"{"imports": {"@endetest/add": "jsr:@endetest/add@^0.2.0"}}"#,
  );
  context
    .new_command()
    .args("install")
    .run()
    .skip_output_check();
  context
    .new_command()
    .args("update --interactive")
    .with_pty(|mut pty| {
      pty.expect("Select which packages to update");
      pty.expect("0.2.0 -> 0.2.1");
    });
}
