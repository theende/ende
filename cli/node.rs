// Copyright 2018-2026 the Deno authors. MIT license.

use ende_resolver::npm::DenoInNpmPackageChecker;

use crate::npm::CliNpmResolver;
use crate::sys::CliSys;

pub type CliNodeResolver<TSys = CliSys> = ende_runtime::ende_node::NodeResolver<
  DenoInNpmPackageChecker,
  CliNpmResolver<TSys>,
  TSys,
>;
pub type CliPackageJsonResolver<TSys = CliSys> =
  node_resolver::PackageJsonResolver<TSys>;
