// Copyright 2018-2026 the Deno authors. MIT license.

use std::env;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use ende_runtime::*;

fn compress_decls(out_dir: &Path) {
  let decls = [
    "lib.ende.ns.d.ts",
    "lib.ende.unstable.d.ts",
    "lib.ende.window.d.ts",
    "lib.ende.worker.d.ts",
    "lib.ende.shared_globals.d.ts",
    "lib.ende.desktop.d.ts",
    "lib.ende.unstable.d.ts",
    "lib.ende_console.d.ts",
    "lib.ende_url.d.ts",
    "lib.ende_web.d.ts",
    "lib.ende_fetch.d.ts",
    "lib.ende_websocket.d.ts",
    "lib.ende_webstorage.d.ts",
    "lib.ende_webgpu.d.ts",
    "lib.ende_canvas.d.ts",
    "lib.ende_crypto.d.ts",
    "lib.ende_cache.d.ts",
    "lib.ende_net.d.ts",
    "lib.ende_broadcast_channel.d.ts",
    "lib.decorators.d.ts",
    "lib.decorators.legacy.d.ts",
    "lib.dom.asynciterable.d.ts",
    "lib.dom.d.ts",
    "lib.dom.extras.d.ts",
    "lib.dom.iterable.d.ts",
    "lib.es2015.collection.d.ts",
    "lib.es2015.core.d.ts",
    "lib.es2015.d.ts",
    "lib.es2015.generator.d.ts",
    "lib.es2015.iterable.d.ts",
    "lib.es2015.promise.d.ts",
    "lib.es2015.proxy.d.ts",
    "lib.es2015.reflect.d.ts",
    "lib.es2015.symbol.d.ts",
    "lib.es2015.symbol.wellknown.d.ts",
    "lib.es2016.array.include.d.ts",
    "lib.es2016.d.ts",
    "lib.es2016.full.d.ts",
    "lib.es2016.intl.d.ts",
    "lib.es2017.arraybuffer.d.ts",
    "lib.es2017.d.ts",
    "lib.es2017.date.d.ts",
    "lib.es2017.full.d.ts",
    "lib.es2017.intl.d.ts",
    "lib.es2017.object.d.ts",
    "lib.es2017.sharedmemory.d.ts",
    "lib.es2017.string.d.ts",
    "lib.es2017.typedarrays.d.ts",
    "lib.es2018.asyncgenerator.d.ts",
    "lib.es2018.asynciterable.d.ts",
    "lib.es2018.d.ts",
    "lib.es2018.full.d.ts",
    "lib.es2018.intl.d.ts",
    "lib.es2018.promise.d.ts",
    "lib.es2018.regexp.d.ts",
    "lib.es2019.array.d.ts",
    "lib.es2019.d.ts",
    "lib.es2019.full.d.ts",
    "lib.es2019.intl.d.ts",
    "lib.es2019.object.d.ts",
    "lib.es2019.string.d.ts",
    "lib.es2019.symbol.d.ts",
    "lib.es2020.bigint.d.ts",
    "lib.es2020.d.ts",
    "lib.es2020.date.d.ts",
    "lib.es2020.full.d.ts",
    "lib.es2020.intl.d.ts",
    "lib.es2020.number.d.ts",
    "lib.es2020.promise.d.ts",
    "lib.es2020.sharedmemory.d.ts",
    "lib.es2020.string.d.ts",
    "lib.es2020.symbol.wellknown.d.ts",
    "lib.es2021.d.ts",
    "lib.es2021.full.d.ts",
    "lib.es2021.intl.d.ts",
    "lib.es2021.promise.d.ts",
    "lib.es2021.string.d.ts",
    "lib.es2021.weakref.d.ts",
    "lib.es2022.array.d.ts",
    "lib.es2022.d.ts",
    "lib.es2022.error.d.ts",
    "lib.es2022.full.d.ts",
    "lib.es2022.intl.d.ts",
    "lib.es2022.object.d.ts",
    "lib.es2022.regexp.d.ts",
    "lib.es2022.sharedmemory.d.ts",
    "lib.es2022.string.d.ts",
    "lib.es2023.array.d.ts",
    "lib.es2023.collection.d.ts",
    "lib.es2023.d.ts",
    "lib.es2023.full.d.ts",
    "lib.es2023.intl.d.ts",
    "lib.es2024.arraybuffer.d.ts",
    "lib.es2024.collection.d.ts",
    "lib.es2024.d.ts",
    "lib.es2024.full.d.ts",
    "lib.es2024.object.d.ts",
    "lib.es2024.promise.d.ts",
    "lib.es2024.regexp.d.ts",
    "lib.es2024.sharedmemory.d.ts",
    "lib.es2024.string.d.ts",
    "lib.es2025.collection.d.ts",
    "lib.es2025.d.ts",
    "lib.es2025.float16.d.ts",
    "lib.es2025.full.d.ts",
    "lib.es2025.intl.d.ts",
    "lib.es2025.iterator.d.ts",
    "lib.es2025.promise.d.ts",
    "lib.es2025.regexp.d.ts",
    "lib.es5.d.ts",
    "lib.es6.d.ts",
    "lib.esnext.array.d.ts",
    "lib.esnext.collection.d.ts",
    "lib.esnext.d.ts",
    "lib.esnext.date.d.ts",
    "lib.esnext.decorators.d.ts",
    "lib.esnext.disposable.d.ts",
    "lib.esnext.error.d.ts",
    "lib.esnext.full.d.ts",
    "lib.esnext.intl.d.ts",
    "lib.esnext.object.d.ts",
    "lib.esnext.promise.d.ts",
    "lib.esnext.regexp.d.ts",
    "lib.esnext.sharedmemory.d.ts",
    "lib.esnext.string.d.ts",
    "lib.esnext.temporal.d.ts",
    "lib.esnext.typedarrays.d.ts",
    "lib.scripthost.d.ts",
    "lib.webworker.asynciterable.d.ts",
    "lib.webworker.d.ts",
    "lib.webworker.importscripts.d.ts",
    "lib.webworker.iterable.d.ts",
  ];
  for decl in decls {
    let file = format!("./tsc/dts/{decl}");
    compress_source(out_dir, &file);
  }
}

fn compress_source(out_dir: &Path, file: &str) {
  #[allow(clippy::disallowed_methods, reason = "build code")]
  let path = Path::new(file)
    .canonicalize()
    .unwrap_or_else(|_| panic!("expected file \"{file}\" to exist"));
  let contents = std::fs::read(&path).unwrap();

  println!("cargo:rerun-if-changed={}", path.display());

  let compressed = zstd::bulk::compress(&contents, 19).unwrap();
  let mut out = out_dir.join(file.trim_start_matches("../"));
  let mut ext = out
    .extension()
    .map(|s| s.to_string_lossy())
    .unwrap_or_default()
    .into_owned();
  ext.push_str(".zstd");
  out.set_extension(ext);
  std::fs::create_dir_all(out.parent().unwrap()).unwrap();
  let mut file = std::fs::OpenOptions::new()
    .create(true)
    .truncate(true)
    .write(true)
    .open(out)
    .unwrap();
  file
    .write_all(&(contents.len() as u32).to_le_bytes())
    .unwrap();

  file.write_all(&compressed).unwrap();
}

fn compress_sources(out_dir: &Path) {
  compress_decls(out_dir);

  let ext_sources = [
    "./tsc/99_main_compiler.js",
    "./tsc/97_ts_host.js",
    "./tsc/98_lsp.js",
    "./tsc/00_typescript.js",
  ];
  for ext_source in ext_sources {
    compress_source(out_dir, ext_source);
  }
}

/// Read the pinned `laufey` capi crate version from the workspace Cargo.lock and
/// expose it as the `LAUFEY_VERSION` rustc env var. Desktop backend downloads are
/// resolved against `github.com/littledivy/laufey/releases/tag/v{LAUFEY_VERSION}`, so
/// tying this to Cargo.lock keeps a single source of truth.
///
/// Also asserts that `cli/laufey_sums.lock` (the trust anchor for those downloads)
/// pins the same version, failing the build if someone bumps the crate without
/// refreshing the digests. This is a pure consistency check between two
/// checked-in files, so it runs here rather than at runtime — a stale lock file
/// becomes a compile error instead of a first-launch failure.
fn emit_laufey_version() {
  let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
  let workspace_lock = Path::new(&manifest_dir).join("../Cargo.lock");
  println!("cargo:rerun-if-changed={}", workspace_lock.display());

  // Prefer the workspace Cargo.lock as the source of truth: it pins the `laufey`
  // crate version and we cross-check `cli/laufey_sums.lock` against it.
  //
  // During `cargo publish` the crate is verified in an isolated package dir
  // where the workspace lock is absent. The crate-local `Cargo.lock` cargo
  // bundles into the tarball is no help here either: `laufey` is a dependency
  // of `endert_desktop`, not of the `ende` crate, so it never appears in the
  // `deno` crate's own resolved lockfile. Fall back to the `# version:`
  // directive committed in `cli/laufey_sums.lock`, which is always bundled into
  // the published tarball.
  if let Some(version) = laufey_version_from_lock(&workspace_lock) {
    println!("cargo:rustc-env=LAUFEY_VERSION={version}");
    check_laufey_pinned_sums_version(&manifest_dir, &version);
    return;
  }

  if let Some(version) = laufey_version_from_sums(&manifest_dir) {
    println!("cargo:rustc-env=LAUFEY_VERSION={version}");
  }
}

/// Parse the pinned `laufey` crate version out of a Cargo.lock file. Returns
/// `None` when the file is missing or has no `laufey` package entry.
fn laufey_version_from_lock(lock_path: &Path) -> Option<String> {
  let lock = std::fs::read_to_string(lock_path).ok()?;

  let mut in_laufey = false;
  for line in lock.lines() {
    if line == "name = \"laufey\"" {
      in_laufey = true;
      continue;
    }
    if in_laufey {
      if let Some(rest) = line.strip_prefix("version = \"")
        && let Some(version) = rest.strip_suffix('"')
      {
        return Some(version.to_string());
      }
      if line.starts_with("[[package]]") {
        return None;
      }
    }
  }
  None
}

/// Read the `# version: vX.Y.Z` directive from `cli/laufey_sums.lock`. This file
/// is committed and bundled into the published crate, so it is the version
/// source of truth during the `cargo publish` verification build where no
/// Cargo.lock pins `laufey`.
fn laufey_version_from_sums(manifest_dir: &str) -> Option<String> {
  let sums_path = Path::new(manifest_dir).join("laufey_sums.lock");
  println!("cargo:rerun-if-changed={}", sums_path.display());

  let sums = std::fs::read_to_string(&sums_path).ok()?;
  for line in sums.lines() {
    let Some(rest) = line.trim_start().strip_prefix('#') else {
      continue;
    };
    let Some(version) = rest.trim().strip_prefix("version:") else {
      continue;
    };
    let pinned = version.trim().trim_start_matches('v');
    if !pinned.is_empty() {
      return Some(pinned.to_string());
    }
  }
  None
}

/// Confirm `cli/laufey_sums.lock` targets `laufey_version`. The lock file carries a
/// `# version: vX.Y.Z` directive that must match the `laufey` crate version
/// the binary is built against; a mismatch means the pinned digests are stale.
fn check_laufey_pinned_sums_version(manifest_dir: &str, laufey_version: &str) {
  let Some(pinned) = laufey_version_from_sums(manifest_dir) else {
    panic!(
      "cli/laufey_sums.lock has no pinned laufey version — populate it for \
       v{laufey_version} before building"
    );
  };
  if pinned != laufey_version {
    panic!(
      "cli/laufey_sums.lock pins Laufey v{pinned} but this build expects \
       v{laufey_version} — refresh the lock file from the upstream SHA256SUMS"
    );
  }
}

/// SHA-256 digests of the vendored AppImage Type-2 runtime stubs (from
/// `cli/tools/appimage_runtime/README.md`). Verified at build time so a
/// silent local modification (or a bad rebase) of those checked-in binaries
/// can't slip into a release build undetected.
const APPIMAGE_RUNTIME_HASHES: &[(&str, &str)] = &[
  (
    "tools/appimage_runtime/runtime-x86_64",
    "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
  ),
  (
    "tools/appimage_runtime/runtime-aarch64",
    "00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444",
  ),
];

fn check_appimage_runtime_hashes() {
  use sha2::Digest;
  let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
  for (rel, expected) in APPIMAGE_RUNTIME_HASHES {
    let path = Path::new(&manifest_dir).join(rel);
    println!("cargo:rerun-if-changed={}", path.display());
    let bytes = match std::fs::read(&path) {
      Ok(b) => b,
      Err(_) => continue,
    };
    let actual = format!("{:x}", sha2::Sha256::digest(&bytes));
    if actual != *expected {
      panic!(
        "checked-in AppImage runtime stub {} does not match the SHA-256 \
         pinned in cli/tools/appimage_runtime/README.md (expected {expected}, got {actual}). \
         If this update is intentional, refresh both the binary and the README.",
        path.display()
      );
    }
  }
}

fn compress_appimage_runtimes(out_dir: &Path) {
  let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
  let output_dir = out_dir.join("appimage_runtime");
  std::fs::create_dir_all(&output_dir).unwrap();

  for (rel, _) in APPIMAGE_RUNTIME_HASHES {
    let path = Path::new(&manifest_dir).join(rel);
    let contents = std::fs::read(&path).unwrap();
    let compressed = zstd::bulk::compress(&contents, 19).unwrap();
    let mut output = Vec::with_capacity(4 + compressed.len());
    output.extend_from_slice(&(contents.len() as u32).to_le_bytes());
    output.extend_from_slice(&compressed);

    let file_name = path.file_name().unwrap().to_string_lossy();
    std::fs::write(output_dir.join(format!("{file_name}.zstd")), output)
      .unwrap();
  }
}

/// Embeds the pinned TypeScript 7 compiler for `--unstable-tsc7` when
/// `ENDE_TSC7_DIR` points at a target directory of the tools/tsc7_mirror.ts
/// output (`tsc/<version>/<target>/`: `tsc[.exe]`, the `lib.*.d.ts` files,
/// LICENSE and NOTICE.txt). The files are checked against cli/tsc7_sums.lock
/// for this build's target and embedded as one zstd-compressed tar, which
/// cli/tsc/tsc7.rs extracts to the cache. Without `ENDE_TSC7_DIR` nothing is
/// embedded. Either way it writes `$OUT_DIR/tsc7_bundle.rs`, which defines
/// `BUNDLE`. It never downloads anything.
fn embed_tsc7(out_dir: &Path) {
  use sha2::Digest;

  const DIR_ENV: &str = "ENDE_TSC7_DIR";
  println!("cargo:rerun-if-env-changed={DIR_ENV}");
  let generated = out_dir.join("tsc7_bundle.rs");
  let Some(dir) = env::var_os(DIR_ENV).map(PathBuf::from) else {
    std::fs::write(
      &generated,
      "pub const BUNDLE: Option<Tsc7Bundle> = None;\n",
    )
    .unwrap();
    return;
  };

  let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
  let lock_path = Path::new(&manifest_dir).join("tsc7_sums.lock");
  println!("cargo:rerun-if-changed={}", lock_path.display());
  let lock = std::fs::read_to_string(&lock_path).unwrap();
  let directive = |name: &str| {
    lock.lines().find_map(|line| {
      let rest = line.trim().strip_prefix('#')?.trim();
      Some(rest.strip_prefix(name)?.trim().to_string())
    })
  };
  let version = directive("version:")
    .unwrap_or_else(|| panic!("cli/tsc7_sums.lock has no `# version:`"));
  let files_sha256 = directive("files-sha256:")
    .unwrap_or_else(|| panic!("cli/tsc7_sums.lock has no `# files-sha256:`"));
  let target = env::var("TARGET").unwrap();
  let tsc_sha256 = lock
    .lines()
    .map(|line| line.split_whitespace().collect::<Vec<_>>())
    .find(|fields| fields.len() == 4 && fields[0] == target)
    .map(|fields| fields[3].to_string())
    .unwrap_or_else(|| {
      panic!("cli/tsc7_sums.lock pins no TypeScript 7 compiler for {target}")
    });
  let exe_name = if target.contains("windows") {
    "tsc.exe"
  } else {
    "tsc"
  };

  println!("cargo:rerun-if-changed={}", dir.display());
  let mut names = std::fs::read_dir(&dir)
    .unwrap_or_else(|e| panic!("{DIR_ENV}={}: {e}", dir.display()))
    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
    .collect::<Vec<_>>();
  names.sort();
  let mut files = Vec::with_capacity(names.len());
  for name in names {
    let path = dir.join(&name);
    let contents = std::fs::read(&path)
      .unwrap_or_else(|e| panic!("{DIR_ENV}: {}: {e}", path.display()));
    files.push((name, contents));
  }

  let hex = |bytes: &[u8]| format!("{:x}", sha2::Sha256::digest(bytes));
  let Some((_, tsc)) = files.iter().find(|(name, _)| name == exe_name) else {
    panic!("{DIR_ENV}={} has no {exe_name}", dir.display());
  };
  let actual = hex(tsc);
  if actual != tsc_sha256 {
    panic!(
      "{DIR_ENV}: {exe_name} has SHA-256 {actual}, but cli/tsc7_sums.lock \
       pins {tsc_sha256} for {target} (is {} the {target} directory of \
       the TypeScript {version} mirror?)",
      dir.display()
    );
  }
  let tsc_len = tsc.len();
  let mut manifest = String::new();
  for (name, contents) in files.iter().filter(|(name, _)| name != exe_name) {
    manifest.push_str(&format!("{}  {name}\n", hex(contents)));
  }
  let actual = hex(manifest.as_bytes());
  if actual != files_sha256 {
    panic!(
      "{DIR_ENV}: the files next to {exe_name} in {} have manifest SHA-256 \
       {actual}, but cli/tsc7_sums.lock pins {files_sha256}",
      dir.display()
    );
  }

  let mut tar = tar::Builder::new(Vec::new());
  for (name, contents) in &files {
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(if name == exe_name { 0o755 } else { 0o644 });
    header.set_mtime(0);
    tar
      .append_data(&mut header, name, contents.as_slice())
      .unwrap();
  }
  let tar = tar.into_inner().unwrap();
  let archive = zstd::bulk::compress(&tar, 19).unwrap();
  let archive_path = out_dir.join("tsc7.tar.zst");
  std::fs::write(&archive_path, &archive).unwrap();
  let archive_hash = &hex(&archive)[..16];

  std::fs::write(
    &generated,
    format!(
      "pub const BUNDLE: Option<Tsc7Bundle> = Some(Tsc7Bundle {{
  version: {version:?},
  archive: include_bytes!({archive_path:?}),
  archive_hash: {archive_hash:?},
  exe_name: {exe_name:?},
  tsc_sha256: {tsc_sha256:?},
  tsc_len: {tsc_len},
}});
"
    ),
  )
  .unwrap();
}

fn emit_dts_rerun_if_changed() {
  let dts_dir = Path::new("tsc/dts");
  for entry in std::fs::read_dir(dts_dir).unwrap() {
    let entry = entry.unwrap();
    let path = entry.path();
    if path.extension().and_then(|s| s.to_str()) == Some("ts") {
      println!("cargo:rerun-if-changed={}", path.display());
    }
  }
}

#[allow(clippy::disallowed_methods, reason = "build code")]
fn emit_startup_order_link_args(out_dir: &Path) {
  const ENABLE_ENV: &str = "ENDE_USE_STARTUP_ORDER";
  const ORDER_FILE_ENV: &str = "ENDE_STARTUP_ORDER_FILE";

  println!("cargo:rerun-if-env-changed={ENABLE_ENV}");
  println!("cargo:rerun-if-env-changed={ORDER_FILE_ENV}");

  if env::var_os(ENABLE_ENV).is_none() {
    return;
  }
  if env::var(ENABLE_ENV).ok().as_deref() != Some("1") {
    panic!("{ENABLE_ENV} must be unset or set to 1");
  }

  let profile = env::var("PROFILE").unwrap();
  if profile != "release" {
    panic!("startup ordering is only supported by the release profile");
  }

  let target = env::var("TARGET").unwrap();
  match target.as_str() {
    "aarch64-apple-darwin"
    | "aarch64-unknown-linux-gnu"
    | "x86_64-unknown-linux-gnu" => {}
    _ => return,
  }

  let source =
    PathBuf::from(env::var_os(ORDER_FILE_ENV).unwrap_or_else(|| {
      panic!(
        "{ORDER_FILE_ENV} must point to an order generated from the baseline \
       release binary using the default linker layout"
      )
    }));
  let source = source.canonicalize().unwrap_or_else(|error| {
    panic!(
      "failed to resolve startup order {}: {error}",
      source.display()
    )
  });
  println!("cargo:rerun-if-changed={}", source.display());

  let contents = if source.extension().is_some_and(|ext| ext == "zst") {
    let compressed = std::fs::read(&source).unwrap_or_else(|error| {
      panic!("failed to read startup order {}: {error}", source.display())
    });
    zstd::stream::decode_all(compressed.as_slice()).unwrap_or_else(|error| {
      panic!(
        "failed to decompress startup order {}: {error}",
        source.display()
      )
    })
  } else {
    std::fs::read(&source).unwrap_or_else(|error| {
      panic!("failed to read startup order {}: {error}", source.display())
    })
  };

  // Always use the same linker path for generated orders. Full-LTO output can
  // change when only the order-file path changes.
  let order_file = out_dir.join(format!("startup-order-{target}.order"));
  std::fs::write(&order_file, contents).unwrap_or_else(|error| {
    panic!(
      "failed to write startup order {}: {error}",
      order_file.display()
    )
  });

  let contents = std::fs::read_to_string(&order_file).unwrap_or_else(|error| {
    panic!(
      "failed to validate startup order {}: {error}",
      order_file.display()
    )
  });
  let symbol_count = contents
    .lines()
    .filter(|line| !line.is_empty() && !line.starts_with('#'))
    .count();
  if symbol_count < 1_000 {
    panic!(
      "startup order {} has only {symbol_count} symbols",
      order_file.display()
    );
  }

  let order_file = order_file.canonicalize().unwrap();
  match target.as_str() {
    "aarch64-apple-darwin" => println!(
      "cargo:rustc-link-arg-bin=ende=-Wl,-order_file,{}",
      order_file.display()
    ),
    "aarch64-unknown-linux-gnu" | "x86_64-unknown-linux-gnu" => {
      println!(
        "cargo:rustc-link-arg-bin=ende=-Wl,--symbol-ordering-file={}",
        order_file.display()
      );
      println!("cargo:rustc-link-arg-bin=ende=-Wl,--no-warn-symbol-ordering");
    }
    _ => unreachable!(),
  }
}

fn main() {
  let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());

  check_appimage_runtime_hashes();
  compress_appimage_runtimes(&out_dir);
  embed_tsc7(&out_dir);

  // Skip building from docs.rs.
  if env::var_os("DOCS_RS").is_some() {
    return;
  }

  ende_webgpu::print_linker_flags("ende");

  // Host snapshots won't work when cross compiling.
  let target = env::var("TARGET").unwrap();
  let host = env::var("HOST").unwrap();
  let skip_cross_check =
    env::var("ENDE_SKIP_CROSS_BUILD_CHECK").is_ok_and(|v| v == "1");
  if !skip_cross_check && target != host {
    panic!("Cross compiling with snapshot is not supported.");
  }

  // To debug snapshot issues uncomment:
  // op_fetch_asset::trace_serializer();

  emit_startup_order_link_args(&out_dir);

  // Always emit rerun-if-changed for dts files (they are included via
  // include_str! in debug mode). Without this, cargo may use stale
  // cached artifacts when dts files change.
  emit_dts_rerun_if_changed();

  if !cfg!(debug_assertions) && std::env::var("CARGO_FEATURE_HMR").is_err() {
    compress_sources(&out_dir);
  }

  println!("cargo:rustc-env=TARGET={}", env::var("TARGET").unwrap());
  println!("cargo:rustc-env=PROFILE={}", env::var("PROFILE").unwrap());

  emit_laufey_version();

  #[cfg(target_os = "windows")]
  {
    let mut res = winres::WindowsResource::new();
    res.set_icon("deno.ico");
    // 0x0409 == MAKELANGID(LANG_ENGLISH, SUBLANG_ENGLISH_US)
    res.set_language(0x0409);
    res.compile().unwrap();
  }
}
