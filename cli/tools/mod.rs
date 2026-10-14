// Copyright 2018-2026 the Deno authors. MIT license.

pub mod bench;
pub mod bump_version;
pub mod check;
pub mod clean;
pub mod compile;
pub mod coverage;
pub mod deploy;
pub mod desktop;
pub mod desktop_devtools;
pub mod doc;
pub mod fmt;
pub mod fmt_editorconfig;
pub mod info;
pub mod init;
pub mod installer;
pub mod lint;
pub mod pm;
pub mod publish;
pub mod repl;
pub mod run;
pub mod serve;
pub mod task;
pub mod task_cache;
pub mod test;
pub mod transpile;
pub mod unfurl_utils;
pub mod upgrade;
pub mod x;

/// The error for an `npm:` package passed to a command.
pub fn npm_not_supported_error(specifier: &str) -> ende_core::error::AnyError {
  ende_core::anyhow::anyhow!(
    "npm packages are not supported in Ende (found '{}').",
    specifier
  )
}
