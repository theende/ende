// Copyright 2018-2026 the Deno authors. MIT license.

mod args;
mod cache;
mod cdp;
mod factory;
mod file_fetcher;
mod graph_container;
mod graph_util;
mod http_util;
mod jsr;
mod lsp;
mod module_loader;
mod ops;
mod registry;
mod resolver;
mod standalone;
mod task_runner;
mod tools;
mod tsc;
mod type_checker;
mod util;
mod worker;

pub(crate) mod sys {
  #[allow(clippy::disallowed_types, reason = "definition")]
  pub type CliSys = sys_traits::impls::RealSys;
}

use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::future::Future;
use std::io::IsTerminal;
use std::io::Write as _;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;

use args::TaskFlags;
use deno_terminal::colors;
use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_core::futures::FutureExt;
use ende_core::unsync::JoinHandle;
use ende_lib::util::result::js_error_downcast_ref;
use ende_lib::worker::LibWorkerFactoryRoots;
use ende_runtime::UnconfiguredRuntime;
use ende_runtime::WorkerExecutionMode;
use ende_runtime::fmt_errors::format_js_error;
use ende_runtime::tokio_util::create_and_run_current_thread_with_maybe_metrics;
use ende_telemetry::OtelConfig;
use util::fs::canonicalize_path;

const MODULE_NOT_FOUND: &str = "Module not found";
const UNSUPPORTED_SCHEME: &str = "Unsupported scheme";
const UNSUPPORTED_DIR_IMPORT: &str =
  "[ERR_UNSUPPORTED_DIR_IMPORT] Directory import";

use self::util::draw_thread::DrawThread;
use self::util::env::resolve_cwd;
use crate::args::CompletionsFlags;
use crate::args::EndeSubcommand;
use crate::args::Flags;
use crate::args::FlagsExt;
// Re-exported for the flag-parsing benchmark in `cli/benches/flags.rs`.
#[doc(hidden)]
pub use crate::args::flags_from_vec_with_initial_cwd;
use crate::args::get_default_v8_flags;
use crate::util::display;
use crate::util::env::WatchEnvTracker;
use crate::util::env::load_env_variables_from_env_files;
use crate::util::v8::get_v8_flags_from_env;
use crate::util::v8::init_v8_flags;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Ensures that all subcommands return an i32 exit code and an [`AnyError`] error type.
trait SubcommandOutput {
  fn output(self) -> Result<i32, AnyError>;
}

impl SubcommandOutput for Result<i32, AnyError> {
  fn output(self) -> Result<i32, AnyError> {
    self
  }
}

impl SubcommandOutput for Result<(), AnyError> {
  fn output(self) -> Result<i32, AnyError> {
    self.map(|_| 0)
  }
}

impl SubcommandOutput for Result<(), std::io::Error> {
  fn output(self) -> Result<i32, AnyError> {
    self.map(|_| 0).map_err(|e| e.into())
  }
}

/// Ensure that the subcommand runs in a task, rather than being directly executed. Since some of these
/// futures are very large, this prevents the stack from getting blown out from passing them by value up
/// the callchain (especially in debug mode when Rust doesn't have a chance to elide copies!).
#[inline(always)]
fn spawn_subcommand<F: Future<Output = T> + 'static, T: SubcommandOutput>(
  f: F,
) -> JoinHandle<Result<i32, AnyError>> {
  // the boxed_local() is important in order to get windows to not blow the stack in debug
  ende_core::unsync::spawn(
    async move { f.map(|r| r.output()).await }.boxed_local(),
  )
}

async fn run_subcommand(
  flags: Flags,
  unconfigured_runtime: Option<UnconfiguredRuntime>,
  roots: LibWorkerFactoryRoots,
) -> Result<i32, AnyError> {
  let handle = match flags.subcommand.clone() {
    EndeSubcommand::Add(add_flags) => spawn_subcommand(async {
      tools::pm::add(Arc::new(flags), add_flags, tools::pm::AddCommandName::Add)
        .await
    }),
    EndeSubcommand::Remove(remove_flags) => spawn_subcommand(async {
      tools::pm::remove(Arc::new(flags), remove_flags).await
    }),
    EndeSubcommand::Link(link_flags) => spawn_subcommand(async {
      tools::pm::link(Arc::new(flags), link_flags).await
    }),
    EndeSubcommand::Unlink(unlink_flags) => spawn_subcommand(async {
      tools::pm::unlink(Arc::new(flags), unlink_flags).await
    }),
    EndeSubcommand::Bench(bench_flags) => spawn_subcommand(async {
      let flags = Arc::new(flags);
      if flags.watch.is_some() {
        tools::bench::run_benchmarks_with_watch(flags, bench_flags)
          .boxed_local()
          .await
      } else {
        tools::bench::run_benchmarks(flags, bench_flags).await
      }
    }),
    EndeSubcommand::Doc(doc_flags) => spawn_subcommand(async {
      tools::doc::doc(Arc::new(flags), doc_flags).await
    }),
    EndeSubcommand::Eval(eval_flags) => spawn_subcommand(async {
      tools::run::eval_command(Arc::new(flags), eval_flags).await
    }),
    EndeSubcommand::Cache(cache_flags) => spawn_subcommand(async move {
      tools::installer::install_from_entrypoints(
        Arc::new(flags),
        self::args::InstallEntrypointsFlags {
          entrypoints: cache_flags.files,
          lockfile_only: false,
        },
      )
      .await
    }),
    EndeSubcommand::Check(check_flags) => spawn_subcommand(async move {
      tools::check::check(Arc::new(flags), check_flags).await
    }),
    EndeSubcommand::Clean(clean_flags) => spawn_subcommand(async move {
      tools::clean::clean(Arc::new(flags), clean_flags).await
    }),
    EndeSubcommand::Compile(compile_flags) => spawn_subcommand(async {
      tools::compile::compile(flags, compile_flags).await
    }),
    EndeSubcommand::Desktop(desktop_flags) => spawn_subcommand(async {
      Box::pin(tools::desktop::desktop(flags, desktop_flags)).await
    }),
    EndeSubcommand::Coverage(coverage_flags) => spawn_subcommand(async move {
      let reporter =
        crate::tools::coverage::reporter::create(coverage_flags.r#type.clone());
      tools::coverage::cover_files(
        Arc::new(flags),
        coverage_flags.files.include,
        coverage_flags.files.ignore,
        coverage_flags.include,
        coverage_flags.exclude,
        coverage_flags.output,
        coverage_flags.threshold.map(|t| t as f64),
        &[&*reporter],
      )
    }),
    EndeSubcommand::Fmt(fmt_flags) => spawn_subcommand(async move {
      tools::fmt::format(Arc::new(flags), fmt_flags).await
    }),
    EndeSubcommand::Transpile(transpile_flags) => {
      spawn_subcommand(async move {
        log::warn!(
          "⚠️  {} is experimental and subject to changes",
          colors::cyan("ende transpile")
        );
        tools::transpile::transpile(Arc::new(flags), transpile_flags).await
      })
    }
    EndeSubcommand::Init(init_flags) => spawn_subcommand(async {
      tools::init::init_project(flags, init_flags).await
    }),
    EndeSubcommand::Info(info_flags) => spawn_subcommand(async {
      tools::info::info(Arc::new(flags), info_flags).await
    }),
    EndeSubcommand::List(list_flags) => spawn_subcommand(async {
      tools::pm::list(Arc::new(flags), list_flags).await
    }),
    EndeSubcommand::Install(install_flags) => spawn_subcommand(async {
      tools::installer::install_command(Arc::new(flags), install_flags).await
    }),
    EndeSubcommand::Ci(ci_flags) => spawn_subcommand(async {
      tools::installer::ci_command(Arc::new(flags), ci_flags).await
    }),
    EndeSubcommand::JSONReference(json_reference) => {
      spawn_subcommand(async move {
        display::write_json_to_stdout(&json_reference.json)
      })
    }
    EndeSubcommand::Uninstall(uninstall_flags) => spawn_subcommand(async {
      tools::installer::uninstall(Arc::new(flags), uninstall_flags).await
    }),
    EndeSubcommand::Lsp => spawn_subcommand(async move {
      if std::io::stderr().is_terminal() {
        log::warn!(
          "{} command is intended to be run by text editors and IDEs and shouldn't be run manually.

  Visit https://docs.ende.sh/runtime/getting_started/setup_your_environment/ for instruction
  how to setup your favorite text editor.

  Press Ctrl+C to exit.
        ", colors::cyan("ende lsp"));
      }
      lsp::start().await
    }),
    EndeSubcommand::Lint(lint_flags) => spawn_subcommand(async {
      if lint_flags.rules {
        tools::lint::print_rules_list(
          lint_flags.json,
          lint_flags.maybe_rules_tags,
        );
        Ok(())
      } else {
        tools::lint::lint(Arc::new(flags), lint_flags).await
      }
    }),
    EndeSubcommand::Outdated(update_flags) => spawn_subcommand(async move {
      tools::pm::outdated(Arc::new(flags), update_flags).await
    }),
    EndeSubcommand::Repl(repl_flags) => spawn_subcommand(async move {
      tools::repl::run(Arc::new(flags), repl_flags).await
    }),
    EndeSubcommand::X(x_flags) => {
      spawn_subcommand(
        async move { tools::x::run(Arc::new(flags), x_flags).await },
      )
    }
    EndeSubcommand::Run(run_flags) => spawn_subcommand(async move {
      if run_flags.print_task_list {
        let task_flags = TaskFlags {
          cwd: None,
          task: None,
          is_run: true,
          recursive: false,
          members: false,
          filter: None,
          eval: false,
          no_prefix: false,
          concurrency: None,
          if_present: false,
        };
        let mut flags = flags;
        flags.subcommand = EndeSubcommand::Task(task_flags.clone());
        writeln!(
          &mut std::io::stdout(),
          "Please specify a {} or a {}.\n",
          colors::bold("[SCRIPT_ARG]"),
          colors::bold("task name")
        )?;
        std::io::stdout().flush()?;
        tools::task::execute_script(Arc::new(flags), task_flags)
          .await
          .map(|_| 1)
      } else if run_flags.is_stdin() {
        // these futures are boxed to prevent stack overflows on Windows
        tools::run::run_from_stdin(Arc::new(flags), unconfigured_runtime, roots)
          .boxed_local()
          .await
      } else if flags.eszip {
        tools::run::run_eszip(
          Arc::new(flags),
          run_flags,
          unconfigured_runtime,
          roots,
        )
        .boxed_local()
        .await
      } else {
        let flags = Arc::new(flags);
        let result = tools::run::run_script(
          WorkerExecutionMode::Run,
          flags.clone(),
          flags.watch.clone(),
          unconfigured_runtime,
          roots.clone(),
        )
        .await;
        match result {
          Ok(v) => Ok(v),
          Err(script_err) => {
            let script_err_msg = script_err.to_string();
            if should_fallback_on_run_error(script_err_msg.as_str()) {
              if run_flags.bare {
                let command_names = ende_cli_parser::defs::ENDE_ROOT
                  .subcommands
                  .iter()
                  .map(|command| command.name)
                  .collect::<Vec<_>>();
                let suggestions =
                  args::did_you_mean(&run_flags.script, command_names);
                if !suggestions.is_empty() && !run_flags.script.contains('.') {
                  let quoted = suggestions
                    .iter()
                    .map(|s| format!("'{s}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                  let tip = if suggestions.len() == 1 {
                    format!("a similar subcommand exists: {quoted}")
                  } else {
                    format!("some similar subcommands exist: {quoted}")
                  };
                  Err(unrecognized_subcommand_error(
                    &ende_cli_parser::CliError::new(
                      ende_cli_parser::CliErrorKind::UnknownSubcommand,
                      format!("unrecognized subcommand '{}'", run_flags.script),
                    )
                    .with_suggestion(tip),
                  ))
                } else {
                  Err(script_err)
                }
              } else {
                let mut new_flags = flags.deref().clone();
                let task_flags = TaskFlags {
                  cwd: None,
                  task: Some(run_flags.script.clone()),
                  is_run: true,
                  recursive: false,
                  members: false,
                  filter: None,
                  eval: false,
                  no_prefix: false,
                  concurrency: None,
                  if_present: false,
                };
                new_flags.subcommand = EndeSubcommand::Task(task_flags.clone());
                let result = tools::task::execute_script(
                  Arc::new(new_flags),
                  task_flags.clone(),
                )
                .await;
                match result {
                  Ok(v) => Ok(v),
                  Err(_) => {
                    // Return script error for backwards compatibility.
                    Err(script_err)
                  }
                }
              }
            } else {
              Err(script_err)
            }
          }
        }
      }
    }),
    EndeSubcommand::Serve(serve_flags) => spawn_subcommand(async move {
      tools::serve::serve(
        Arc::new(flags),
        serve_flags,
        unconfigured_runtime,
        roots,
      )
      .await
    }),
    EndeSubcommand::Task(task_flags) => spawn_subcommand(async {
      tools::task::execute_script(Arc::new(flags), task_flags).await
    }),
    EndeSubcommand::Test(test_flags) => {
      spawn_subcommand(async {
        if let Some(ref coverage_dir) = test_flags.coverage_dir {
          if !test_flags.coverage_raw_data_only || test_flags.clean {
            // Keeps coverage_dir contents only when --coverage-raw-data-only is set and --clean is not set
            let _ = std::fs::remove_dir_all(coverage_dir);
          }
          std::fs::create_dir_all(coverage_dir)
            .with_context(|| format!("Failed creating: {coverage_dir}"))?;
          // this is set in order to ensure spawned processes use the same
          // coverage directory

          // SAFETY: called during single-threaded CLI startup
          unsafe {
            env::set_var(
              "ENDE_COVERAGE_DIR",
              canonicalize_path(&PathBuf::from(coverage_dir))?,
            )
          };
        }

        if flags.watch.is_some() {
          tools::test::run_tests_with_watch(Arc::new(flags), test_flags).await
        } else {
          tools::test::run_tests(Arc::new(flags), test_flags).await
        }
      })
    }
    EndeSubcommand::Completions(completions_flags) => {
      spawn_subcommand(async move {
        match completions_flags {
          CompletionsFlags::Static(buf) => {
            ende_print::drop_write_stdout(&buf);
            Ok::<_, AnyError>(())
          }
          CompletionsFlags::Dynamic { shell } => {
            crate::args::handle_dynamic_shell_completion(&shell)?;
            Ok(())
          }
        }
      })
    }
    EndeSubcommand::Types => spawn_subcommand(async move {
      let types = tsc::get_types_declaration_file_text();
      ende_print::drop_write_stdout(types.as_bytes());
      Ok::<_, AnyError>(())
    }),
    #[cfg(feature = "upgrade")]
    EndeSubcommand::Upgrade(upgrade_flags) => spawn_subcommand(async {
      tools::upgrade::upgrade(Arc::new(flags), upgrade_flags).await
    }),
    #[cfg(not(feature = "upgrade"))]
    EndeSubcommand::Upgrade(_) => exit_with_message(
      "This ende was built without the \"upgrade\" feature. Please upgrade using the installation method originally used to install Ende.",
      1,
    ),
    EndeSubcommand::Why(why_flags) => spawn_subcommand(async {
      tools::pm::why(Arc::new(flags), why_flags).await
    }),
    EndeSubcommand::BumpVersion(version_flags) => spawn_subcommand(async {
      log::warn!(
        "{}",
        colors::yellow(
          "ende bump-version is experimental and subject to change"
        )
      );
      tools::bump_version::bump_version_command(Arc::new(flags), version_flags)
    }),
    EndeSubcommand::Vendor => exit_with_message(
      "⚠️  `ende vendor` was removed.\n\nSee the Migration Guide for migration instructions: https://docs.ende.sh/runtime/manual/advanced/migrate_deprecations",
      1,
    ),
    EndeSubcommand::Publish(publish_flags) => spawn_subcommand(async {
      tools::publish::publish(Arc::new(flags), publish_flags).await
    }),
    EndeSubcommand::Help(help_flags) => spawn_subcommand(async move {
      use std::io::Write;

      let mut stream = anstream::AutoStream::new(
        std::io::stdout(),
        if colors::use_color() {
          anstream::ColorChoice::Auto
        } else {
          anstream::ColorChoice::Never
        },
      );

      match stream.write_all(help_flags.help.as_bytes()) {
        Ok(()) => Ok(()),
        Err(e) => match e.kind() {
          std::io::ErrorKind::BrokenPipe => Ok(()),
          _ => Err(e),
        },
      }
    }),
  };

  handle.await?
}

/// Determines whether a error encountered during `ende run`
/// should trigger fallback behavior, such as attempting to run an Ende task
/// with the same name.
///
/// Checks if the error message indicates a "module not found",
/// "unsupported scheme", or certain OS-level import failures (such as
/// "Is a directory" or "Access is denied"); if so, Ende will attempt to
/// interpret the original argument as a script name or task instead of a
/// file path.
///
/// See: https://github.com/denoland/deno/issues/28878
fn should_fallback_on_run_error(script_err: &str) -> bool {
  if script_err.starts_with(MODULE_NOT_FOUND)
    || script_err.starts_with(UNSUPPORTED_SCHEME)
    || script_err.starts_with(UNSUPPORTED_DIR_IMPORT)
  {
    return true;
  }
  let re = lazy_regex::regex!(
    r"Import 'file:///.+?' failed\.\n\s+0: .+ \(os error \d+\)"
  );
  re.is_match(script_err)
}

#[allow(clippy::print_stderr, reason = "panic hook")]
fn setup_panic_hook() {
  // This function does two things inside of the panic hook:
  // - Tokio does not exit the process when a task panics, so we define a custom
  //   panic hook to implement this behaviour.
  // - We print a message to stderr to indicate that this is a bug in Ende, and
  //   should be reported to us.
  let orig_hook = std::panic::take_hook();
  std::panic::set_hook(Box::new(move |panic_info| {
    // Broken-pipe panics from `println!`/`eprintln!` (e.g. `ende | cls` on
    // Windows, where the reader closes the pipe before we finish writing) are
    // not bugs in Ende — they're a normal consequence of the downstream
    // process exiting. Exit silently rather than emitting the bug-report
    // banner. https://github.com/denoland/deno/issues/16308
    if is_broken_pipe_print_panic(panic_info) {
      ende_runtime::exit(1);
    }

    // Worker thread creation failures are not bugs in Ende — they typically
    // happen when running in a container with a low PID limit (e.g. Docker
    // `--pids-limit`). Show a targeted message instead of the bug-report
    // banner. The matched strings come from V8 (via the `error_handler`
    // below) and `deno_unsync`, so they may need updating if those change.
    // https://github.com/denoland/deno/issues/31300
    let panic_str = panic_info
      .payload()
      .downcast_ref::<String>()
      .map(|s| s.as_str())
      .or_else(|| panic_info.payload().downcast_ref::<&str>().copied())
      .unwrap_or("");
    let is_resource_limit = panic_str.contains("worker thread")
      || panic_str.contains("Resource temporarily unavailable")
      || panic_str.contains("could not start worker threads");
    if is_resource_limit {
      eprintln!(
        "\n============================================================"
      );
      eprintln!("Ende could not start: unable to create worker threads.");
      eprintln!();
      eprintln!("This usually happens when running in a container with a low");
      eprintln!("PID limit. Try increasing the limit (e.g. --pids-limit=40).");
      eprintln!();
      eprintln!("Platform: {} {}", env::consts::OS, env::consts::ARCH);
      eprintln!("Version: {}", ende_lib::version::ENDE_VERSION_INFO.ende);
      eprintln!("Args: {:?}", env::args().collect::<Vec<_>>());
      eprintln!();
      ende_runtime::exit(1);
    }

    eprintln!("\n============================================================");
    eprintln!("Ende has panicked. This is a bug in Ende. Please report this");
    eprintln!("at https://github.com/theende/ende/issues/new.");
    eprintln!("If you can reliably reproduce this panic, include the");
    #[cfg(not(all(
      feature = "panic-trace-frame-pointer",
      target_os = "linux",
      any(target_arch = "x86_64", target_arch = "aarch64")
    )))]
    {
      eprintln!("reproduction steps and re-run with the RUST_BACKTRACE=1 env");
      eprintln!("var set and include the backtrace in your report.");
    }
    #[cfg(all(
      feature = "panic-trace-frame-pointer",
      target_os = "linux",
      any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    eprintln!(
      "reproduction steps; the report link below includes the stack trace."
    );
    eprintln!();
    eprintln!("Platform: {} {}", env::consts::OS, env::consts::ARCH);
    eprintln!("Version: {}", ende_lib::version::ENDE_VERSION_INFO.ende);
    eprintln!("Args: {:?}", env::args().collect::<Vec<_>>());
    eprintln!();

    // Panic traces are not supported for custom/development builds.
    #[cfg(feature = "panic-trace")]
    let trace = {
      #[cfg(all(
        feature = "panic-trace-frame-pointer",
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
      ))]
      let trace = deno_panic::trace_frame_pointer();
      #[cfg(not(all(
        feature = "panic-trace-frame-pointer",
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
      )))]
      let trace = deno_panic::trace();
      Some(trace.to_string())
    };
    #[cfg(not(feature = "panic-trace"))]
    let trace = None;

    let platform = format!("{} {}", env::consts::OS, env::consts::ARCH);
    eprintln!("Report this panic with a prefilled issue:");
    eprintln!(
      "{}",
      util::panic_report::panic_issue_url(&util::panic_report::PanicReport {
        message: panic_str,
        version: ende_lib::version::ENDE_VERSION_INFO.ende,
        target: env!("TARGET"),
        platform: &platform,
        location: panic_info.location().map(|l| l.to_string()),
        trace,
      })
    );

    orig_hook(panic_info);
    ende_runtime::exit(1);
  }));

  fn error_handler(file: &str, line: i32, message: &str) {
    // Provide a clearer message for thread creation failures, which
    // typically happen when running in containers with low PID limits
    // (e.g. Docker --pids-limit). V8's default error is just
    // "Check failed: Start()" which is unhelpful.
    if message.contains("Check failed: Start()") {
      panic!(
        "Failed to initialize V8 platform (could not start worker threads). \
        If running in a container, ensure the PID limit is high enough \
        (try --pids-limit=40 or higher)."
      );
    }
    // Override C++ abort with a rust panic, so we
    // get our message above and a nice backtrace.
    panic!("Fatal error in {file}:{line}: {message}");
  }

  ende_core::v8::V8::set_fatal_error_handler(error_handler);
}

/// Returns `true` if `panic_info` is a panic from `std`'s print macros caused
/// by the downstream reader of stdout/stderr closing the pipe.
///
/// `println!`/`print!`/`eprintln!`/`eprint!` panic with the literal payload
/// `"failed printing to {stdout,stderr}: <io error>"` when the underlying
/// write fails. EPIPE (Unix, 32), ERROR_BROKEN_PIPE (Windows, 109), and
/// ERROR_NO_DATA (Windows, 232) all indicate the receiver dropped the pipe.
fn is_broken_pipe_print_panic(panic_info: &std::panic::PanicHookInfo) -> bool {
  let payload = panic_info.payload();
  let msg: &str = if let Some(s) = payload.downcast_ref::<String>() {
    s.as_str()
  } else if let Some(&s) = payload.downcast_ref::<&'static str>() {
    s
  } else {
    return false;
  };
  if !msg.starts_with("failed printing to stdout:")
    && !msg.starts_with("failed printing to stderr:")
  {
    return false;
  }
  msg.contains("(os error 32)")
    || msg.contains("(os error 109)")
    || msg.contains("(os error 232)")
}

fn exit_with_message(message: &str, code: i32) -> ! {
  log::error!(
    "{}: {}",
    colors::red_bold("error"),
    message.trim_start_matches("error: ")
  );
  ende_runtime::exit(code);
}

/// Render the "unrecognized subcommand" suggestion for the terminal.
///
/// This one error is followed by a blank line, which is what clap's formatter
/// produced for it. Ordinary flag errors are rendered without it (they went
/// through `clap::Error::raw`, which appended nothing), so they keep using
/// `CliError`'s plain `Display`.
fn unrecognized_subcommand_error(err: &ende_cli_parser::CliError) -> AnyError {
  AnyError::msg(format!("{err}\n"))
}

fn exit_for_error(error: AnyError, initial_cwd: Option<&std::path::Path>) -> ! {
  let mut error_string = match js_error_downcast_ref(&error) {
    Some(e) => {
      let initial_cwd = initial_cwd
        .and_then(|cwd| deno_path_util::url_from_directory_path(cwd).ok());
      format_js_error(e, initial_cwd.as_ref())
    }
    None => format!("{error:?}"),
  };

  // When a package couldn't be resolved because the only matching version is
  // newer than the configured minimum dependency age, the error doesn't
  // mention the setting that caused it. Point the user at it.
  if let Some(hint) = maybe_minimum_dependency_age_hint(&error_string) {
    error_string.push_str(&hint);
  }

  exit_with_message(&error_string, 1);
}

/// Substring present in every "package rejected because it is newer than the
/// minimum dependency age" error (both the jsr and npm resolvers include it).
const MINIMUM_DEPENDENCY_AGE_MARKER: &str = "minimum dependency date";

/// If `error_string` is a resolution failure caused by the minimum dependency
/// age safeguard, returns a note explaining the setting and how to override it.
fn maybe_minimum_dependency_age_hint(error_string: &str) -> Option<String> {
  if !error_string.contains(MINIMUM_DEPENDENCY_AGE_MARKER) {
    return None;
  }
  Some(format!(
    concat!(
      "\n\n{} This version is blocked by the minimum dependency age policy, ",
      "which avoids installing recently published versions to reduce supply ",
      "chain risk (the default is 24 hours). To use this version now, pass the ",
      "{} (or {}) flag (for example {} to disable it, or a shorter duration ",
      "like {} minutes) or set {} in your ende.json, or wait until the version ",
      "is old enough.\n{} {}"
    ),
    colors::yellow("hint:"),
    colors::bold("--minimum-dependency-age"),
    colors::bold("--min-dep-age"),
    colors::bold("0"),
    colors::bold("60"),
    colors::bold("\"minimumDependencyAge\""),
    colors::yellow("docs:"),
    colors::cyan("https://docs.ende.sh/go/minimum-dependency-age"),
  ))
}

pub(crate) fn unstable_exit_cb(feature: &str, api_name: &str) {
  log::error!(
    "Unstable API '{api_name}'. The `--unstable-{}` flag must be provided.",
    feature
  );
  ende_runtime::exit(70);
}

fn maybe_setup_permission_broker() {
  let Ok(socket_path) = ende_env::var("ENDE_PERMISSION_BROKER_PATH") else {
    return;
  };
  log::warn!(
    "{} Permission broker is an experimental feature",
    colors::yellow("Warning")
  );
  let broker =
    ende_runtime::ende_permissions::broker::PermissionBroker::new(socket_path);
  ende_runtime::ende_permissions::broker::set_broker(broker);
}

#[inline(always)]
pub(crate) fn boot_phase(label: &str) {
  use std::sync::OnceLock;
  use std::time::Instant;
  static START: OnceLock<Instant> = OnceLock::new();
  static ENABLED: OnceLock<bool> = OnceLock::new();
  let start = START.get_or_init(Instant::now);
  let enabled =
    *ENABLED.get_or_init(|| ende_env::var_os("ENDE_STARTUP_PHASES").is_some());
  if enabled {
    #[allow(clippy::print_stderr, reason = "diagnostic")]
    {
      eprintln!("[boot] {label:>28}  {:?}", start.elapsed());
    }
  }
}

pub fn main() {
  boot_phase("main start");
  #[cfg(feature = "dhat-heap")]
  let profiler = dhat::Profiler::new_heap();

  setup_panic_hook();

  init_logging(None, None);
  boot_phase("after panic+logging");

  util::unix::raise_fd_limit();
  util::windows::ensure_stdio_open();
  #[cfg(windows)]
  {
    ende_subprocess_windows::disable_stdio_inheritance();
    colors::enable_ansi(); // For Windows 10
  }
  ende_runtime::ende_permissions::prompter::set_prompt_callbacks(
    Box::new(util::draw_thread::DrawThread::hide),
    Box::new(util::draw_thread::DrawThread::show),
  );

  maybe_setup_permission_broker();

  boot_phase("before aws_lc install");
  rustls::crypto::aws_lc_rs::default_provider()
    .install_default()
    .unwrap();
  boot_phase("after aws_lc install");

  let args: Vec<_> = env::args_os().collect();
  let future = async move {
    let roots = LibWorkerFactoryRoots::default();

    #[cfg(unix)]
    let (waited_unconfigured_runtime, waited_args, waited_cwd) =
      match wait_for_start(&args, roots.clone()) {
        Some(f) => match f.await {
          Ok(v) => match v {
            Some((u, a, c)) => (Some(u), Some(a), Some(c)),
            None => (None, None, None),
          },
          Err(e) => {
            panic!("Failure from control sock: {e}");
          }
        },
        None => (None, None, None),
      };

    #[cfg(not(unix))]
    let (waited_unconfigured_runtime, waited_args, waited_cwd) =
      (None, None, None);

    let args = waited_args.unwrap_or(args);
    #[allow(
      clippy::disallowed_methods,
      reason = "ok because initialization of cwd"
    )]
    let initial_cwd =
      waited_cwd
        .map(Some)
        .unwrap_or_else(|| match std::env::current_dir() {
          Ok(cwd) => Some(cwd),
          Err(err) => {
            log::error!("Failed getting cwd: {err}");
            None
          }
        });

    // NOTE(lucacasonato): due to new PKU feature introduced in V8 11.6 we need to
    // initialize the V8 platform on a parent thread of all threads that will spawn
    // V8 isolates.
    let flags = match resolve_flags_and_init(args, initial_cwd.clone()) {
      Ok(flags) => flags,
      Err(err) => return (Err(err), initial_cwd),
    };

    if waited_unconfigured_runtime.is_none() {
      init_v8(&flags);
    }
    boot_phase("after init_v8");

    (
      run_subcommand(flags, waited_unconfigured_runtime, roots).await,
      initial_cwd,
    )
  };

  let (result, initial_cwd) =
    create_and_run_current_thread_with_maybe_metrics(future);

  #[cfg(feature = "dhat-heap")]
  drop(profiler);

  match result {
    Ok(exit_code) => ende_runtime::exit(exit_code),
    Err(err) => exit_for_error(err, initial_cwd.as_deref()),
  }
}

fn resolve_flags_and_init(
  args: Vec<std::ffi::OsString>,
  initial_cwd: Option<std::path::PathBuf>,
) -> Result<Flags, AnyError> {
  // This env var enables dynamic completions; it's set by the shell when
  // executing ende to get dynamic completions.
  if std::env::var("COMPLETE").is_ok() {
    let cwd = resolve_cwd(initial_cwd.as_deref())?;
    crate::args::handle_shell_completion(&cwd)?;
    ende_runtime::exit(0);
  }

  boot_phase("before flag parse");
  // `--version` prints the long (multi-line) version, `-V` the short one.
  // Determined here because the parser only signals that a version display was
  // requested, not which form.
  let wants_long_version = args.iter().any(|a| a == "--version");
  let mut flags =
    match flags_from_vec_with_initial_cwd(args, initial_cwd.clone()) {
      Ok(flags) => {
        boot_phase("after flag parse");
        flags
      }
      Err(err) if err.kind == ende_cli_parser::CliErrorKind::DisplayVersion => {
        boot_phase("flag parse (version exit)");
        let version = crate::args::render_version(wants_long_version);
        // drop_write_stdout ignores BrokenPipe etc.
        ende_print::drop_write_stdout(version.as_bytes());
        ende_runtime::exit(0);
      }
      Err(err) => exit_for_error(AnyError::from(err), initial_cwd.as_deref()),
    };
  // preserve already loaded env variables
  if flags.watch.is_some() {
    WatchEnvTracker::snapshot();
  }

  if let Some(files) = &flags.env_file {
    let cwd = match &flags.initial_cwd {
      Some(cwd) => Cow::Borrowed(cwd),
      None => match resolve_cwd(None) {
        Ok(cwd) => {
          flags.initial_cwd = Some(cwd.into_owned());
          Cow::Borrowed(flags.initial_cwd.as_ref().unwrap())
        }
        Err(_) => Cow::Owned(PathBuf::from(".")),
      },
    };
    load_env_variables_from_env_files(&cwd, files, flags.log_level);
  }

  let sys = crate::sys::CliSys::default();

  flags.unstable_config.fill_with_env(&sys);

  let otel_config = flags.otel_config();
  init_logging(flags.log_level, Some(otel_config.clone()));
  ende_telemetry::init(
    &sys,
    ende_lib::version::otel_runtime_config(),
    otel_config.clone(),
  )?;

  if flags.permission_set.is_some() {
    log::warn!(
      "{} Permissions in the config file is an experimental feature and may change in the future.",
      colors::yellow("Warning")
    );
  }

  // TODO(bartlomieju): remove in Deno v2.5 and hard error then.
  if flags.unstable_config.legacy_flag_enabled {
    log::warn!(
      "{} The `--unstable` flag has been removed. Use granular `--unstable-*` flags instead.\nLearn more at: https://docs.ende.sh/runtime/manual/tools/unstable_flags",
      colors::yellow("Warning")
    );
  }

  if let Ok(audit_target) = ende_env::var("ENDE_AUDIT_PERMISSIONS") {
    use ende_runtime::ende_permissions::AuditSink;

    let sink = if audit_target == "otel" {
      AuditSink::Otel(|permission, value, stack| {
        let stack = stack.unwrap_or_default().join("\n");
        let kvs = HashMap::from([
          ("ende.permission.type", permission),
          ("ende.permission.value", value),
          ("ende.permission.stack", stack.as_str()),
        ]);
        ende_telemetry::handle_log(
          &log::Record::builder()
            .level(log::Level::Info)
            .target("ende.permission.access")
            .args(format_args!("{permission}: {value}"))
            .key_values(&kvs)
            .build(),
        );
      })
    } else {
      AuditSink::File(ende_core::parking_lot::Mutex::new(
        std::fs::File::create(audit_target)?,
      ))
    };

    let result = ende_runtime::ende_permissions::AUDIT_SINK.set(sink);
    if result.is_err() {
      log::warn!("⚠️  {}", colors::yellow("Audit sink is already set"));
    }
  }

  Ok(flags)
}

fn init_v8(flags: &Flags) {
  let default_v8_flags = match flags.subcommand {
    EndeSubcommand::Lsp => vec![
      "--stack-size=1024".to_string(),
      // Using same default as VSCode:
      // https://github.com/microsoft/vscode/blob/48d4ba271686e8072fc6674137415bc80d936bc7/extensions/typescript-language-features/src/configuration/configuration.ts#L213-L214
      "--max-old-space-size=3072".to_string(),
    ],
    _ => get_default_v8_flags(),
  };

  let env_v8_flags = get_v8_flags_from_env();
  let is_single_threaded = env_v8_flags
    .iter()
    .chain(&flags.v8_flags)
    .any(|flag| flag == "--single-threaded");
  init_v8_flags(&default_v8_flags, &flags.v8_flags, env_v8_flags);
  let v8_platform = if is_single_threaded {
    Some(::ende_core::v8::Platform::new_single_threaded(true).make_shared())
  } else {
    None
  };

  ende_core::JsRuntime::init_platform(v8_platform);
}

fn init_logging(
  maybe_level: Option<log::Level>,
  otel_config: Option<OtelConfig>,
) {
  ende_lib::util::logger::init(ende_lib::util::logger::InitLoggingOptions {
    maybe_level,
    otel_config,
    // it was considered to hold the draw thread's internal lock
    // across logging, but if outputting to stderr blocks then that
    // could potentially block other threads that access the draw
    // thread's state
    on_log_start: DrawThread::hide,
    on_log_end: DrawThread::show,
  })
}

#[cfg(unix)]
#[allow(clippy::type_complexity, reason = "private code")]
fn wait_for_start(
  args: &[std::ffi::OsString],
  roots: LibWorkerFactoryRoots,
) -> Option<
  impl Future<
    Output = Result<
      Option<(UnconfiguredRuntime, Vec<std::ffi::OsString>, PathBuf)>,
      AnyError,
    >,
  > + use<>,
> {
  let startup_snapshot = ende_snapshots::CLI_SNAPSHOT?;
  let addr = ende_env::var("ENDE_UNSTABLE_CONTROL_SOCK").ok()?;

  // SAFETY: single-threaded at this point in startup
  unsafe {
    std::env::remove_var("ENDE_UNSTABLE_CONTROL_SOCK");
    std::env::remove_var("DENO_UNSTABLE_CONTROL_SOCK");
  };

  let argv0 = args[0].clone();

  Some(async move {
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncRead;
    use tokio::io::AsyncWrite;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    use tokio::net::UnixSocket;
    #[cfg(any(
      target_os = "android",
      target_os = "linux",
      target_os = "macos"
    ))]
    use tokio_vsock::VsockAddr;
    #[cfg(any(
      target_os = "android",
      target_os = "linux",
      target_os = "macos"
    ))]
    use tokio_vsock::VsockListener;

    init_v8(&Flags::default());

    let unconfigured = ende_runtime::UnconfiguredRuntime::new(
      ende_runtime::UnconfiguredRuntimeOptions {
        startup_snapshot,
        residual_lazy_js_sources: ende_snapshots::RESIDUAL_LAZY_JS,
        residual_lazy_esm_sources: ende_snapshots::RESIDUAL_LAZY_ESM,
        create_params: ende_lib::worker::create_isolate_create_params(
          &crate::sys::CliSys::default(),
        ),
        shared_array_buffer_store: Some(
          roots.shared_array_buffer_store.clone(),
        ),
        compiled_wasm_module_store: Some(
          roots.compiled_wasm_module_store.clone(),
        ),
        additional_extensions: vec![],
      },
    );

    let (rx, mut tx): (
      Box<dyn AsyncRead + Unpin>,
      Box<dyn AsyncWrite + Send + Unpin>,
    ) = match addr.split_once(':') {
      Some(("unix", path)) => {
        let socket = UnixSocket::new_stream()?;
        socket.bind(path)?;
        let listener = socket.listen(1)?;
        let (stream, _) = listener.accept().await?;
        let (rx, tx) = stream.into_split();
        (Box::new(rx), Box::new(tx))
      }
      #[cfg(any(
        target_os = "android",
        target_os = "linux",
        target_os = "macos"
      ))]
      Some(("vsock", addr)) => {
        let Some((cid, port)) = addr.split_once(':') else {
          ende_core::anyhow::bail!("invalid vsock addr");
        };
        let cid = if cid == "-1" { u32::MAX } else { cid.parse()? };
        let port = port.parse()?;
        let addr = VsockAddr::new(cid, port);
        let listener = VsockListener::bind(addr)?;
        let (stream, _) = listener.accept().await?;
        let (rx, tx) = stream.into_split();
        (Box::new(rx), Box::new(tx))
      }
      _ => {
        ende_core::anyhow::bail!("invalid control sock");
      }
    };

    let mut buf = Vec::with_capacity(1024);
    BufReader::new(rx).read_until(b'\n', &mut buf).await?;

    tokio::spawn(async move {
      ende_runtime::ende_http::SERVE_NOTIFIER.notified().await;

      #[derive(ende_core::serde::Serialize)]
      enum Event {
        Serving {
          #[serde(skip_serializing_if = "Option::is_none")]
          kind: Option<&'static str>,
        },
      }

      let mut buf = ende_core::serde_json::to_vec(&Event::Serving {
        kind: ende_runtime::ende_http::serving_server_kind(),
      })
      .unwrap();
      buf.push(b'\n');
      let _ = tx.write_all(&buf).await;
    });

    #[derive(ende_core::serde::Deserialize)]
    struct Start {
      cwd: String,
      args: Vec<String>,
      env: Vec<(String, String)>,
    }

    let cmd: Start = ende_core::serde_json::from_slice(&buf)?;

    std::env::set_current_dir(&cmd.cwd)?;

    for (k, v) in cmd.env {
      // SAFETY: We're doing this before any threads are created.
      unsafe { std::env::set_var(k, v) };
    }

    let args = [argv0]
      .into_iter()
      .chain(cmd.args.into_iter().map(Into::into))
      .collect();

    Ok(Some((unconfigured, args, PathBuf::from(cmd.cwd))))
  })
}
