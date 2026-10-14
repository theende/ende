// Copyright 2018-2026 the Deno authors. MIT license.

use test_util as util;
use test_util::test;
use util::TestContext;
use util::TestContextBuilder;
use util::assert_not_contains;
use util::testdata_path;

#[test]
fn compile_basic() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("welcome.exe")
  } else {
    dir.path().join("welcome")
  };
  // try this twice to ensure it works with the cache
  for _ in 0..2 {
    let output = context
      .new_command()
      .args_vec([
        "compile",
        "--output",
        &exe.to_string_lossy(),
        "../../tests/testdata/welcome.ts",
      ])
      .run();
    output.assert_exit_code(0);
    output.skip_output_check();
    let output = context.new_command().name(&exe).run();
    output.assert_matches_text("Welcome to Deno!\n");
  }

  // On arm64 macOS, check if `codesign -v` passes
  #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
  {
    let output = std::process::Command::new("codesign")
      .arg("-v")
      .arg(&exe)
      .output()
      .unwrap();
    assert!(output.status.success());
  }

  // now ensure this works when the ende_dir is readonly
  let readonly_dir = dir.path().join("readonly");
  readonly_dir.make_dir_readonly();
  let readonly_sub_dir = readonly_dir.join("sub");

  let output = context
    .new_command()
    // it should fail creating this, but still work
    .env("ENDE_DIR", readonly_sub_dir)
    .name(exe)
    .run();
  output.assert_matches_text("Welcome to Deno!\n");
}

#[test]
fn standalone_args() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("args.exe")
  } else {
    dir.path().join("args")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./compile/args.ts",
      "a",
      "b",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  context
    .new_command()
    .name(&exe)
    .args("foo --bar --unstable")
    .run()
    .assert_exit_code(0)
    .assert_matches_text("a\nb\nfoo\n--bar\n--unstable\n");
}

#[test]
fn standalone_load_datauri() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("load_datauri.exe")
  } else {
    dir.path().join("load_datauri")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./compile/standalone_import_datauri.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  context
    .new_command()
    .name(&exe)
    .run()
    .assert_exit_code(0)
    .assert_matches_text("Hello Deno!\n");
}

// https://github.com/denoland/deno/issues/13704
#[test]
fn standalone_follow_redirects() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("follow_redirects.exe")
  } else {
    dir.path().join("follow_redirects")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "--config",
      "../config/ende.json",
      "./compile/standalone_follow_redirects.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  context
    .new_command()
    .name(&exe)
    .run()
    .assert_exit_code(0)
    .assert_matches_text("Hello\n");
}

#[test]
fn compile_with_file_exists_error() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let output_path = if cfg!(windows) {
    dir.path().join(r"args\")
  } else {
    dir.path().join("args/")
  };
  let file_path = dir.path().join("args");
  file_path.write("");
  context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &output_path.to_string_lossy(),
      "./compile/args.ts",
    ])
    .run()
    .assert_exit_code(1)
    .assert_matches_text(format!(
      concat!(
        "[WILDCARD]error: Could not compile to file '{}' because its parent directory ",
        "is an existing file. You can use the `--output <file-path>` flag to ",
        "provide an alternative name.\n",
      ),
      file_path,
    ));
}

#[test]
fn compile_with_directory_exists_error() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("args.exe")
  } else {
    dir.path().join("args")
  };
  std::fs::create_dir(&exe).unwrap();
  context.new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./compile/args.ts"
    ]).run()
    .assert_exit_code(1)
    .assert_matches_text(format!(
      concat!(
        "[WILDCARD]error: Could not compile to file '{}' because a directory exists with ",
        "the same name. You can use the `--output <file-path>` flag to ",
        "provide an alternative name.\n"
      ),
      exe
    ));
}

#[test]
fn compile_with_conflict_file_exists_error() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("args.exe")
  } else {
    dir.path().join("args")
  };
  std::fs::write(&exe, b"SHOULD NOT BE OVERWRITTEN").unwrap();
  context.new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./compile/args.ts"
    ]).run()
    .assert_exit_code(1)
    .assert_matches_text(format!(
      concat!(
        "[WILDCARD]error: Could not compile to file '{}' because the file already exists ",
        "and cannot be overwritten. Please delete the existing file or ",
        "use the `--output <file-path>` flag to provide an alternative name.\n"
      ),
      exe
    ));
  exe.assert_matches_text("SHOULD NOT BE OVERWRITTEN");
}

#[test]
fn compile_and_overwrite_file() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("args.exe")
  } else {
    dir.path().join("args")
  };

  // do this twice
  for _ in 0..2 {
    context
      .new_command()
      .args_vec([
        "compile",
        "--output",
        &exe.to_string_lossy(),
        "./compile/args.ts",
      ])
      .run()
      .skip_output_check()
      .assert_exit_code(0);
    assert!(&exe.exists());
  }
}

#[test]
fn standalone_runtime_flags() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("flags.exe")
  } else {
    dir.path().join("flags")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--allow-read",
      "--seed",
      "1",
      "--output",
      &exe.to_string_lossy(),
      "./compile/standalone_runtime_flags.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  context
    .new_command()
    .env("NO_COLOR", "1")
    .name(&exe)
    .split_output()
    .run()
    .assert_exit_code(1)
    .assert_stdout_matches_text("0.16917030546536072\n")
    .assert_stderr_matches_text(
      "[WILDCARD]NotCapable: Requires write access to[WILDCARD]",
    );
}

#[test]
fn standalone_ext_flag_ts() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("ext_flag_ts.exe")
  } else {
    dir.path().join("ext_flag_ts")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--ext",
      "ts",
      "--output",
      &exe.to_string_lossy(),
      "./file_extensions/ts_without_extension",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  context
    .new_command()
    .env("NO_COLOR", "1")
    .name(&exe)
    .run()
    .assert_exit_code(0)
    .assert_matches_text("executing typescript with no extension\n");
}

#[test]
fn standalone_ext_flag_js() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("ext_flag_js.exe")
  } else {
    dir.path().join("ext_flag_js")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--ext",
      "js",
      "--output",
      &exe.to_string_lossy(),
      "./file_extensions/js_without_extension",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  context
    .new_command()
    .env("NO_COLOR", "1")
    .name(&exe)
    .run()
    .assert_matches_text("executing javascript with no extension\n");
}

#[test]
fn standalone_import_map() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("import_map.exe")
  } else {
    dir.path().join("import_map")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--allow-read",
      "--import-map",
      "compile/standalone_import_map.json",
      "--output",
      &exe.to_string_lossy(),
      "./compile/standalone_import_map.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  context
    .new_command()
    .name(&exe)
    .run()
    .skip_output_check()
    .assert_exit_code(0);
}

#[test]
fn standalone_import_map_config_file() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("import_map.exe")
  } else {
    dir.path().join("import_map")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--allow-read",
      "--config",
      "compile/standalone_import_map_config.json",
      "--output",
      &exe.to_string_lossy(),
      "./compile/standalone_import_map.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  context
    .new_command()
    .name(&exe)
    .run()
    .skip_output_check()
    .assert_exit_code(0);
}

#[test]
// https://github.com/denoland/deno/issues/12670
fn skip_rebundle() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("hello_world.exe")
  } else {
    dir.path().join("hello_world")
  };
  let output = context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./run/001_hello.js",
    ])
    .run();

  //no "Bundle testdata_path/run/001_hello.js" in output
  assert_not_contains!(output.combined_output(), "Bundle");

  context
    .new_command()
    .name(&exe)
    .run()
    .assert_exit_code(0)
    .assert_matches_text("Hello World\n");
}

#[test]
fn check_local_by_default() {
  let context = TestContext::with_http_server();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("welcome.exe")
  } else {
    dir.path().join("welcome")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--allow-import",
      "--output",
      &exe.to_string_lossy(),
      "./compile/check_local_by_default.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
}

#[test]
fn check_local_by_default2() {
  let context = TestContext::with_http_server();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("welcome.exe")
  } else {
    dir.path().join("welcome")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--allow-import",
      "--output",
      &exe.to_string_lossy(),
      "./compile/check_local_by_default2.ts"
    ])
    .run()
    .assert_exit_code(1)
    .assert_matches_text(
      r#"[WILDCARD]TS2322 [ERROR]: Type '12' is not assignable to type '"b"'.[WILDCARD]"#,
    );
}

#[test]
fn workers_basic() {
  let context = TestContext::with_http_server();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("basic.exe")
  } else {
    dir.path().join("basic")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--no-check",
      "--output",
      &exe.to_string_lossy(),
      "./compile/workers/basic.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);

  context
    .new_command()
    .name(&exe)
    .run()
    .assert_exit_code(0)
    .assert_matches_file("./compile/workers/basic.out");
}

#[test]
fn workers_not_in_module_map() {
  let context = TestContext::with_http_server();
  let temp_dir = context.temp_dir();
  let exe = if cfg!(windows) {
    temp_dir.path().join("not_in_module_map.exe")
  } else {
    temp_dir.path().join("not_in_module_map")
  };
  let output = context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./compile/workers/not_in_module_map.ts",
    ])
    .run();
  output.assert_exit_code(0);
  output.skip_output_check();

  let output = context.new_command().name(exe).env("NO_COLOR", "").run();
  output.assert_exit_code(1);
  output.assert_matches_text(concat!(
    "error: Uncaught (in worker \"\") Module not found: [WILDCARD]",
    "error: Uncaught (in promise) Error: Unhandled error in child worker.\n[WILDCARD]"
  ));
}

#[test]
fn workers_with_include_flag() {
  let context = TestContext::with_http_server();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("workers_with_include_flag.exe")
  } else {
    dir.path().join("workers_with_include_flag")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "--include",
      "./compile/workers/worker.ts",
      "./compile/workers/not_in_module_map.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);

  context
    .new_command()
    .name(&exe)
    .env("NO_COLOR", "")
    .run()
    .assert_matches_text("Hello from worker!\nReceived 42\nClosing\n");
}

#[test]
fn dynamic_import() {
  let context = TestContext::with_http_server();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("dynamic_import.exe")
  } else {
    dir.path().join("dynamic_import")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./compile/dynamic_imports/main.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);

  context
    .new_command()
    .name(&exe)
    .env("NO_COLOR", "")
    .run()
    .assert_matches_file("./compile/dynamic_imports/main.out")
    .assert_exit_code(0);
}

#[test]
fn dynamic_import_unanalyzable() {
  let context = TestContext::with_http_server();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("dynamic_import_unanalyzable.exe")
  } else {
    dir.path().join("dynamic_import_unanalyzable")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--allow-read",
      "--include",
      "./compile/dynamic_imports/import1.ts",
      "--output",
      &exe.to_string_lossy(),
      "./compile/dynamic_imports/main_unanalyzable.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);

  context
    .new_command()
    .current_dir(util::root_path())
    .name(&exe)
    .env("NO_COLOR", "")
    .run()
    .assert_matches_file("./compile/dynamic_imports/main.out")
    .assert_exit_code(0);
}

#[test]
fn dynamic_imports_tmp_lit() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("app.exe")
  } else {
    dir.path().join("app")
  };
  let output = context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./compile/dynamic_imports_tmp_lit/main.js",
    ])
    .run();
  output.assert_exit_code(0);
  output.skip_output_check();
  let output = context.new_command().name(&exe).run();
  output.assert_matches_text("a\nb\n{ data: 5 }\n{ data: 1 }\n");
}

#[test]
fn granular_unstable_features() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("app.exe")
  } else {
    dir.path().join("app")
  };
  let output = context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "--unstable-cron",
      "./compile/unstable_features.ts",
    ])
    .run();
  output.assert_exit_code(0);
  output.skip_output_check();
  let output = context.new_command().name(&exe).run();
  output.assert_exit_code(0);
  output.assert_matches_text("function\nObject [Temporal] {}\n");
}

#[test]
fn granular_unstable_features_config_file() {
  let context = TestContextBuilder::new().use_temp_cwd().build();
  let dir = context.temp_dir();
  testdata_path()
    .join("compile/unstable_features.ts")
    .copy(&dir.path().join("unstable_features.ts"));
  let exe = if cfg!(windows) {
    dir.path().join("app.exe")
  } else {
    dir.path().join("app")
  };
  dir.write(
    "ende.json",
    serde_json::to_string_pretty(&serde_json::json!({
      "unstable": ["cron", "temporal"]
    }))
    .unwrap(),
  );
  let output = context
    .new_command()
    .args_vec([
      "compile",
      "--config",
      &dir.path().join("ende.json").to_string(),
      "--output",
      &exe.to_string_lossy(),
      "./unstable_features.ts",
    ])
    .run();
  output.assert_exit_code(0);
  output.skip_output_check();
  let output = context.new_command().name(&exe).run();
  output.assert_exit_code(0);
  output.assert_matches_text("function\nObject [Temporal] {}\n");
}

#[test]
fn dynamic_import_bad_data_uri() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("app.exe")
  } else {
    dir.path().join("app")
  };
  let file = dir.path().join("bad_data_uri.ts");
  file.write("await import('data:application/')");
  let output = context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      &file.to_string_lossy(),
    ])
    .run();
  output.assert_exit_code(0);
  output.skip_output_check();
  let output = context.new_command().name(&exe).run();
  output.assert_exit_code(1);
  output.assert_matches_text(
    "[WILDCARD]TypeError: Unable to decode data url.[WILDCARD]",
  );
}

#[test]
fn standalone_config_file_respects_compiler_options() {
  let context = TestContextBuilder::new().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("compiler_options.exe")
  } else {
    dir.path().join("compiler_options")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--allow-read",
      "--config",
      "compile/compiler_options/ende.json",
      "--output",
      &exe.to_string_lossy(),
      "./compile/compiler_options/main.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  let output = context.new_command().name(&exe).run();

  output.assert_exit_code(0);
  output.assert_matches_text("[WILDCARD]C.test() called[WILDCARD]");
}

#[test]
fn standalone_jsr_dynamic_import() {
  let context = TestContextBuilder::for_jsr().build();
  let dir = context.temp_dir();
  let exe = if cfg!(windows) {
    dir.path().join("jsr_dynamic_import.exe")
  } else {
    dir.path().join("jsr_dynamic_import")
  };
  context
    .new_command()
    .args_vec([
      "compile",
      "--output",
      &exe.to_string_lossy(),
      "./compile/jsr_dynamic_import/main.ts",
    ])
    .run()
    .skip_output_check()
    .assert_exit_code(0);
  let output = context.new_command().name(&exe).run();

  output.assert_exit_code(0);
  output.assert_matches_text("Hello world\n");
}
