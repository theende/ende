// Copyright 2018-2026 the Deno authors. MIT license.

use std::os::unix::fs::PermissionsExt;

use test_util as util;
use test_util::test;
use util::TestContextBuilder;

/// `lint --fix` replaces the file atomically, so it has to carry the
/// original permission bits over, unchanged by the umask.
#[test]
fn lint_fix_preserves_permissions() {
  let context = TestContextBuilder::new().use_temp_cwd().build();
  let temp_dir = context.temp_dir().path();
  let modes = [0o755, 0o775, 0o664, 0o600];
  for (i, mode) in modes.iter().enumerate() {
    let file_path = temp_dir.join(format!("{i}.ts"));
    file_path.write("window.foo = 1;\n");
    std::fs::set_permissions(
      &file_path,
      std::fs::Permissions::from_mode(*mode),
    )
    .unwrap();
  }

  let output = context.new_command().args("lint --fix").run();
  output.assert_exit_code(0);
  output.assert_matches_text("Checked 4 files\n");

  for (i, mode) in modes.iter().enumerate() {
    let file_path = temp_dir.join(format!("{i}.ts"));
    assert_eq!(file_path.read_to_string(), "globalThis.foo = 1;\n");
    let actual = std::fs::metadata(&file_path).unwrap().permissions().mode();
    assert_eq!(actual & 0o7777, *mode, "{i}.ts");
  }
}
