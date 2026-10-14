// Copyright 2018-2026 the Deno authors. MIT license.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;

use deno_task_shell::ExecutableCommand;
use deno_task_shell::ExecuteResult;
use deno_task_shell::KillSignal;
use deno_task_shell::ShellCommand;
use deno_task_shell::ShellCommandContext;
use deno_task_shell::ShellPipeReader;
use deno_task_shell::ShellPipeWriter;
use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_core::futures::future::LocalBoxFuture;
use tokio::task::JoinHandle;
use tokio::task::LocalSet;
use tokio_util::sync::CancellationToken;

use crate::util::fs::canonicalize_path;

pub fn get_script_with_args(script: &str, argv: &[String]) -> String {
  let additional_args = argv
    .iter()
    // Wrap each argument in single quotes so the shell preserves the
    // content literally (including backslashes, $, `, etc.). For an
    // argument containing a single quote, splice in a double-quoted `'`
    // using the POSIX idiom `'foo'"'"'bar'`.
    .map(|a| format!("'{}'", a.replace('\'', "'\"'\"'")))
    .collect::<Vec<_>>()
    .join(" ");

  let script = format!("{script} {additional_args}");
  script.trim().to_owned()
}

pub struct TaskStdio(ShellPipeWriter);

pub struct PrefixedWriter<W: std::io::Write> {
  prefix: Vec<u8>,
  inner: W,
  line_buf: Vec<u8>,
  at_line_start: bool,
}

impl<W: std::io::Write> PrefixedWriter<W> {
  pub fn new(prefix: String, inner: W) -> Self {
    Self {
      prefix: prefix.into_bytes(),
      inner,
      line_buf: Vec::new(),
      at_line_start: true,
    }
  }
}

impl<W: std::io::Write> std::io::Write for PrefixedWriter<W> {
  fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
    let mut rest = buf;
    while !rest.is_empty() {
      if self.at_line_start {
        self.line_buf.extend_from_slice(&self.prefix);
        self.at_line_start = false;
      }
      match rest.iter().position(|&b| b == b'\n') {
        Some(pos) => {
          self.line_buf.extend_from_slice(&rest[..pos + 1]);
          self.inner.write_all(&self.line_buf)?;
          self.line_buf.clear();
          self.at_line_start = true;
          rest = &rest[pos + 1..];
        }
        None => {
          self.line_buf.extend_from_slice(rest);
          break;
        }
      }
    }
    Ok(buf.len())
  }

  fn flush(&mut self) -> std::io::Result<()> {
    if !self.line_buf.is_empty() {
      self.inner.write_all(&self.line_buf)?;
      self.line_buf.clear();
      self.at_line_start = true;
    }
    self.inner.flush()
  }
}

pub fn make_prefixed_task_io(prefix: String) -> (TaskIo, Vec<JoinHandle<()>>) {
  let (out_r, out_w) = deno_task_shell::pipe();
  let (err_r, err_w) = deno_task_shell::pipe();

  let out_prefix = prefix.clone();
  let out_handle = tokio::task::spawn_blocking(move || {
    let mut writer = PrefixedWriter::new(out_prefix, std::io::stdout());
    let _ = out_r.pipe_to(&mut writer);
    let _ = writer.flush();
  });

  let err_handle = tokio::task::spawn_blocking(move || {
    let mut writer = PrefixedWriter::new(prefix, std::io::stderr());
    let _ = err_r.pipe_to(&mut writer);
    let _ = writer.flush();
  });

  (
    TaskIo {
      stdout: TaskStdio(out_w),
      stderr: TaskStdio(err_w),
    },
    vec![out_handle, err_handle],
  )
}

impl TaskStdio {
  pub fn stdout() -> Self {
    Self(ShellPipeWriter::stdout())
  }

  pub fn stderr() -> Self {
    Self(ShellPipeWriter::stderr())
  }
}

pub struct TaskIo {
  pub stdout: TaskStdio,
  pub stderr: TaskStdio,
}

impl Default for TaskIo {
  fn default() -> Self {
    Self {
      stdout: TaskStdio::stdout(),
      stderr: TaskStdio::stderr(),
    }
  }
}

pub struct RunTaskOptions<'a> {
  pub task_name: &'a str,
  pub script: &'a str,
  pub cwd: PathBuf,
  pub init_cwd: &'a Path,
  pub env_vars: HashMap<OsString, OsString>,
  pub argv: &'a [String],
  pub custom_commands: HashMap<String, Rc<dyn ShellCommand>>,
  pub stdio: Option<TaskIo>,
  pub kill_signal: KillSignal,
}

pub struct TaskResult {
  pub exit_code: i32,
}

pub async fn run_task(
  mut opts: RunTaskOptions<'_>,
) -> Result<TaskResult, AnyError> {
  let script = get_script_with_args(opts.script, opts.argv);
  let seq_list = deno_task_shell::parser::parse(&script)
    .with_context(|| format!("Error parsing script '{}'.", opts.task_name))?;
  let env_vars = prepare_env_vars(opts.env_vars, opts.init_cwd);
  // `ende` runs the current executable; `deno` stays as an alias so existing
  // task scripts keep working.
  for name in ["ende", "deno"] {
    if !opts.custom_commands.contains_key(name) {
      opts
        .custom_commands
        .insert(name.to_string(), Rc::new(EndeCommand::default()));
    }
  }
  let state = deno_task_shell::ShellState::new(
    env_vars,
    opts.cwd,
    opts.custom_commands,
    opts.kill_signal,
  );
  let stdio = opts.stdio.unwrap_or_default();
  let (TaskStdio(stdout_write), TaskStdio(stderr_write)) =
    (stdio.stdout, stdio.stderr);

  let local = LocalSet::new();
  let future = async move {
    let exit_code = deno_task_shell::execute_with_pipes(
      seq_list,
      state,
      ShellPipeReader::stdin(),
      stdout_write,
      stderr_write,
    )
    .await;
    Ok::<_, AnyError>(TaskResult { exit_code })
  };
  local.run_until(future).await
}

fn prepare_env_vars(
  mut env_vars: HashMap<OsString, OsString>,
  initial_cwd: &Path,
) -> HashMap<OsString, OsString> {
  const INIT_CWD_NAME: &str = "INIT_CWD";
  if !env_vars.contains_key(OsStr::new(INIT_CWD_NAME)) {
    // if not set, set an INIT_CWD env var that has the cwd
    env_vars.insert(
      INIT_CWD_NAME.into(),
      initial_cwd.to_path_buf().into_os_string(),
    );
  }
  env_vars
}

pub fn real_env_vars() -> HashMap<OsString, OsString> {
  std::env::vars_os()
    .map(|(k, v)| {
      if cfg!(windows) {
        (k.to_ascii_uppercase(), v)
      } else {
        (k, v)
      }
    })
    .collect()
}

pub struct EndeCommand(ExecutableCommand);

impl Default for EndeCommand {
  fn default() -> Self {
    Self(ExecutableCommand::new(
      "ende".to_string(),
      std::env::current_exe()
        .and_then(|p| canonicalize_path(&p))
        .unwrap(),
    ))
  }
}

impl ShellCommand for EndeCommand {
  fn execute(
    &self,
    context: ShellCommandContext,
  ) -> LocalBoxFuture<'static, ExecuteResult> {
    self.0.execute(context)
  }
}

/// Runs an ende task future forwarding any signals received
/// to the process.
///
/// Signal listeners and ctrl+c listening will be setup.
pub async fn run_future_forwarding_signals<TOutput>(
  kill_signal: KillSignal,
  future: impl std::future::Future<Output = TOutput>,
) -> TOutput {
  fn spawn_future_with_cancellation(
    future: impl std::future::Future<Output = ()> + 'static,
    token: CancellationToken,
  ) {
    ende_core::unsync::spawn(async move {
      tokio::select! {
        _ = future => {}
        _ = token.cancelled() => {}
      }
    });
  }

  // Save the terminal mode up front so it can be restored once the task (and
  // all of its child processes) have finished. A child process such as a dev
  // server may switch the terminal into raw mode and, when terminated via
  // Ctrl+C, leave it in a broken state (no echo, no line editing). Declared
  // first so it is dropped last, i.e. after the children have been torn down by
  // the kill-signal drop guards below.
  let terminal_mode_guard = crate::util::console::TerminalModeGuard::acquire();
  let saved_terminal_mode = terminal_mode_guard.saved();

  let token = CancellationToken::new();
  let _token_drop_guard = token.clone().drop_guard();
  let _drop_guard = kill_signal.clone().drop_guard();

  spawn_future_with_cancellation(
    listen_ctrl_c(kill_signal.clone(), saved_terminal_mode),
    token.clone(),
  );
  #[cfg(unix)]
  spawn_future_with_cancellation(
    listen_and_forward_all_signals(kill_signal),
    token,
  );

  future.await
}

async fn listen_ctrl_c(
  kill_signal: KillSignal,
  saved_terminal_mode: crate::util::console::SavedTerminalMode,
) {
  while let Ok(()) = ende_signals::ctrl_c().await {
    // On windows, ctrl+c is sent to the process group, so the signal would
    // have already been sent to the child process. We still want to listen
    // for ctrl+c here to keep the process alive when receiving it, but no
    // need to forward the signal because it's already been sent.
    if cfg!(windows) {
      // A child (e.g. a dev server) may have put the terminal into raw mode.
      // Restore it now so that input echoes again — for instance at cmd.exe's
      // "Terminate batch job (Y/N)?" prompt that appears for `.cmd` scripts.
      // The TerminalModeGuard still restores once more when the task ends, in
      // case the child re-enters raw mode before exiting.
      saved_terminal_mode.restore();
    } else {
      kill_signal.send(deno_task_shell::SignalKind::SIGINT)
    }
  }
}

#[cfg(unix)]
async fn listen_and_forward_all_signals(kill_signal: KillSignal) {
  use ende_core::futures::FutureExt;
  use ende_signals::SIGNAL_NUMS;

  // listen and forward every signal we support
  let mut futures = Vec::with_capacity(SIGNAL_NUMS.len());
  for signo in SIGNAL_NUMS.iter().copied() {
    if signo == libc::SIGKILL || signo == libc::SIGSTOP {
      continue; // skip, can't listen to these
    }

    let kill_signal = kill_signal.clone();
    futures.push(
      async move {
        let Ok(mut stream) = ende_signals::signal_stream(signo) else {
          return;
        };
        let signal_kind: deno_task_shell::SignalKind = signo.into();
        while let Some(()) = stream.recv().await {
          kill_signal.send(signal_kind);
        }
      }
      .boxed_local(),
    )
  }
  ende_core::futures::future::join_all(futures).await;
}

#[cfg(test)]
mod test {

  use super::*;

  #[test]
  fn test_get_script_with_args() {
    let cases: &[(&[&str], &str)] = &[
      (&[], "echo"),
      (&["hello"], "echo 'hello'"),
      (&["hello", "world"], "echo 'hello' 'world'"),
      // Windows path with trailing backslash (issue #31453).
      (&[".\\dist\\"], "echo '.\\dist\\'"),
      // Dollar sign and backtick must not be expanded.
      (&["$HOME"], "echo '$HOME'"),
      (&["`cmd`"], "echo '`cmd`'"),
      // Double quotes pass through literally.
      (&["foo\"bar"], "echo 'foo\"bar'"),
      // Single quote uses the POSIX `'"'"'` idiom.
      (&["it's"], "echo 'it'\"'\"'s'"),
    ];
    for (argv, expected) in cases {
      let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
      assert_eq!(get_script_with_args("echo", &argv), *expected);
    }
  }
}
