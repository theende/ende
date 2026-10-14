// Copyright 2018-2026 the Deno authors. MIT license.

use crate::sys::CliSys;

pub type CliResolver = ende_resolver::graph::EndeResolver<CliSys>;

pub fn on_resolve_diagnostic(
  diagnostic: ende_resolver::graph::MappedResolutionDiagnosticWithPosition,
) {
  log::warn!(
    "{} {}\n    at {}:{}",
    ende_runtime::colors::yellow("Warning"),
    diagnostic.diagnostic,
    diagnostic.referrer,
    diagnostic.start
  );
}
