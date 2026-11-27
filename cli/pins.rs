// Copyright 2018-2026 the Ende authors. MIT license.

//! Lockfile pins of what ende.json lets run in a sandbox: extensions (the
//! `extensions` section) and `isolate` rules (the `isolate` section). Both
//! work the same way:
//!
//! - Trust on first use: the first run records the pin and says so, even
//!   with `--quiet`, so a record is never silent.
//! - A run whose pin differs from the approved one is refused until
//!   `ende install` approves the change: it shows each change and asks, or
//!   takes `--yes`; without a terminal it refuses. `--frozen` refuses any
//!   write, a first record included.
//! - `--no-lock` runs unpinned, with a warning; a lockfile turned off in
//!   the config (`"lock": false`) is an error.

use deno_error::JsErrorBox;
use ende_core::error::AnyError;
use ende_lockfile::Lockfile;
use ende_runtime::ende_permissions::PermissionsOptions;

use crate::args::CliLockfile;

/// Something the lockfile pins: one extension or one `isolate` rule, as it
/// is now.
pub trait Pinned {
  /// What the lockfile records about it.
  type Entry: Clone + PartialEq;

  /// What a change to one is called in the `ende install` prompt, such as
  /// "extension".
  const KIND: &'static str;
  /// Many of them, for the error about a lockfile turned off.
  const PLURAL: &'static str;
  /// What its pin covers, for the warnings and errors about running
  /// without one, such as "its code, manifest or grant".
  const PINNED_PARTS: &'static str;
  /// What the lockfile protects, for the error about a lockfile turned
  /// off, such as "its code and permissions".
  const PROTECTS: &'static str;

  /// Its name in a sentence, such as `extension "@acme/fmt"`.
  fn name(&self) -> String;
  /// Its name at the start of a sentence, such as `Extension "@acme/fmt"`.
  fn title(&self) -> String;
  /// What it runs with, for the notices.
  fn options(&self) -> &PermissionsOptions;
  /// Its approved pin, if any.
  fn approved(&self, lockfile: &Lockfile) -> Option<Self::Entry>;
  /// Records `entry` as its approved pin.
  fn record(&self, lockfile: &mut Lockfile, entry: Self::Entry);
  /// What to record now: its pin as it is, merged into `approved` where
  /// what is new needs no approval.
  fn pin(&self, approved: Option<&Self::Entry>) -> Self::Entry;
  /// What needs approval since `approved` (`None` before a first record);
  /// empty when nothing does.
  fn changed_parts(&self, approved: Option<&Self::Entry>) -> Vec<String>;
  /// The change `ende install` asks about: each part that changed, old and
  /// new.
  fn describe_change(&self, approved: Option<&Self::Entry>) -> String;
  /// The command that approves what [`Self::changed_parts`] names.
  fn approve_command(&self) -> String {
    "ende install".to_string()
  }
  /// Whether recording it prints the notice.
  fn notice(&self) -> bool {
    true
  }
}

/// Errors if the lockfile approved `item` differently. A first run (or
/// what needs no approval) is recorded, with a notice.
pub fn check_pin<P: Pinned>(
  item: &P,
  lockfile: Option<&CliLockfile>,
  no_lock_flag: bool,
) -> Result<(), JsErrorBox> {
  let Some(lockfile) = lockfile else {
    if no_lock_flag {
      // The user's own opt-out, so it runs; shown even with --quiet.
      #[allow(clippy::print_stderr, reason = "must show even with --quiet")]
      {
        eprintln!(
          "{} {} runs unpinned: --no-lock turned off the lockfile, so changes to {} aren't checked.",
          crate::colors::yellow("Warning"),
          item.title(),
          P::PINNED_PARTS,
        );
      }
      return Ok(());
    }
    return Err(JsErrorBox::type_error(format!(
      "{} needs a lockfile, which pins {}, but the lockfile is turned off (\"lock\": false in the config). Turn it back on, or pass --no-lock to run {} unpinned.",
      item.title(),
      P::PROTECTS,
      P::PLURAL,
    )));
  };
  let approved = item.approved(&lockfile.lock());
  let parts = item.changed_parts(approved.as_ref());
  if !parts.is_empty() {
    let what = if approved.is_some() {
      format!("changed since {} approved it", lockfile_name(lockfile))
    } else {
      format!(
        "needs approval before {} records it",
        lockfile_name(lockfile)
      )
    };
    return Err(JsErrorBox::type_error(format!(
      "{} {what}: {}. Run `{}` to review the change and approve it.",
      item.title(),
      parts.join(", "),
      item.approve_command(),
    )));
  }
  let pin = item.pin(approved.as_ref());
  if approved.as_ref() != Some(&pin) {
    item.record(&mut lockfile.lock(), pin);
    lockfile.write_if_changed().map_err(JsErrorBox::from_err)?;
    print_recorded_notice(item, lockfile);
  }
  Ok(())
}

/// `ende install`: records new items and asks before recording changes to
/// approved ones. `yes` approves them without asking; without a terminal,
/// changes need `yes`.
pub fn approve_changes<P: Pinned>(
  items: Vec<P>,
  lockfile: &CliLockfile,
  yes: bool,
) -> Result<(), AnyError> {
  let mut changed = Vec::new();
  let mut recorded = Vec::new();
  for item in items {
    let approved = item.approved(&lockfile.lock());
    if !item.changed_parts(approved.as_ref()).is_empty() {
      changed.push((item, approved));
      continue;
    }
    let pin = item.pin(approved.as_ref());
    if approved.as_ref() != Some(&pin) {
      item.record(&mut lockfile.lock(), pin);
      recorded.push(item);
    }
  }
  if changed.is_empty() {
    return write_and_report(lockfile, &[], &recorded);
  }
  for (item, approved) in &changed {
    log::info!("{}", item.describe_change(approved.as_ref()));
  }
  let approve = yes
    || match crate::util::console::confirm(
      crate::util::console::ConfirmOptions {
        message: format!("Approve these {} changes?", P::KIND),
        default: false,
      },
    ) {
      Some(answer) => answer,
      None => {
        return Err(
          JsErrorBox::type_error(format!(
            "{} changes need approval: run `ende install` in a terminal to review them, or `ende install --yes` to approve them. {} was not changed.",
            capitalized(P::KIND),
            lockfile_name(lockfile),
          ))
          .into(),
        );
      }
    };
  if !approve {
    return Err(
      JsErrorBox::type_error(format!(
        "{} changes not approved; {} was not changed.",
        capitalized(P::KIND),
        lockfile_name(lockfile),
      ))
      .into(),
    );
  }
  let mut approved_items = Vec::with_capacity(changed.len());
  for (item, approved) in changed {
    let pin = item.pin(approved.as_ref());
    item.record(&mut lockfile.lock(), pin);
    approved_items.push(item);
  }
  write_and_report(lockfile, &approved_items, &recorded)
}

/// Writes what `ende install` recorded, then says so: one line per approved
/// change and per new item. Nothing is said if the write fails (for example
/// with `--frozen`).
fn write_and_report<P: Pinned>(
  lockfile: &CliLockfile,
  approved: &[P],
  recorded: &[P],
) -> Result<(), AnyError> {
  lockfile.write_if_changed()?;
  for item in approved {
    log::info!(
      "Recorded the approved changes to {} in {}.",
      item.name(),
      lockfile_name(lockfile)
    );
  }
  for item in recorded {
    print_recorded_notice(item, lockfile);
  }
  Ok(())
}

/// Prints the line for an item first recorded in the lockfile (or recorded
/// again after its pin was lost). It is shown even with --quiet: a record
/// is the visible sign that a pin was reset, so it is never silent.
fn print_recorded_notice<P: Pinned>(item: &P, lockfile: &CliLockfile) {
  if !item.notice() {
    return;
  }
  #[allow(clippy::print_stderr, reason = "must show even with --quiet")]
  {
    eprintln!(
      "Recorded {} in {}; it will run with: {}",
      item.name(),
      lockfile_name(lockfile),
      describe_permissions(item.options())
    );
  }
}

/// A one-line summary of effective permissions, like
/// `read /data; net api.example.com`.
pub fn describe_permissions(options: &PermissionsOptions) -> String {
  let kinds = [
    ("read", &options.allow_read),
    ("write", &options.allow_write),
    ("net", &options.allow_net),
    ("env", &options.allow_env),
    ("run", &options.allow_run),
    ("ffi", &options.allow_ffi),
    ("sys", &options.allow_sys),
    ("import", &options.allow_import),
  ];
  let parts = kinds
    .iter()
    .filter_map(|(kind, allow)| {
      let allow = allow.as_ref()?;
      Some(if allow.is_empty() && *kind == "write" {
        // Ende fences scoped writes off from the app's code (see
        // `ende_permissions::check_code_load`); everything can't be.
        "write (all; can escape: it can change the app's code)".to_string()
      } else if allow.is_empty() {
        format!("{kind} (all)")
      } else {
        format!("{kind} {}", allow.join(", "))
      })
    })
    .collect::<Vec<_>>();
  if parts.is_empty() {
    "no permissions".to_string()
  } else {
    parts.join("; ")
  }
}

pub fn lockfile_name(lockfile: &CliLockfile) -> String {
  lockfile
    .filename
    .file_name()
    .map(|name| name.to_string_lossy().into_owned())
    .unwrap_or_else(|| "the lockfile".to_string())
}

fn capitalized(text: &str) -> String {
  let mut chars = text.chars();
  match chars.next() {
    Some(first) => first.to_uppercase().chain(chars).collect(),
    None => String::new(),
  }
}
