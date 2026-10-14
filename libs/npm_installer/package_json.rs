// Copyright 2018-2026 the Deno authors. MIT license.

use std::path::PathBuf;
use std::sync::Arc;

use deno_semver::SmallStackString;
use deno_semver::StackString;
use deno_semver::Version;
use deno_semver::VersionReq;
use deno_semver::npm::NpmPackageReqReference;
use deno_semver::package::PackageNv;
use deno_semver::package::PackageReq;
use ende_config::workspace::Workspace;
use ende_package_json::PackageJsonDepValueParseError;
use serde_json;
use thiserror::Error;
use url::Url;

#[derive(Debug)]
pub struct InstallNpmRemotePkg {
  pub alias: Option<StackString>,
  pub base_dir: PathBuf,
  pub req: PackageReq,
}

#[derive(Debug)]
pub struct InstallLocalPkg {
  pub alias: Option<StackString>,
  pub target_dir: PathBuf,
}

#[derive(Debug)]
pub struct InstallPatchPkg {
  pub nv: PackageNv,
  pub target_dir: PathBuf,
}

#[derive(Debug)]
pub enum InstallWorkspacePkgDep {
  Remote { alias: StackString, req: PackageReq },
  Workspace { alias: StackString, nv: PackageNv },
}

impl InstallWorkspacePkgDep {
  /// The name the dependency is linked under in the member's `node_modules`.
  pub fn alias(&self) -> &StackString {
    match self {
      InstallWorkspacePkgDep::Remote { alias, .. } => alias,
      InstallWorkspacePkgDep::Workspace { alias, .. } => alias,
    }
  }
}

#[derive(Debug)]
pub struct InstallWorkspacePkg {
  pub nv: PackageNv,
  pub target_dir: PathBuf,
  /// Whether this package is the workspace root. The root's `node_modules`
  /// is set up separately, so the per-member linking must skip it.
  pub is_root: bool,
  pub scripts: std::collections::HashMap<SmallStackString, String>,
  pub deps: Vec<InstallWorkspacePkgDep>,
}

#[derive(Debug, Error, Clone)]
#[error("Failed to install '{}'\n    at {}", alias, location)]
pub struct PackageJsonDepValueParseWithLocationError {
  pub location: Url,
  pub alias: StackString,
  #[source]
  pub source: PackageJsonDepValueParseError,
}

/// A `workspace:<version>` dependency referenced a local workspace member by
/// name, but the member's version did not satisfy the requested constraint.
/// Like pnpm, this is a hard error rather than silently linking the member or
/// falling back to the registry. The message mirrors the equivalent
/// `deno run` resolver error (`VersionNotSatisfied`).
#[derive(Debug, Error, Clone)]
#[error(
  "Failed to install '{alias}'{}: found package.json in workspace, but version '{version}' didn't satisfy constraint '{version_req}'\n    at {location}",
  self.member_suffix()
)]
pub struct WorkspaceMemberVersionNotSatisfiedError {
  pub location: Url,
  /// The dependency key the member is imported under.
  pub alias: StackString,
  /// The resolved workspace member name. Differs from `alias` for pnpm-style
  /// `workspace:<name>@<range>` aliases.
  pub member_name: StackString,
  pub version_req: VersionReq,
  pub version: Version,
}

impl WorkspaceMemberVersionNotSatisfiedError {
  /// When the member is imported under a different name (an alias), append the
  /// resolved member name so the offending entry is easy to locate.
  fn member_suffix(&self) -> String {
    if self.member_name == self.alias {
      String::new()
    } else {
      format!(" (workspace member '{}')", self.member_name)
    }
  }
}

/// An error surfaced while reconciling a package.json's dependencies against
/// the workspace before installing.
#[derive(Debug, Error, Clone)]
pub enum EnsurePackageJsonDepsError {
  #[error(transparent)]
  DepValueParse(#[from] Box<PackageJsonDepValueParseWithLocationError>),
  #[error(transparent)]
  WorkspaceMemberVersionNotSatisfied(
    #[from] Box<WorkspaceMemberVersionNotSatisfiedError>,
  ),
}

#[derive(Debug, Default)]
pub struct NpmInstallDepsProvider {
  remote_pkgs: Vec<InstallNpmRemotePkg>,
  local_pkgs: Vec<InstallLocalPkg>,
  patch_pkgs: Vec<InstallPatchPkg>,
  workspace_pkgs: Vec<InstallWorkspacePkg>,
  pkg_json_dep_errors: Vec<PackageJsonDepValueParseWithLocationError>,
  workspace_member_version_errors: Vec<WorkspaceMemberVersionNotSatisfiedError>,
}

impl NpmInstallDepsProvider {
  pub fn empty() -> Self {
    Self::default()
  }

  /// Collects the `npm:` imports of the workspace's ende.json files.
  /// package.json files are not supported in Ende.
  pub fn from_workspace(
    workspace: &Arc<Workspace>,
    _production: bool,
    skip_types: bool,
  ) -> Self {
    let mut remote_pkgs = Vec::new();

    for folder in workspace.config_folders().values() {
      // deal with the deno.json first because it takes precedence during resolution
      if let Some(deno_json) = &folder.ende_json {
        // don't bother with externally referenced import maps as users
        // should inline their import map to get this behaviour
        if let Some(serde_json::Value::Object(obj)) = &deno_json.json.imports {
          let mut pkg_pkgs = Vec::with_capacity(obj.len());
          for value in obj.values() {
            let serde_json::Value::String(specifier) = value else {
              continue;
            };
            let Ok(npm_req_ref) = NpmPackageReqReference::from_str(specifier)
            else {
              continue;
            };
            let pkg_req = npm_req_ref.into_inner().req;

            if skip_types && pkg_req.name.starts_with("@types/") {
              continue;
            }

            pkg_pkgs.push(InstallNpmRemotePkg {
              alias: None,
              base_dir: deno_json.dir_path(),
              req: pkg_req,
            });
          }

          // sort within each package (more like npm resolution)
          pkg_pkgs.sort_by(|a, b| a.req.cmp(&b.req));
          remote_pkgs.extend(pkg_pkgs);
        }
      }
    }

    remote_pkgs.shrink_to_fit();
    Self {
      remote_pkgs,
      ..Default::default()
    }
  }

  pub fn remote_pkgs(&self) -> &[InstallNpmRemotePkg] {
    &self.remote_pkgs
  }

  pub fn local_pkgs(&self) -> &[InstallLocalPkg] {
    &self.local_pkgs
  }

  pub fn patch_pkgs(&self) -> &[InstallPatchPkg] {
    &self.patch_pkgs
  }

  pub fn workspace_pkgs(&self) -> &[InstallWorkspacePkg] {
    &self.workspace_pkgs
  }

  pub fn pkg_json_dep_errors(
    &self,
  ) -> &[PackageJsonDepValueParseWithLocationError] {
    &self.pkg_json_dep_errors
  }

  pub fn workspace_member_version_errors(
    &self,
  ) -> &[WorkspaceMemberVersionNotSatisfiedError] {
    &self.workspace_member_version_errors
  }
}
