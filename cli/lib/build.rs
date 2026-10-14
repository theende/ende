// Copyright 2018-2026 the Deno authors. MIT license.

#[allow(dead_code, reason = "only the parser is used here")]
#[path = "chronver.rs"]
mod chronver;

fn main() {
  // `version.txt` is the single source of truth for Ende's product version
  // (a ChronVer date, see `chronver.rs`). It is independent of the crate
  // versions in Cargo.toml; the release tooling in `tools/release` writes it.
  let version_path = std::path::Path::new(".").join("version.txt");
  println!("cargo:rerun-if-changed={}", version_path.display());
  #[allow(clippy::disallowed_methods, reason = "build code")]
  let text = std::fs::read_to_string(version_path).unwrap();
  let version = chronver::ChronVer::parse(text.trim())
    .unwrap_or_else(|err| panic!("cli/lib/version.txt: {err}"));
  println!("cargo:rustc-env=ENDE_VERSION={}", version);

  let commit_hash = git_commit_hash();
  println!("cargo:rustc-env=GIT_COMMIT_HASH={}", commit_hash);
  println!("cargo:rerun-if-env-changed=GIT_COMMIT_HASH");
}

fn git_commit_hash() -> String {
  if let Ok(output) = std::process::Command::new("git")
    .arg("rev-list")
    .arg("-1")
    .arg("HEAD")
    .output()
  {
    if output.status.success() {
      std::str::from_utf8(&output.stdout[..40])
        .unwrap()
        .to_string()
    } else {
      // When not in git repository
      // (e.g. when the user install by `cargo install ende`)
      "UNKNOWN".to_string()
    }
  } else {
    // When there is no git command for some reason
    "UNKNOWN".to_string()
  }
}
