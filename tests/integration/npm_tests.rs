// Copyright 2018-2026 the Deno authors. MIT license.

use pretty_assertions::assert_eq;
use serde_json::Value;
use test_util as util;
use test_util::eprintln;
use test_util::test;
use url::Url;
use util::TestContextBuilder;
use util::assert_contains;
use util::env_vars_for_npm_tests;
use util::http_server;

// NOTE: See how to make test npm packages at ./testdata/npm/README.md

#[test]
fn parallel_downloading() {
  let (out, _err) = util::run_and_collect_output_with_args(
    true,
    vec![
      "run",
      "--allow-read",
      "--allow-env",
      "npm/cjs_with_deps/main.js",
    ],
    None,
    // don't use the sync env var
    Some(env_vars_for_npm_tests()),
    true,
  );
  assert!(out.contains("chalk cjs loads"));
}

#[test]
fn cached_only_after_first_run() {
  let _server = http_server();

  let deno_dir = util::new_ende_dir();

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(util::testdata_path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("npm/cached_only_after_first_run/main1.ts")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert_contains!(stderr, "Download");
  assert_contains!(stdout, "[Function: chalk] createChalk");
  assert!(output.status.success());

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(util::testdata_path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("--cached-only")
    .arg("npm/cached_only_after_first_run/main2.ts")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert_contains!(
    stderr,
    "npm package not found in cache: \"ansi-styles\", --cached-only is specified."
  );
  assert!(stdout.is_empty());
  assert!(!output.status.success());

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(util::testdata_path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("--cached-only")
    .arg("npm/cached_only_after_first_run/main1.ts")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();

  let output = deno.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert!(output.status.success());
  assert!(stderr.is_empty());
  assert_contains!(stdout, "[Function: chalk] createChalk");
}

#[test]
fn reload_flag() {
  let _server = http_server();

  let deno_dir = util::new_ende_dir();

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(util::testdata_path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("npm/reload/main.ts")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert_contains!(stderr, "Download");
  assert_contains!(stdout, "[Function: chalk] createChalk");
  assert!(output.status.success());

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(util::testdata_path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("--reload")
    .arg("npm/reload/main.ts")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert_contains!(stderr, "Download");
  assert_contains!(stdout, "[Function: chalk] createChalk");
  assert!(output.status.success());

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(util::testdata_path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("--reload=npm:")
    .arg("npm/reload/main.ts")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert_contains!(stderr, "Download");
  assert_contains!(stdout, "[Function: chalk] createChalk");
  assert!(output.status.success());

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(util::testdata_path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("--reload=npm:chalk")
    .arg("npm/reload/main.ts")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert_contains!(stderr, "Download");
  assert_contains!(stdout, "[Function: chalk] createChalk");
  assert!(output.status.success());

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(util::testdata_path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("--reload=npm:foobar")
    .arg("npm/reload/main.ts")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&output.stderr);
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert!(stderr.is_empty());
  assert_contains!(stdout, "[Function: chalk] createChalk");
  assert!(output.status.success());
}

#[test]
fn deno_run_cjs_module() {
  let _server = http_server();

  let deno_dir = util::new_ende_dir();

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(deno_dir.path())
    .arg("run")
    .arg("--allow-read")
    .arg("--allow-env")
    .arg("--allow-write")
    .arg("npm:mkdirp@1.0.4")
    .arg("test_dir")
    .env("NO_COLOR", "1")
    .envs(env_vars_for_npm_tests())
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  assert!(output.status.success());

  assert!(deno_dir.path().join("test_dir").exists());
}

#[test]
fn deno_run_bin_lockfile() {
  let context = TestContextBuilder::for_npm().use_temp_cwd().build();
  let temp_dir = context.temp_dir();
  temp_dir.write("ende.json", "{}");
  let output = context
    .new_command()
    .args("run -A --quiet npm:@denotest/bin/cli-esm this is a test")
    .run();
  output.assert_matches_file("npm/deno_run_esm.out");
  assert!(temp_dir.path().join("ende.lock").exists());
}

#[test]
fn ensure_registry_files_local() {
  // ensures the registry files all point at local tarballs
  let registry_dir_path = util::tests_path().join("registry").join("npm");
  let check_dir = |dir: &std::path::Path| {
    let registry_json_path = dir.join("registry.json");
    if registry_json_path.exists() {
      let file_text = std::fs::read_to_string(&registry_json_path).unwrap();
      if file_text.contains(&format!(
        "https://registry.npmjs.org/{}/-/",
        dir
          .strip_prefix(&registry_dir_path)
          .unwrap()
          .to_string_lossy()
      )) {
        panic!(
          "file {} contained a reference to the npm registry",
          registry_json_path.display()
        );
      }
    }
  };
  for entry in registry_dir_path.read_dir() {
    let entry = entry.unwrap();
    if entry.file_type().unwrap().is_dir() {
      check_dir(&entry.path());
      for inner in std::fs::read_dir(entry.path()).unwrap() {
        let inner = inner.unwrap();
        if inner.file_type().unwrap().is_dir() {
          check_dir(&inner.path());
        }
      }
    }
  }
}

#[test]
fn lock_file_missing_top_level_package() {
  let _server = http_server();

  let deno_dir = util::new_ende_dir();
  let temp_dir = util::TempDir::new();

  // write empty config file
  temp_dir.write("ende.json", "{}");

  // Lock file that is automatically picked up has been intentionally broken,
  // by removing "cowsay" package from it. This test ensures that npm resolver
  // snapshot can be successfully hydrated in such situation
  let lock_file_content = r#"{
    "version": "2",
    "remote": {},
    "npm": {
      "specifiers": { "cowsay": "cowsay@1.5.0" },
      "packages": {
        "ansi-regex@3.0.1": {
          "integrity": "sha512-+O9Jct8wf++lXxxFc4hc8LsjaSq0HFzzL7cVsw8pRDIPdjKD2mT4ytDZlLuSBZ4cLKZFXIrMGO7DbQCtMJJMKw==",
          "dependencies": {}
        },
        "ansi-regex@5.0.1": {
          "integrity": "sha512-quJQXlTSUGL2LH9SUXo8VwsY4soanhgo6LNSm84E1LBcE8s3O0wpdiRzyR9z/ZZJMlMWv37qOOb9pdJlMUEKFQ==",
          "dependencies": {}
        },
        "ansi-styles@4.3.0": {
          "integrity": "sha512-zbB9rCJAT1rbjiVDb2hqKFHNYLxgtk8NURxZ3IZwD3F6NtxbXZQCnnSi1Lkx+IDohdPlFp222wVALIheZJQSEg==",
          "dependencies": { "color-convert": "color-convert@2.0.1" }
        },
        "camelcase@5.3.1": {
          "integrity": "sha512-L28STB170nwWS63UjtlEOE3dldQApaJXZkOI1uMFfzf3rRuPegHaHesyee+YxQ+W6SvRDQV6UrdOdRiR153wJg==",
          "dependencies": {}
        },
        "cliui@6.0.0": {
          "integrity": "sha512-t6wbgtoCXvAzst7QgXxJYqPt0usEfbgQdftEPbLL/cvv6HPE5VgvqCuAIDR0NgU52ds6rFwqrgakNLrHEjCbrQ==",
          "dependencies": {
            "string-width": "string-width@4.2.3",
            "strip-ansi": "strip-ansi@6.0.1",
            "wrap-ansi": "wrap-ansi@6.2.0"
          }
        },
        "color-convert@2.0.1": {
          "integrity": "sha512-RRECPsj7iu/xb5oKYcsFHSppFNnsj/52OVTRKb4zP5onXwVF3zVmmToNcOfGC+CRDpfK/U584fMg38ZHCaElKQ==",
          "dependencies": { "color-name": "color-name@1.1.4" }
        },
        "color-name@1.1.4": {
          "integrity": "sha512-dOy+3AuW3a2wNbZHIuMZpTcgjGuLU/uBL/ubcZF9OXbDo8ff4O8yVp5Bf0efS8uEoYo5q4Fx7dY9OgQGXgAsQA==",
          "dependencies": {}
        },
        "decamelize@1.2.0": {
          "integrity": "sha512-z2S+W9X73hAUUki+N+9Za2lBlun89zigOyGrsax+KUQ6wKW4ZoWpEYBkGhQjwAjjDCkWxhY0VKEhk8wzY7F5cA==",
          "dependencies": {}
        },
        "emoji-regex@8.0.0": {
          "integrity": "sha512-MSjYzcWNOA0ewAHpz0MxpYFvwg6yjy1NG3xteoqz644VCo/RPgnr1/GGt+ic3iJTzQ8Eu3TdM14SawnVUmGE6A==",
          "dependencies": {}
        },
        "find-up@4.1.0": {
          "integrity": "sha512-PpOwAdQ/YlXQ2vj8a3h8IipDuYRi3wceVQQGYWxNINccq40Anw7BlsEXCMbt1Zt+OLA6Fq9suIpIWD0OsnISlw==",
          "dependencies": {
            "locate-path": "locate-path@5.0.0",
            "path-exists": "path-exists@4.0.0"
          }
        },
        "get-caller-file@2.0.5": {
          "integrity": "sha512-DyFP3BM/3YHTQOCUL/w0OZHR0lpKeGrxotcHWcqNEdnltqFwXVfhEBQ94eIo34AfQpo0rGki4cyIiftY06h2Fg==",
          "dependencies": {}
        },
        "get-stdin@8.0.0": {
          "integrity": "sha512-sY22aA6xchAzprjyqmSEQv4UbAAzRN0L2dQB0NlN5acTTK9Don6nhoc3eAbUnpZiCANAMfd/+40kVdKfFygohg==",
          "dependencies": {}
        },
        "is-fullwidth-code-point@2.0.0": {
          "integrity": "sha512-VHskAKYM8RfSFXwee5t5cbN5PZeq1Wrh6qd5bkyiXIf6UQcN6w/A0eXM9r6t8d+GYOh+o6ZhiEnb88LN/Y8m2w==",
          "dependencies": {}
        },
        "is-fullwidth-code-point@3.0.0": {
          "integrity": "sha512-zymm5+u+sCsSWyD9qNaejV3DFvhCKclKdizYaJUuHA83RLjb7nSuGnddCHGv0hk+KY7BMAlsWeK4Ueg6EV6XQg==",
          "dependencies": {}
        },
        "locate-path@5.0.0": {
          "integrity": "sha512-t7hw9pI+WvuwNJXwk5zVHpyhIqzg2qTlklJOf0mVxGSbe3Fp2VieZcduNYjaLDoy6p9uGpQEGWG87WpMKlNq8g==",
          "dependencies": { "p-locate": "p-locate@4.1.0" }
        },
        "p-limit@2.3.0": {
          "integrity": "sha512-//88mFWSJx8lxCzwdAABTJL2MyWB12+eIY7MDL2SqLmAkeKU9qxRvWuSyTjm3FUmpBEMuFfckAIqEaVGUDxb6w==",
          "dependencies": { "p-try": "p-try@2.2.0" }
        },
        "p-locate@4.1.0": {
          "integrity": "sha512-R79ZZ/0wAxKGu3oYMlz8jy/kbhsNrS7SKZ7PxEHBgJ5+F2mtFW2fK2cOtBh1cHYkQsbzFV7I+EoRKe6Yt0oK7A==",
          "dependencies": { "p-limit": "p-limit@2.3.0" }
        },
        "p-try@2.2.0": {
          "integrity": "sha512-R4nPAVTAU0B9D35/Gk3uJf/7XYbQcyohSKdvAxIRSNghFl4e71hVoGnBNQz9cWaXxO2I10KTC+3jMdvvoKw6dQ==",
          "dependencies": {}
        },
        "path-exists@4.0.0": {
          "integrity": "sha512-ak9Qy5Q7jYb2Wwcey5Fpvg2KoAc/ZIhLSLOSBmRmygPsGwkVVt0fZa0qrtMz+m6tJTAHfZQ8FnmB4MG4LWy7/w==",
          "dependencies": {}
        },
        "require-directory@2.1.1": {
          "integrity": "sha512-fGxEI7+wsG9xrvdjsrlmL22OMTTiHRwAMroiEeMgq8gzoLC/PQr7RsRDSTLUg/bZAZtF+TVIkHc6/4RIKrui+Q==",
          "dependencies": {}
        },
        "require-main-filename@2.0.0": {
          "integrity": "sha512-NKN5kMDylKuldxYLSUfrbo5Tuzh4hd+2E8NPPX02mZtn1VuREQToYe/ZdlJy+J3uCpfaiGF05e7B8W0iXbQHmg==",
          "dependencies": {}
        },
        "set-blocking@2.0.0": {
          "integrity": "sha512-KiKBS8AnWGEyLzofFfmvKwpdPzqiy16LvQfK3yv/fVH7Bj13/wl3JSR1J+rfgRE9q7xUJK4qvgS8raSOeLUehw==",
          "dependencies": {}
        },
        "string-width@2.1.1": {
          "integrity": "sha512-nOqH59deCq9SRHlxq1Aw85Jnt4w6KvLKqWVik6oA9ZklXLNIOlqg4F2yrT1MVaTjAqvVwdfeZ7w7aCvJD7ugkw==",
          "dependencies": {
            "is-fullwidth-code-point": "is-fullwidth-code-point@2.0.0",
            "strip-ansi": "strip-ansi@4.0.0"
          }
        },
        "string-width@4.2.3": {
          "integrity": "sha512-wKyQRQpjJ0sIp62ErSZdGsjMJWsap5oRNihHhu6G7JVO/9jIB6UyevL+tXuOqrng8j/cxKTWyWUwvSTriiZz/g==",
          "dependencies": {
            "emoji-regex": "emoji-regex@8.0.0",
            "is-fullwidth-code-point": "is-fullwidth-code-point@3.0.0",
            "strip-ansi": "strip-ansi@6.0.1"
          }
        },
        "strip-ansi@4.0.0": {
          "integrity": "sha512-4XaJ2zQdCzROZDivEVIDPkcQn8LMFSa8kj8Gxb/Lnwzv9A8VctNZ+lfivC/sV3ivW8ElJTERXZoPBRrZKkNKow==",
          "dependencies": { "ansi-regex": "ansi-regex@3.0.1" }
        },
        "strip-ansi@6.0.1": {
          "integrity": "sha512-Y38VPSHcqkFrCpFnQ9vuSXmquuv5oXOKpGeT6aGrr3o3Gc9AlVa6JBfUSOCnbxGGZF+/0ooI7KrPuUSztUdU5A==",
          "dependencies": { "ansi-regex": "ansi-regex@5.0.1" }
        },
        "strip-final-newline@2.0.0": {
          "integrity": "sha512-BrpvfNAE3dcvq7ll3xVumzjKjZQ5tI1sEUIKr3Uoks0XUl45St3FlatVqef9prk4jRDzhW6WZg+3bk93y6pLjA==",
          "dependencies": {}
        },
        "which-module@2.0.0": {
          "integrity": "sha512-B+enWhmw6cjfVC7kS8Pj9pCrKSc5txArRyaYGe088shv/FGWH+0Rjx/xPgtsWfsUtS27FkP697E4DDhgrgoc0Q==",
          "dependencies": {}
        },
        "wrap-ansi@6.2.0": {
          "integrity": "sha512-r6lPcBGxZXlIcymEu7InxDMhdW0KDxpLgoFLcguasxCaJ/SOIZwINatK9KY/tf+ZrlywOKU0UDj3ATXUBfxJXA==",
          "dependencies": {
            "ansi-styles": "ansi-styles@4.3.0",
            "string-width": "string-width@4.2.3",
            "strip-ansi": "strip-ansi@6.0.1"
          }
        },
        "y18n@4.0.3": {
          "integrity": "sha512-JKhqTOwSrqNA1NY5lSztJ1GrBiUodLMmIZuLiDaMRJ+itFd+ABVE8XBjOvIWL+rSqNDC74LCSFmlb/U4UZ4hJQ==",
          "dependencies": {}
        },
        "yargs-parser@18.1.3": {
          "integrity": "sha512-o50j0JeToy/4K6OZcaQmW6lyXXKhq7csREXcDwk2omFPJEwUNOVtJKvmDr9EI1fAJZUyZcRF7kxGBWmRXudrCQ==",
          "dependencies": {
            "camelcase": "camelcase@5.3.1",
            "decamelize": "decamelize@1.2.0"
          }
        },
        "yargs@15.4.1": {
          "integrity": "sha512-aePbxDmcYW++PaqBsJ+HYUFwCdv4LVvdnhBy78E57PIor8/OVvhMrADFFEDh8DHDFRv/O9i3lPhsENjO7QX0+A==",
          "dependencies": {
            "cliui": "cliui@6.0.0",
            "decamelize": "decamelize@1.2.0",
            "find-up": "find-up@4.1.0",
            "get-caller-file": "get-caller-file@2.0.5",
            "require-directory": "require-directory@2.1.1",
            "require-main-filename": "require-main-filename@2.0.0",
            "set-blocking": "set-blocking@2.0.0",
            "string-width": "string-width@4.2.3",
            "which-module": "which-module@2.0.0",
            "y18n": "y18n@4.0.3",
            "yargs-parser": "yargs-parser@18.1.3"
          }
        }
      }
    }
  }
  "#;
  temp_dir.write("ende.lock", lock_file_content);
  let main_contents = r#"
  import cowsay from "npm:cowsay";
  console.log(cowsay);
  "#;
  temp_dir.write("main.ts", main_contents);

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(temp_dir.path())
    .arg("run")
    .arg("--quiet")
    .arg("--lock")
    .arg("ende.lock")
    .arg("-A")
    .arg("main.ts")
    .envs(env_vars_for_npm_tests())
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  assert!(!output.status.success());

  let stderr = String::from_utf8(output.stderr).unwrap();
  test_util::assertions::assert_wildcard_match(
    &stderr,
    concat!(
      "error: failed reading lockfile '[WILDLINE]ende.lock'\n",
      "\n",
      "Caused by:\n",
      "    0: The lockfile is corrupt. Remove the lockfile to regenerate it.\n",
      "    1: Could not find 'cowsay@1.5.0' in the list of packages.\n"
    ),
  );
}

#[test]
fn lock_file_lock_write() {
  // https://github.com/denoland/deno/issues/16666
  let _server = http_server();

  let deno_dir = util::new_ende_dir();
  let temp_dir = util::TempDir::new();

  temp_dir.write("ende.json", "{}");
  let lock_file_content = r#"{
  "version": "5",
  "specifiers": {
    "npm:cowsay@1.5.0": "1.5.0"
  },
  "npm": {
    "ansi-regex@3.0.1": {
      "integrity": "sha512-+O9Jct8wf++lXxxFc4hc8LsjaSq0HFzzL7cVsw8pRDIPdjKD2mT4ytDZlLuSBZ4cLKZFXIrMGO7DbQCtMJJMKw=="
    },
    "ansi-regex@5.0.1": {
      "integrity": "sha512-quJQXlTSUGL2LH9SUXo8VwsY4soanhgo6LNSm84E1LBcE8s3O0wpdiRzyR9z/ZZJMlMWv37qOOb9pdJlMUEKFQ=="
    },
    "ansi-styles@4.3.0": {
      "integrity": "sha512-zbB9rCJAT1rbjiVDb2hqKFHNYLxgtk8NURxZ3IZwD3F6NtxbXZQCnnSi1Lkx+IDohdPlFp222wVALIheZJQSEg==",
      "dependencies": [
        "color-convert"
      ]
    },
    "camelcase@5.3.1": {
      "integrity": "sha512-L28STB170nwWS63UjtlEOE3dldQApaJXZkOI1uMFfzf3rRuPegHaHesyee+YxQ+W6SvRDQV6UrdOdRiR153wJg=="
    },
    "cliui@6.0.0": {
      "integrity": "sha512-t6wbgtoCXvAzst7QgXxJYqPt0usEfbgQdftEPbLL/cvv6HPE5VgvqCuAIDR0NgU52ds6rFwqrgakNLrHEjCbrQ==",
      "dependencies": [
        "string-width@4.2.3",
        "strip-ansi@6.0.1",
        "wrap-ansi"
      ]
    },
    "color-convert@2.0.1": {
      "integrity": "sha512-RRECPsj7iu/xb5oKYcsFHSppFNnsj/52OVTRKb4zP5onXwVF3zVmmToNcOfGC+CRDpfK/U584fMg38ZHCaElKQ==",
      "dependencies": [
        "color-name"
      ]
    },
    "color-name@1.1.4": {
      "integrity": "sha512-dOy+3AuW3a2wNbZHIuMZpTcgjGuLU/uBL/ubcZF9OXbDo8ff4O8yVp5Bf0efS8uEoYo5q4Fx7dY9OgQGXgAsQA=="
    },
    "cowsay@1.5.0": {
      "integrity": "sha512-8Ipzr54Z8zROr/62C8f0PdhQcDusS05gKTS87xxdji8VbWefWly0k8BwGK7+VqamOrkv3eGsCkPtvlHzrhWsCA==",
      "dependencies": [
        "get-stdin",
        "string-width@2.1.1",
        "strip-final-newline",
        "yargs"
      ]
    },
    "decamelize@1.2.0": {
      "integrity": "sha512-z2S+W9X73hAUUki+N+9Za2lBlun89zigOyGrsax+KUQ6wKW4ZoWpEYBkGhQjwAjjDCkWxhY0VKEhk8wzY7F5cA=="
    },
    "emoji-regex@8.0.0": {
      "integrity": "sha512-MSjYzcWNOA0ewAHpz0MxpYFvwg6yjy1NG3xteoqz644VCo/RPgnr1/GGt+ic3iJTzQ8Eu3TdM14SawnVUmGE6A=="
    },
    "find-up@4.1.0": {
      "integrity": "sha512-PpOwAdQ/YlXQ2vj8a3h8IipDuYRi3wceVQQGYWxNINccq40Anw7BlsEXCMbt1Zt+OLA6Fq9suIpIWD0OsnISlw==",
      "dependencies": [
        "locate-path",
        "path-exists"
      ]
    },
    "get-caller-file@2.0.5": {
      "integrity": "sha512-DyFP3BM/3YHTQOCUL/w0OZHR0lpKeGrxotcHWcqNEdnltqFwXVfhEBQ94eIo34AfQpo0rGki4cyIiftY06h2Fg=="
    },
    "get-stdin@8.0.0": {
      "integrity": "sha512-sY22aA6xchAzprjyqmSEQv4UbAAzRN0L2dQB0NlN5acTTK9Don6nhoc3eAbUnpZiCANAMfd/+40kVdKfFygohg=="
    },
    "is-fullwidth-code-point@2.0.0": {
      "integrity": "sha512-VHskAKYM8RfSFXwee5t5cbN5PZeq1Wrh6qd5bkyiXIf6UQcN6w/A0eXM9r6t8d+GYOh+o6ZhiEnb88LN/Y8m2w=="
    },
    "is-fullwidth-code-point@3.0.0": {
      "integrity": "sha512-zymm5+u+sCsSWyD9qNaejV3DFvhCKclKdizYaJUuHA83RLjb7nSuGnddCHGv0hk+KY7BMAlsWeK4Ueg6EV6XQg=="
    },
    "locate-path@5.0.0": {
      "integrity": "sha512-t7hw9pI+WvuwNJXwk5zVHpyhIqzg2qTlklJOf0mVxGSbe3Fp2VieZcduNYjaLDoy6p9uGpQEGWG87WpMKlNq8g==",
      "dependencies": [
        "p-locate"
      ]
    },
    "p-limit@2.3.0": {
      "integrity": "sha512-//88mFWSJx8lxCzwdAABTJL2MyWB12+eIY7MDL2SqLmAkeKU9qxRvWuSyTjm3FUmpBEMuFfckAIqEaVGUDxb6w==",
      "dependencies": [
        "p-try"
      ]
    },
    "p-locate@4.1.0": {
      "integrity": "sha512-R79ZZ/0wAxKGu3oYMlz8jy/kbhsNrS7SKZ7PxEHBgJ5+F2mtFW2fK2cOtBh1cHYkQsbzFV7I+EoRKe6Yt0oK7A==",
      "dependencies": [
        "p-limit"
      ]
    },
    "p-try@2.2.0": {
      "integrity": "sha512-R4nPAVTAU0B9D35/Gk3uJf/7XYbQcyohSKdvAxIRSNghFl4e71hVoGnBNQz9cWaXxO2I10KTC+3jMdvvoKw6dQ=="
    },
    "path-exists@4.0.0": {
      "integrity": "sha512-ak9Qy5Q7jYb2Wwcey5Fpvg2KoAc/ZIhLSLOSBmRmygPsGwkVVt0fZa0qrtMz+m6tJTAHfZQ8FnmB4MG4LWy7/w=="
    },
    "require-directory@2.1.1": {
      "integrity": "sha512-fGxEI7+wsG9xrvdjsrlmL22OMTTiHRwAMroiEeMgq8gzoLC/PQr7RsRDSTLUg/bZAZtF+TVIkHc6/4RIKrui+Q=="
    },
    "require-main-filename@2.0.0": {
      "integrity": "sha512-NKN5kMDylKuldxYLSUfrbo5Tuzh4hd+2E8NPPX02mZtn1VuREQToYe/ZdlJy+J3uCpfaiGF05e7B8W0iXbQHmg=="
    },
    "set-blocking@2.0.0": {
      "integrity": "sha512-KiKBS8AnWGEyLzofFfmvKwpdPzqiy16LvQfK3yv/fVH7Bj13/wl3JSR1J+rfgRE9q7xUJK4qvgS8raSOeLUehw=="
    },
    "string-width@2.1.1": {
      "integrity": "sha512-nOqH59deCq9SRHlxq1Aw85Jnt4w6KvLKqWVik6oA9ZklXLNIOlqg4F2yrT1MVaTjAqvVwdfeZ7w7aCvJD7ugkw==",
      "dependencies": [
        "is-fullwidth-code-point@2.0.0",
        "strip-ansi@4.0.0"
      ]
    },
    "string-width@4.2.3": {
      "integrity": "sha512-wKyQRQpjJ0sIp62ErSZdGsjMJWsap5oRNihHhu6G7JVO/9jIB6UyevL+tXuOqrng8j/cxKTWyWUwvSTriiZz/g==",
      "dependencies": [
        "emoji-regex",
        "is-fullwidth-code-point@3.0.0",
        "strip-ansi@6.0.1"
      ]
    },
    "strip-ansi@4.0.0": {
      "integrity": "sha512-4XaJ2zQdCzROZDivEVIDPkcQn8LMFSa8kj8Gxb/Lnwzv9A8VctNZ+lfivC/sV3ivW8ElJTERXZoPBRrZKkNKow==",
      "dependencies": [
        "ansi-regex@3.0.1"
      ]
    },
    "strip-ansi@6.0.1": {
      "integrity": "sha512-Y38VPSHcqkFrCpFnQ9vuSXmquuv5oXOKpGeT6aGrr3o3Gc9AlVa6JBfUSOCnbxGGZF+/0ooI7KrPuUSztUdU5A==",
      "dependencies": [
        "ansi-regex@5.0.1"
      ]
    },
    "strip-final-newline@2.0.0": {
      "integrity": "sha512-BrpvfNAE3dcvq7ll3xVumzjKjZQ5tI1sEUIKr3Uoks0XUl45St3FlatVqef9prk4jRDzhW6WZg+3bk93y6pLjA=="
    },
    "which-module@2.0.0": {
      "integrity": "sha512-B+enWhmw6cjfVC7kS8Pj9pCrKSc5txArRyaYGe088shv/FGWH+0Rjx/xPgtsWfsUtS27FkP697E4DDhgrgoc0Q=="
    },
    "wrap-ansi@6.2.0": {
      "integrity": "sha512-r6lPcBGxZXlIcymEu7InxDMhdW0KDxpLgoFLcguasxCaJ/SOIZwINatK9KY/tf+ZrlywOKU0UDj3ATXUBfxJXA==",
      "dependencies": [
        "ansi-styles",
        "string-width@4.2.3",
        "strip-ansi@6.0.1"
      ]
    },
    "y18n@4.0.3": {
      "integrity": "sha512-JKhqTOwSrqNA1NY5lSztJ1GrBiUodLMmIZuLiDaMRJ+itFd+ABVE8XBjOvIWL+rSqNDC74LCSFmlb/U4UZ4hJQ=="
    },
    "yargs-parser@18.1.3": {
      "integrity": "sha512-o50j0JeToy/4K6OZcaQmW6lyXXKhq7csREXcDwk2omFPJEwUNOVtJKvmDr9EI1fAJZUyZcRF7kxGBWmRXudrCQ==",
      "dependencies": [
        "camelcase",
        "decamelize"
      ]
    },
    "yargs@15.4.1": {
      "integrity": "sha512-aePbxDmcYW++PaqBsJ+HYUFwCdv4LVvdnhBy78E57PIor8/OVvhMrADFFEDh8DHDFRv/O9i3lPhsENjO7QX0+A==",
      "dependencies": [
        "cliui",
        "decamelize",
        "find-up",
        "get-caller-file",
        "require-directory",
        "require-main-filename",
        "set-blocking",
        "string-width@4.2.3",
        "which-module",
        "y18n",
        "yargs-parser"
      ]
    }
  }
}
"#;
  temp_dir.write("ende.lock", lock_file_content);

  let deno = util::ende_cmd_with_ende_dir(&deno_dir)
    .current_dir(temp_dir.path())
    .arg("cache")
    .arg("--quiet")
    .arg("npm:cowsay@1.5.0")
    .envs(env_vars_for_npm_tests())
    // This test relies on downloading the real cowsay tarballs (the lockfile
    // omits the tarball URLs, so they resolve to `registry.npmjs.org`). Point
    // the registry at the real npm registry so those downloads aren't relocated
    // to the (private) test registry, which doesn't serve these packages.
    .env("NPM_CONFIG_REGISTRY", "https://registry.npmjs.org/")
    .piped_output()
    .spawn()
    .unwrap();
  let output = deno.wait_with_output().unwrap();
  eprintln!(
    "output: {}",
    String::from_utf8(output.stderr.clone()).unwrap()
  );
  assert!(output.status.success());
  assert_eq!(output.status.code(), Some(0));

  let stdout = String::from_utf8(output.stdout).unwrap();
  assert!(stdout.is_empty());
  let stderr = String::from_utf8(output.stderr).unwrap();
  assert!(stderr.is_empty());
  assert_eq!(
    std::fs::read_to_string(temp_dir.path().join("ende.lock")).unwrap(),
    lock_file_content,
  );
}

#[test]
fn auto_discover_lock_file() {
  let context = TestContextBuilder::for_npm().use_temp_cwd().build();

  let temp_dir = context.temp_dir();

  // write empty config file
  temp_dir.write("ende.json", "{}");

  // write a lock file with borked integrity
  let lock_file_content = r#"{
    "version": "5",
    "specifiers": {
      "npm:@denotest/bin": "1.0.0"
    },
    "npm": {
      "@denotest/bin@1.0.0": {
        "integrity": "sha512-foobar",
        "tarball": "http://localhost:4260/@denotest/bin/1.0.0.tgz"
      }
    }
  }"#;
  temp_dir.write("ende.lock", lock_file_content);

  let output = context
    .new_command()
    .args("run -A npm:@denotest/bin/cli-esm test")
    .run();
  output
    .assert_matches_text(
r#"Download http://localhost:4260/@denotest/bin/1.0.0.tgz
error: Failed caching npm package '@denotest/bin@1.0.0'

Caused by:
    Tarball checksum did not match what was provided by npm registry for @denotest/bin@1.0.0.
    
    Expected: foobar
    Actual: [WILDCARD]
"#)
    .assert_exit_code(1);
}

#[test]
fn reload_info_not_found_cache_but_exists_remote() {
  fn remove_version(registry_json: &mut Value, version: &str) {
    registry_json
      .as_object_mut()
      .unwrap()
      .get_mut("versions")
      .unwrap()
      .as_object_mut()
      .unwrap()
      .remove(version);
  }

  fn remove_version_for_package(
    deno_dir: &util::TempDir,
    package: &str,
    version: &str,
  ) {
    let registry_json_path =
      format!("npm/localhost_4260/{}/registry.json", package);
    let mut registry_json: Value =
      serde_json::from_str(&deno_dir.read_to_string(&registry_json_path))
        .unwrap();
    remove_version(&mut registry_json, version);
    // for the purpose of this test, just remove the dist-tag as it might contain this version
    registry_json
      .as_object_mut()
      .unwrap()
      .get_mut("dist-tags")
      .unwrap()
      .as_object_mut()
      .unwrap()
      .remove("latest");
    // remove the etag so the cache is invalidated and re-fetched
    registry_json.as_object_mut().unwrap().remove("_deno.etag");
    deno_dir.write(
      &registry_json_path,
      serde_json::to_string(&registry_json).unwrap(),
    );
  }

  // This tests that when a local machine doesn't have a version
  // specified in a dependency that exists in the npm registry
  let test_context = TestContextBuilder::for_npm().use_temp_cwd().build();
  let deno_dir = test_context.ende_dir();
  let temp_dir = test_context.temp_dir();
  temp_dir.write(
    "main.ts",
    "import 'npm:@denotest/esm-import-cjs-default@1.0.0';",
  );

  // cache successfully to the deno_dir
  let output = test_context
    .new_command()
    .args("cache main.ts npm:@denotest/esm-basic@1.0.0")
    .run();
  output.assert_matches_text(concat!(
    "[UNORDERED_START]\n",
    "Download http://localhost:4260/@denotest%2fesm-basic\n",
    "Download http://localhost:4260/@denotest%2fesm-import-cjs-default\n",
    "Download http://localhost:4260/@denotest%2fcjs-default-export\n",
    "Download http://localhost:4260/@denotest/cjs-default-export/1.0.0.tgz\n",
    "Download http://localhost:4260/@denotest/esm-basic/1.0.0.tgz\n",
    "Download http://localhost:4260/@denotest/esm-import-cjs-default/1.0.0.tgz\n",
    "[UNORDERED_END]\n",
    "[WILDCARD]",
  ));

  // test in dependency
  {
    // modify the package information in the cache to remove the latest version
    remove_version_for_package(
      deno_dir,
      "@denotest/cjs-default-export",
      "1.0.0",
    );

    // should error when `--cache-only` is used now because the version is not in the cache
    let output = test_context
      .new_command()
      .args("run --cached-only main.ts")
      .run();
    output.assert_exit_code(1);
    output.assert_matches_text("error: Could not find npm package '@denotest/cjs-default-export' matching '^1.0.0'.\n");

    // now try running without it, it should download the package now
    let output = test_context.new_command().args("run main.ts").run();
    output.assert_matches_text(concat!(
      "[UNORDERED_START]\n",
      "Download http://localhost:4260/@denotest%2fesm-import-cjs-default\n",
      "Download http://localhost:4260/@denotest%2fcjs-default-export\n",
      "[UNORDERED_END]\n",
      "Node esm importing node cjs\n[WILDCARD]",
    ));
    output.assert_exit_code(0);
  }

  // test in npm specifier
  {
    // now remove the information for the top level package
    remove_version_for_package(
      deno_dir,
      "@denotest/esm-import-cjs-default",
      "1.0.0",
    );

    // should error for --cached-only
    let output = test_context
      .new_command()
      .args("run --cached-only main.ts")
      .run();
    output.assert_matches_text(concat!(
      "error: Could not find npm package '@denotest/esm-import-cjs-default' matching '1.0.0'.\n",
      "    at file:///[WILDCARD]/main.ts:1:8\n",
    ));
    output.assert_exit_code(1);

    // now try running, it should work
    let output = test_context.new_command().args("run main.ts").run();
    output.assert_matches_text(concat!(
      "[UNORDERED_START]\n",
      "Download http://localhost:4260/@denotest%2fesm-import-cjs-default\n",
      "Download http://localhost:4260/@denotest%2fcjs-default-export\n",
      "[UNORDERED_END]\n",
      "Node esm importing node cjs\n[WILDCARD]",
    ));
    output.assert_exit_code(0);
  }
}

#[test]
fn check_css_package_json_exports() {
  let test_context = TestContextBuilder::for_npm().use_temp_cwd().build();
  let dir = test_context.temp_dir();
  dir.write(
    "main.ts",
    r#"import "npm:@denotest/css-export/dist/index.css";"#,
  );
  test_context
    .new_command()
    .args("check main.ts")
    .run()
    .assert_matches_text("Download [WILDCARD]css-export\nDownload [WILDCARD]css-export/1.0.0.tgz\nCheck main.ts\n")
    .assert_exit_code(0);
}

#[test]
async fn test_private_npm_registry() {
  let _server = http_server();

  // For now just check that private server rejects requests without proper
  // auth header.

  let client = reqwest::Client::new();

  let url = Url::parse("http://127.0.0.1:4261/@denotest/basic").unwrap();

  let req = reqwest::Request::new(reqwest::Method::GET, url.clone());
  let resp = client.execute(req).await.unwrap();
  assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

  let mut req = reqwest::Request::new(reqwest::Method::GET, url.clone());
  req.headers_mut().insert(
    reqwest::header::AUTHORIZATION,
    reqwest::header::HeaderValue::from_static("Bearer private-reg-token"),
  );
  let resp = client.execute(req).await.unwrap();
  assert_eq!(resp.status(), reqwest::StatusCode::OK);
}
