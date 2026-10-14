// Copyright 2018-2026 the Deno authors. MIT license.

use std::io::Read;
use std::sync::Arc;

use deno_path_util::resolve_url_or_path;
use ende_cache_dir::file_fetcher::File;
use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_core::futures::FutureExt;
use ende_lib::worker::LibWorkerFactoryRoots;
use ende_runtime::WorkerExecutionMode;

use crate::args::EvalFlags;
use crate::args::Flags;
use crate::args::RunFlags;
use crate::args::WatchFlagsWithPaths;
use crate::factory::CliFactory;
use crate::util;
use crate::util::file_watcher::WatcherRestartMode;

pub mod hmr;

pub fn check_permission_before_script(flags: &Flags) {
  if !flags.has_permission() && flags.has_permission_in_argv() {
    log::warn!(
      "{}",
      crate::colors::yellow(
        r#"Permission flags have likely been incorrectly set after the script argument.
To grant permissions, set them before the script argument. For example:
    ende run --allow-read=. main.js"#
      )
    );
  }
}

pub async fn run_script(
  mode: WorkerExecutionMode,
  flags: Arc<Flags>,
  watch: Option<WatchFlagsWithPaths>,
  unconfigured_runtime: Option<ende_runtime::UnconfiguredRuntime>,
  roots: LibWorkerFactoryRoots,
) -> Result<i32, AnyError> {
  check_permission_before_script(&flags);

  if let Some(watch_flags) = watch {
    return run_with_watch(mode, flags, watch_flags).boxed_local().await;
  }

  // TODO(bartlomieju): actually I think it will also fail if there's an import
  // map specified and bare specifier is used on the command line
  crate::boot_phase("run_script start");
  let factory = CliFactory::from_flags(flags.clone());
  let cli_options = factory.cli_options()?;
  crate::boot_phase("after cli_options");
  let ende_dir = factory.ende_dir()?;
  let http_client = factory.http_client_provider();
  let workspace_resolver = factory.workspace_resolver().await?;
  crate::boot_phase("after workspace_resolver");
  // Run a background task that checks for available upgrades or output
  // if an earlier run of this background task found a new version of Ende.
  #[cfg(feature = "upgrade")]
  super::upgrade::check_for_upgrades(
    http_client.clone(),
    ende_dir.upgrade_check_file_path(),
  );

  let main_module_resolver =
    crate::args::WorkspaceMainModuleResolver::new(workspace_resolver.clone());
  let main_module = cli_options
    .resolve_main_module_with_resolver(Some(&main_module_resolver))?;
  let preload_modules =
    cli_options.preload_modules_with_resolver(Some(&main_module_resolver))?;

  crate::boot_phase("after resolve main_module");

  let worker_factory = factory
    .create_cli_main_worker_factory_with_roots(roots)
    .await?;
  crate::boot_phase("after worker_factory");
  let mut worker = worker_factory
    .create_main_worker_with_unconfigured_runtime(
      mode,
      main_module.clone(),
      preload_modules,
      unconfigured_runtime,
    )
    .inspect_err(|e| ende_telemetry::report_event("boot_failure", e))?;
  crate::boot_phase("after create_main_worker");

  let exit_code = worker
    .run()
    .await
    .inspect_err(|e| ende_telemetry::report_event("uncaught_exception", e))?;
  crate::boot_phase("after worker.run (exit)");
  Ok(exit_code)
}

pub async fn run_from_stdin(
  flags: Arc<Flags>,
  unconfigured_runtime: Option<ende_runtime::UnconfiguredRuntime>,
  roots: LibWorkerFactoryRoots,
) -> Result<i32, AnyError> {
  let factory = CliFactory::from_flags(flags);
  let cli_options = factory.cli_options()?;
  let main_module = cli_options.resolve_main_module()?;
  let preload_modules = cli_options.preload_modules()?;

  let file_fetcher = factory.file_fetcher()?;
  let worker_factory = factory
    .create_cli_main_worker_factory_with_roots(roots)
    .await?;
  let mut source = Vec::new();
  std::io::stdin().read_to_end(&mut source)?;
  // Save a fake file into file fetcher cache
  // to allow module access by TS compiler
  file_fetcher.insert_memory_files(File {
    url: main_module.clone(),
    mtime: None,
    maybe_headers: None,
    source: source.into(),
    loaded_from: ende_cache_dir::file_fetcher::LoadedFrom::Local,
  });

  let mut worker = worker_factory
    .create_main_worker_with_unconfigured_runtime(
      WorkerExecutionMode::Run,
      main_module.clone(),
      preload_modules,
      unconfigured_runtime,
    )?;
  let exit_code = worker.run().await?;
  Ok(exit_code)
}

// TODO(bartlomieju): this function is not handling `exit_code` set by the runtime
// code properly.
async fn run_with_watch(
  mode: WorkerExecutionMode,
  flags: Arc<Flags>,
  watch_flags: WatchFlagsWithPaths,
) -> Result<i32, AnyError> {
  util::file_watcher::watch_recv(
    flags,
    util::file_watcher::PrintConfig::new_with_banner(
      if watch_flags.hmr { "HMR" } else { "Watcher" },
      "Process",
      !watch_flags.no_clear_screen,
    ),
    WatcherRestartMode::Automatic,
    move |flags, watcher_communicator, changed_paths| {
      watcher_communicator.show_path_changed(changed_paths.clone());
      Ok(async move {
        let factory = CliFactory::from_flags_for_watcher(
          flags,
          watcher_communicator.clone(),
        );
        let cli_options = factory.cli_options()?;
        let main_module = cli_options.resolve_main_module()?;
        let preload_modules = cli_options.preload_modules()?;

        let _ = watcher_communicator.watch_paths(cli_options.watch_paths());

        let mut worker = factory
          .create_cli_main_worker_factory()
          .await?
          .create_main_worker(mode, main_module.clone(), preload_modules)?;

        let exit_code = if watch_flags.hmr {
          worker.run().await?
        } else {
          worker.run_for_watcher().await?
        };

        Ok(exit_code)
      })
    },
  )
  .boxed_local()
  .await?;

  Ok(0)
}

pub async fn eval_command(
  flags: Arc<Flags>,
  eval_flags: EvalFlags,
) -> Result<i32, AnyError> {
  let factory = CliFactory::from_flags(flags);
  let cli_options = factory.cli_options()?;
  let file_fetcher = factory.file_fetcher()?;
  let main_module = cli_options.resolve_main_module()?;
  let preload_modules = cli_options.preload_modules()?;

  // Create a dummy source file.
  let source_code = if eval_flags.print {
    format!("console.log({})", eval_flags.code)
  } else {
    eval_flags.code
  };
  // Match Node's `[eval]` URL for `-e` scripts so inspector clients (and
  // Node compat tests) see the same script URL. V8 honors the
  // `//# sourceURL=` comment for both classic scripts and ES modules.
  // Appended (not prepended) so user line numbers are preserved.
  let source_code = format!("{source_code}\n//# sourceURL=[eval]\n");

  // Save a fake file into file fetcher cache
  // to allow module access by TS compiler.
  file_fetcher.insert_memory_files(File {
    url: main_module.clone(),
    mtime: None,
    maybe_headers: None,
    source: source_code.into_bytes().into(),
    loaded_from: ende_cache_dir::file_fetcher::LoadedFrom::Local,
  });

  let worker_factory = factory.create_cli_main_worker_factory().await?;
  let mut worker = worker_factory.create_main_worker(
    WorkerExecutionMode::Eval,
    main_module.clone(),
    preload_modules,
  )?;
  let exit_code = worker.run().await?;
  Ok(exit_code)
}

pub async fn run_eszip(
  flags: Arc<Flags>,
  run_flags: RunFlags,
  unconfigured_runtime: Option<ende_runtime::UnconfiguredRuntime>,
  roots: LibWorkerFactoryRoots,
) -> Result<i32, AnyError> {
  // TODO(bartlomieju): actually I think it will also fail if there's an import
  // map specified and bare specifier is used on the command line
  let factory = CliFactory::from_flags(flags.clone());
  let cli_options = factory.cli_options()?;

  // entrypoint#path1,path2,...
  let (entrypoint, _files) = run_flags
    .script
    .split_once("#")
    .with_context(|| "eszip: invalid script string")?;

  let mode = WorkerExecutionMode::Run;
  let main_module = resolve_url_or_path(entrypoint, cli_options.initial_cwd())?;
  let preload_modules = cli_options.preload_modules()?;

  let worker_factory = factory
    .create_cli_main_worker_factory_with_roots(roots)
    .await?;
  let mut worker = worker_factory
    .create_main_worker_with_unconfigured_runtime(
      mode,
      main_module.clone(),
      preload_modules,
      unconfigured_runtime,
    )?;

  let exit_code = worker.run().await?;
  Ok(exit_code)
}
