// Copyright 2018-2026 the Deno authors. MIT license.

use ende_resolver::cjs::analyzer::DenoCjsCodeAnalyzer;
use ende_resolver::npm::DenoInNpmPackageChecker;
use node_resolver::DenoIsBuiltInNodeModuleChecker;
use node_resolver::analyze::CjsModuleExportAnalyzer;

use crate::npm::CliNpmResolver;
use crate::sys::CliSys;

pub type CliCjsCodeAnalyzer = DenoCjsCodeAnalyzer<CliSys>;

pub type CliCjsModuleExportAnalyzer = CjsModuleExportAnalyzer<
  CliCjsCodeAnalyzer,
  DenoInNpmPackageChecker,
  DenoIsBuiltInNodeModuleChecker,
  CliNpmResolver,
  CliSys,
>;
pub type CliNodeResolver<TSys = CliSys> = ende_runtime::ende_node::NodeResolver<
  DenoInNpmPackageChecker,
  CliNpmResolver<TSys>,
  TSys,
>;
pub type CliPackageJsonResolver<TSys = CliSys> =
  node_resolver::PackageJsonResolver<TSys>;
