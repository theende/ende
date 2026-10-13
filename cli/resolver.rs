// Copyright 2018-2026 the Deno authors. MIT license.

use ende_resolver::npm::DenoInNpmPackageChecker;
use node_resolver::DenoIsBuiltInNodeModuleChecker;

use crate::npm::CliNpmResolver;
use crate::sys::CliSys;

pub type CliCjsTracker =
  ende_resolver::cjs::CjsTracker<DenoInNpmPackageChecker, CliSys>;
pub type CliIsCjsResolver =
  ende_resolver::cjs::IsCjsResolver<DenoInNpmPackageChecker, CliSys>;
pub type CliNpmReqResolver<TSys = CliSys> = ende_resolver::npm::NpmReqResolver<
  DenoInNpmPackageChecker,
  DenoIsBuiltInNodeModuleChecker,
  CliNpmResolver<TSys>,
  TSys,
>;
pub type CliResolver = ende_resolver::graph::EndeResolver<
  DenoInNpmPackageChecker,
  DenoIsBuiltInNodeModuleChecker,
  CliNpmResolver,
  CliSys,
>;

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
