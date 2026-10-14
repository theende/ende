// Copyright 2018-2026 the Deno authors. MIT license.

use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;

use ende_core::Extension;
use ende_core::ExtensionFileSource;
use ende_core::ExtensionFileSourceCode;
use ende_core::snapshot::*;
use ende_core::v8;
use ende_resolver::npm::DenoInNpmPackageChecker;
use ende_resolver::npm::NpmResolver;

use crate::ops;
use crate::ops::bootstrap::SnapshotOptions;
use crate::shared::runtime;

/// A single lazy_loaded_* entry declared by an extension, in
/// `(specifier, absolute source path)` form. The build script uses this to
/// decide which sources still need to be embedded in the final binary.
#[derive(Clone, Debug)]
pub struct LazyExtensionFile {
  pub specifier: String,
  pub path: PathBuf,
  pub kind: LazyExtensionFileKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LazyExtensionFileKind {
  /// `lazy_loaded_js` — loaded on demand via `core.loadExtScript()`.
  Js,
  /// `lazy_loaded_esm` — loaded on demand via `op_lazy_load_esm` (i.e. the
  /// `createLazyLoader` factory exposed by `Ende.core`).
  Esm,
}

pub struct CreateRuntimeSnapshotOutput {
  /// Paths the snapshot read from disk; emit as `cargo:rerun-if-changed`.
  pub files_loaded_during_snapshot: Vec<PathBuf>,
  /// Specifiers of `lazy_loaded_*` files compiled into the snapshot blob.
  /// Their source already lives in the snapshot; no `include_str!` is needed.
  pub consumed_lazy_specifiers: Vec<String>,
  /// Every `lazy_loaded_*` file declared by any extension fed to the snapshot.
  /// The residual set the binary needs at runtime is
  /// `lazy_extension_files \ consumed_lazy_specifiers`.
  pub lazy_extension_files: Vec<LazyExtensionFile>,
}

pub fn create_runtime_snapshot(
  snapshot_path: PathBuf,
  snapshot_options: SnapshotOptions,
  // NOTE: For embedders that wish to add additional extensions to the snapshot
  custom_extensions: Vec<Extension>,
) -> CreateRuntimeSnapshotOutput {
  // NOTE(bartlomieju): ordering is important here, keep it in sync with
  // `runtime/worker.rs`, `runtime/web_worker.rs`, `runtime/snapshot_info.rs`
  // and `runtime/snapshot.rs`!
  let mut extensions: Vec<Extension> = vec![
    ende_telemetry::ende_telemetry::lazy_init(),
    ende_webidl::ende_webidl::lazy_init(),
    ende_web::ende_web::lazy_init(),
    ende_webgpu::ende_webgpu::lazy_init(),
    ende_image::ende_image::lazy_init(),
    ende_canvas::ende_canvas::lazy_init(),
    ende_fetch::ende_fetch::lazy_init(),
    ende_cache::ende_cache::lazy_init(),
    ende_websocket::ende_websocket::lazy_init(),
    ende_webstorage::ende_webstorage::lazy_init(),
    ende_crypto::ende_crypto::lazy_init(),
    ende_ffi::ende_ffi::lazy_init(),
    ende_net::ende_net::lazy_init(),
    ende_tls::ende_tls::lazy_init(),
    ende_kv::ende_kv::lazy_init(),
    ende_cron::ende_cron::init(Box::new(
      ende_cron::local::LocalCronHandler::new(),
    )),
    ende_napi::ende_napi::lazy_init(),
    ende_http::ende_http::lazy_init(),
    ende_io::ende_io::lazy_init(),
    ende_fs::ende_fs::lazy_init(),
    ende_os::ende_os::lazy_init(),
    ende_process::ende_process::lazy_init(),
    ende_node_crypto::ende_node_crypto::lazy_init(),
    ende_node_sqlite::ende_node_sqlite::lazy_init(),
    ende_node::ende_node::lazy_init::<
      DenoInNpmPackageChecker,
      NpmResolver<sys_traits::impls::RealSys>,
      sys_traits::impls::RealSys,
    >(),
    ops::runtime::ende_runtime::lazy_init(),
    ops::worker_host::ende_worker_host::lazy_init(),
    ops::fs_events::ende_fs_events::lazy_init(),
    ops::permissions::ende_permissions::lazy_init(),
    ops::tty::ende_tty::lazy_init(),
    ops::http::ende_http_runtime::lazy_init(),
    ops::desktop::ende_desktop::lazy_init(),
    ops::bootstrap::ende_bootstrap::init(Some(snapshot_options), false),
    runtime::lazy_init(),
    ops::web_worker::ende_web_worker::lazy_init(),
  ];
  extensions.extend(custom_extensions);

  let lazy_extension_files = collect_lazy_extension_files(&extensions);
  let minify_sources =
    std::env::var_os("ENDE_SNAPSHOT_MINIFY_SOURCES").is_some();

  let output = create_snapshot(
    CreateSnapshotOptions {
      cargo_manifest_dir: env!("CARGO_MANIFEST_DIR"),
      startup_snapshot: None,
      extensions,
      extension_transpiler: Some(Rc::new(move |specifier, source| {
        if minify_sources {
          crate::transpile::maybe_transpile_and_minify_source(specifier, source)
        } else {
          crate::transpile::maybe_transpile_source(specifier, source)
        }
      })),
      with_runtime_cb: Some(Box::new(|rt| {
        let isolate = rt.v8_isolate();
        v8::scope!(scope, isolate);

        let tmpl = ende_node::init_global_template(
          scope,
          ende_node::ContextInitMode::ForSnapshot,
        );
        let ctx = ende_node::create_v8_context(
          scope,
          tmpl,
          ende_node::ContextInitMode::ForSnapshot,
          std::ptr::null_mut(),
        );
        assert_eq!(scope.add_context(ctx), ende_node::VM_CONTEXT_INDEX);
      })),
      skip_op_registration: false,
    },
    None,
  )
  .unwrap();
  let mut snapshot = std::fs::File::create(snapshot_path).unwrap();
  snapshot.write_all(&output.output).unwrap();

  #[allow(clippy::print_stdout, reason = "necessary for build code")]
  for path in &output.files_loaded_during_snapshot {
    println!("cargo:rerun-if-changed={}", path.display());
  }

  CreateRuntimeSnapshotOutput {
    files_loaded_during_snapshot: output.files_loaded_during_snapshot,
    consumed_lazy_specifiers: output.consumed_lazy_specifiers,
    lazy_extension_files,
  }
}

fn collect_lazy_extension_files(
  extensions: &[Extension],
) -> Vec<LazyExtensionFile> {
  let mut out = Vec::new();
  for ext in extensions {
    for file in &*ext.lazy_loaded_js_files {
      if let Some(entry) = lazy_file_entry(file, LazyExtensionFileKind::Js) {
        out.push(entry);
      }
    }
    for file in &*ext.lazy_loaded_esm_files {
      if let Some(entry) = lazy_file_entry(file, LazyExtensionFileKind::Esm) {
        out.push(entry);
      }
    }
  }
  out.sort_by(|a, b| a.specifier.cmp(&b.specifier));
  out.dedup_by(|a, b| a.specifier == b.specifier);
  out
}

fn lazy_file_entry(
  file: &ExtensionFileSource,
  kind: LazyExtensionFileKind,
) -> Option<LazyExtensionFile> {
  #[allow(deprecated, reason = "matching deprecated variant we still inspect")]
  let path = match &file.code {
    ExtensionFileSourceCode::LoadedFromFsDuringSnapshot(p) => PathBuf::from(p),
    // After the macro change every lazy entry is LoadedFromFsDuringSnapshot;
    // the other variants only appear for ad-hoc entries pushed by customizers
    // (e.g. inline sources) that have no on-disk path and therefore can't be
    // re-embedded by the build script.
    ExtensionFileSourceCode::IncludedInBinary(_)
    | ExtensionFileSourceCode::LoadedFromMemoryDuringSnapshot(_)
    | ExtensionFileSourceCode::Computed(_) => return None,
  };
  Some(LazyExtensionFile {
    specifier: file.specifier.to_string(),
    path,
    kind,
  })
}
