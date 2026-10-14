// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(clippy::print_stderr)]
#![deny(clippy::print_stdout)]

use std::borrow::Cow;
use std::path::PathBuf;

use boxed_error::Boxed;
use deno_error::JsError;
use deno_semver::npm::NpmPackageReqReference;
use ende_cache_dir::npm::NpmCacheDir;
pub use node_resolver::DenoIsBuiltInNodeModuleChecker;
use node_resolver::InNpmPackageChecker;
use node_resolver::IsBuiltInNodeModuleChecker;
use node_resolver::NodeResolutionKind;
pub use node_resolver::NodeResolverOptions;
use node_resolver::NodeResolverRc;
use node_resolver::NpmPackageFolderResolver;
use node_resolver::ResolutionMode;
use node_resolver::UrlOrPath;
use node_resolver::errors::NodeJsErrorCode;
use node_resolver::errors::NodeResolveError;
use node_resolver::errors::NodeResolveErrorKind;
use node_resolver::errors::UnknownBuiltInNodeModuleError;
use npm::NodeModulesOutOfDateError;
use npm::NpmReqResolverRc;
use npm::ResolveIfForNpmPackageErrorKind;
use npm::ResolvePkgFolderFromDenoReqError;
use thiserror::Error;
use url::Url;

use self::npm::NpmResolver;
use self::npm::NpmResolverSys;
use self::npm::ResolveNpmReqRefError;
use crate::workspace::MappedResolution;
use crate::workspace::MappedResolutionDiagnostic;
use crate::workspace::MappedResolutionError;
use crate::workspace::WorkspaceResolver;

pub mod cache;
pub mod cjs;
pub mod collections;
pub mod display;
#[cfg(feature = "deno_ast")]
pub mod emit;
pub mod ende_json;
pub mod factory;
#[cfg(feature = "graph")]
pub mod file_fetcher;
#[cfg(feature = "graph")]
pub mod graph;
pub mod import_map;
pub mod loader;
pub mod lockfile;
pub mod npm;
pub mod npmrc;
#[cfg(feature = "sync")]
mod rt;
pub mod workspace;

#[allow(clippy::disallowed_types, reason = "definition")]
pub type WorkspaceResolverRc<TSys> =
  ende_maybe_sync::MaybeArc<WorkspaceResolver<TSys>>;

#[allow(clippy::disallowed_types, reason = "definition")]
pub(crate) type NpmCacheDirRc = ende_maybe_sync::MaybeArc<NpmCacheDir>;

#[derive(Debug, Clone)]
pub struct EndeResolution {
  pub url: Url,
  pub maybe_diagnostic: Option<Box<MappedResolutionDiagnostic>>,
}

#[derive(Debug, Boxed, JsError)]
pub struct EndeResolveError(pub Box<EndeResolveErrorKind>);

impl EndeResolveError {
  #[cfg(feature = "graph")]
  pub fn into_deno_graph_error(self) -> deno_graph::source::ResolveError {
    use deno_graph::source::ResolveError;

    match self.into_kind() {
      EndeResolveErrorKind::MappedResolution(mapped_resolution_error) => {
        match mapped_resolution_error {
          MappedResolutionError::Specifier(e) => ResolveError::Specifier(e),
          // deno_graph checks specifically for an ImportMapError
          MappedResolutionError::ImportMap(e) => ResolveError::from_err(e),
          MappedResolutionError::Workspace(e) => ResolveError::from_err(e),
          MappedResolutionError::NotFoundInCompilerOptionsPaths(e) => {
            ResolveError::from_err(e)
          }
        }
      }
      err => ResolveError::from_err(err),
    }
  }

  pub fn maybe_specifier(&self) -> Option<Cow<'_, UrlOrPath>> {
    match self.as_kind() {
      EndeResolveErrorKind::Node(err) => err.maybe_specifier(),
      EndeResolveErrorKind::PathToUrl(err) => {
        Some(Cow::Owned(UrlOrPath::Path(err.0.clone())))
      }
      EndeResolveErrorKind::ResolveNpmReqRef(err) => err.err.maybe_specifier(),
      EndeResolveErrorKind::MappedResolution(_)
      | EndeResolveErrorKind::ResolvePkgFolderFromDenoReq(_)
      | EndeResolveErrorKind::InvalidVendorFolderImport
      | EndeResolveErrorKind::NodeModulesOutOfDate(_) => None,
    }
  }
}

#[derive(Debug, Error, JsError)]
pub enum EndeResolveErrorKind {
  #[class(type)]
  #[error(
    "Importing from the vendor directory is not permitted. Use a remote specifier instead or disable vendoring."
  )]
  InvalidVendorFolderImport,
  #[class(inherit)]
  #[error(transparent)]
  MappedResolution(#[from] MappedResolutionError),
  #[class(inherit)]
  #[error(transparent)]
  Node(#[from] NodeResolveError),
  #[class(inherit)]
  #[error(transparent)]
  NodeModulesOutOfDate(#[from] NodeModulesOutOfDateError),
  #[class(inherit)]
  #[error(transparent)]
  PathToUrl(#[from] deno_path_util::PathToUrlError),
  #[class(inherit)]
  #[error(transparent)]
  ResolveNpmReqRef(#[from] ResolveNpmReqRefError),
  #[class(inherit)]
  #[error(transparent)]
  ResolvePkgFolderFromDenoReq(#[from] ResolvePkgFolderFromDenoReqError),
}

impl EndeResolveErrorKind {
  pub fn maybe_node_code(&self) -> Option<NodeJsErrorCode> {
    match self {
      EndeResolveErrorKind::InvalidVendorFolderImport
      | EndeResolveErrorKind::MappedResolution { .. }
      | EndeResolveErrorKind::NodeModulesOutOfDate { .. }
      | EndeResolveErrorKind::PathToUrl { .. }
      | EndeResolveErrorKind::ResolvePkgFolderFromDenoReq { .. } => None,
      EndeResolveErrorKind::ResolveNpmReqRef(err) => {
        err.err.as_kind().maybe_code()
      }
      EndeResolveErrorKind::Node(err) => err.as_kind().maybe_code(),
    }
  }
}

#[derive(Debug)]
pub struct NodeAndNpmResolvers<
  TInNpmPackageChecker: InNpmPackageChecker,
  TIsBuiltInNodeModuleChecker: IsBuiltInNodeModuleChecker,
  TNpmPackageFolderResolver: NpmPackageFolderResolver,
  TSys: NpmResolverSys,
> {
  pub node_resolver: NodeResolverRc<
    TInNpmPackageChecker,
    TIsBuiltInNodeModuleChecker,
    TNpmPackageFolderResolver,
    TSys,
  >,
  pub npm_resolver: NpmResolver<TSys>,
  pub npm_req_resolver: NpmReqResolverRc<
    TInNpmPackageChecker,
    TIsBuiltInNodeModuleChecker,
    TNpmPackageFolderResolver,
    TSys,
  >,
}

#[sys_traits::auto_impl]
pub trait EndeResolverSys: NpmResolverSys {}

pub struct EndeResolverOptions<
  'a,
  TInNpmPackageChecker: InNpmPackageChecker,
  TIsBuiltInNodeModuleChecker: IsBuiltInNodeModuleChecker,
  TNpmPackageFolderResolver: NpmPackageFolderResolver,
  TSys: EndeResolverSys,
> {
  pub in_npm_pkg_checker: TInNpmPackageChecker,
  pub node_and_req_resolver: Option<
    NodeAndNpmResolvers<
      TInNpmPackageChecker,
      TIsBuiltInNodeModuleChecker,
      TNpmPackageFolderResolver,
      TSys,
    >,
  >,
  pub workspace_resolver: WorkspaceResolverRc<TSys>,
  /// Whether "bring your own node_modules" is enabled where Ende does not
  /// setup the node_modules directories automatically, but instead uses
  /// what already exists on the file system.
  pub is_byonm: bool,
  pub maybe_vendor_dir: Option<&'a PathBuf>,
}

#[allow(clippy::disallowed_types, reason = "definition")]
pub type RawEndeResolverRc<
  TInNpmPackageChecker,
  TIsBuiltInNodeModuleChecker,
  TNpmPackageFolderResolver,
  TSys,
> = ende_maybe_sync::MaybeArc<
  RawEndeResolver<
    TInNpmPackageChecker,
    TIsBuiltInNodeModuleChecker,
    TNpmPackageFolderResolver,
    TSys,
  >,
>;

/// Helper type for a RawEndeResolverRc that has the implementations
/// used by the Ende CLI.
pub type DefaultRawEndeResolverRc<TSys> = RawEndeResolverRc<
  npm::DenoInNpmPackageChecker,
  DenoIsBuiltInNodeModuleChecker,
  npm::NpmResolver<TSys>,
  TSys,
>;

/// A resolver that takes care of resolution, taking into account loaded
/// import map, JSX settings.
#[derive(Debug)]
pub struct RawEndeResolver<
  TInNpmPackageChecker: InNpmPackageChecker,
  TIsBuiltInNodeModuleChecker: IsBuiltInNodeModuleChecker,
  TNpmPackageFolderResolver: NpmPackageFolderResolver,
  TSys: EndeResolverSys,
> {
  in_npm_pkg_checker: TInNpmPackageChecker,
  node_and_npm_resolver: Option<
    NodeAndNpmResolvers<
      TInNpmPackageChecker,
      TIsBuiltInNodeModuleChecker,
      TNpmPackageFolderResolver,
      TSys,
    >,
  >,
  workspace_resolver: WorkspaceResolverRc<TSys>,
  is_byonm: bool,
  maybe_vendor_specifier: Option<Url>,
}

impl<
  TInNpmPackageChecker: InNpmPackageChecker,
  TIsBuiltInNodeModuleChecker: IsBuiltInNodeModuleChecker,
  TNpmPackageFolderResolver: NpmPackageFolderResolver,
  TSys: EndeResolverSys,
>
  RawEndeResolver<
    TInNpmPackageChecker,
    TIsBuiltInNodeModuleChecker,
    TNpmPackageFolderResolver,
    TSys,
  >
{
  pub fn new(
    options: EndeResolverOptions<
      TInNpmPackageChecker,
      TIsBuiltInNodeModuleChecker,
      TNpmPackageFolderResolver,
      TSys,
    >,
  ) -> Self {
    Self {
      in_npm_pkg_checker: options.in_npm_pkg_checker,
      node_and_npm_resolver: options.node_and_req_resolver,
      workspace_resolver: options.workspace_resolver,
      is_byonm: options.is_byonm,
      maybe_vendor_specifier: options
        .maybe_vendor_dir
        .and_then(|v| deno_path_util::url_from_directory_path(v).ok()),
    }
  }

  pub fn resolve(
    &self,
    raw_specifier: &str,
    referrer: &Url,
    resolution_mode: ResolutionMode,
    resolution_kind: NodeResolutionKind,
  ) -> Result<EndeResolution, EndeResolveError> {
    let mut maybe_diagnostic = None;
    // Use node resolution if we're in an npm package
    if let Some(node_and_npm_resolver) = self.node_and_npm_resolver.as_ref() {
      let node_resolver = &node_and_npm_resolver.node_resolver;
      if referrer.scheme() == "file"
        && self.in_npm_pkg_checker.in_npm_package(referrer)
      {
        log::debug!(
          "{}: specifier={} referrer={} mode={:?} kind={:?}",
          deno_terminal::colors::magenta("resolving in npm package"),
          raw_specifier,
          referrer,
          resolution_mode,
          resolution_kind
        );
        return node_resolver
          .resolve(raw_specifier, referrer, resolution_mode, resolution_kind)
          .and_then(|res| {
            Ok(EndeResolution {
              url: res.into_url()?,
              maybe_diagnostic,
            })
          })
          .map_err(|e| e.into());
      }
    }

    // Attempt to resolve with the workspace resolver
    let result = self.workspace_resolver.resolve(
      raw_specifier,
      referrer,
      resolution_kind.into(),
    );
    let result = match result {
      Ok(resolution) => match resolution {
        MappedResolution::Normal {
          specifier,
          maybe_diagnostic: current_diagnostic,
          ..
        } => {
          maybe_diagnostic = current_diagnostic;
          Ok(specifier)
        }
        MappedResolution::WorkspaceJsrPackage { specifier, .. } => {
          Ok(specifier)
        }
      },
      Err(err) => Err(err.into()),
    };

    // When the user is vendoring, don't allow them to import directly from the vendor/ directory
    // as it might cause them confusion or duplicate dependencies. Additionally, this folder has
    // special treatment in the language server so it will definitely cause issues/confusion there
    // if they do this.
    if let Some(vendor_specifier) = &self.maybe_vendor_specifier
      && let Ok(specifier) = &result
      && specifier.as_str().starts_with(vendor_specifier.as_str())
    {
      return Err(EndeResolveErrorKind::InvalidVendorFolderImport.into_box());
    }

    let Some(NodeAndNpmResolvers {
      node_resolver,
      npm_req_resolver,
      ..
    }) = &self.node_and_npm_resolver
    else {
      return Ok(EndeResolution {
        url: result?,
        maybe_diagnostic,
      });
    };

    match result {
      Ok(specifier) => {
        if specifier.scheme() == "node" {
          let module_name = specifier.path();
          return if node_resolver.is_builtin_node_module(module_name) {
            Ok(EndeResolution {
              url: specifier,
              maybe_diagnostic,
            })
          } else {
            Err(
              NodeResolveErrorKind::UnknownBuiltInNodeModule(
                UnknownBuiltInNodeModuleError {
                  module_name: module_name.to_string(),
                },
              )
              .into_box()
              .into(),
            )
          };
        }

        if self.is_byonm
          && let Ok(npm_req_ref) =
            NpmPackageReqReference::from_specifier(&specifier)
        {
          return npm_req_resolver
            .resolve_req_reference(
              &npm_req_ref,
              referrer,
              resolution_mode,
              resolution_kind,
            )
            .map_err(|err| {
              EndeResolveErrorKind::ResolveNpmReqRef(err).into_box()
            })
            .and_then(|url_or_path| {
              Ok(EndeResolution {
                url: url_or_path.into_url()?,
                maybe_diagnostic,
              })
            });
        }

        Ok(EndeResolution {
          url: node_resolver
            .handle_if_in_node_modules(&specifier)
            .unwrap_or(specifier),
          maybe_diagnostic,
        })
      }
      Err(err) => {
        // Check if the bare specifier is a known Node built-in module.
        // Built-ins always take priority, matching Node.js behavior where
        // built-in modules cannot be shadowed by node_modules packages.
        if node_resolver.is_builtin_node_module(raw_specifier) {
          return Ok(EndeResolution {
            url: Url::parse(&format!("node:{}", raw_specifier)).unwrap(),
            maybe_diagnostic,
          });
        }

        // If byonm, check if the bare specifier resolves to an npm package
        if self.is_byonm && referrer.scheme() == "file" {
          let maybe_resolution = npm_req_resolver
            .resolve_if_for_npm_pkg(
              raw_specifier,
              referrer,
              resolution_mode,
              resolution_kind,
            )
            .map_err(|e| match e.into_kind() {
              ResolveIfForNpmPackageErrorKind::NodeResolve(e) => {
                EndeResolveErrorKind::Node(e).into_box()
              }
              ResolveIfForNpmPackageErrorKind::NodeModulesOutOfDate(e) => {
                e.into()
              }
            })?;
          if let Some(res) = maybe_resolution {
            return Ok(EndeResolution {
              url: res.into_url()?,
              maybe_diagnostic,
            });
          }
        }

        Err(err)
      }
    }
  }

  #[cfg(feature = "graph")]
  pub(crate) fn resolve_non_workspace_npm_req_ref_to_file(
    &self,
    npm_req_ref: &NpmPackageReqReference,
    referrer: &Url,
    resolution_mode: ResolutionMode,
    resolution_kind: NodeResolutionKind,
  ) -> Result<node_resolver::UrlOrPath, npm::ResolveNpmReqRefError> {
    let Some(NodeAndNpmResolvers {
      npm_req_resolver, ..
    }) = &self.node_and_npm_resolver
    else {
      return Err(npm::ResolveNpmReqRefError {
        npm_req_ref: npm_req_ref.clone(),
        err: npm::ResolveReqWithSubPathErrorKind::NoNpm(npm::NoNpmError)
          .into_box(),
      });
    };
    npm_req_resolver.resolve_req_reference(
      npm_req_ref,
      referrer,
      resolution_mode,
      resolution_kind,
    )
  }
}

/// Whether the React CVE source patches are enabled. Opt in by setting the
/// `ENDE_PATCH_REACT_CVE` environment variable to a non-empty value other than
/// `0`.
///
/// The variable is read exactly once (via `sys`) and cached. Module loading
/// happens before any user code runs, so the value is effectively snapshotted
/// at startup and cannot be toggled later via `Ende.env.set`.
pub fn is_react_cve_patch_enabled(sys: &impl sys_traits::EnvVar) -> bool {
  static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
  *ENABLED.get_or_init(|| {
    ende_env::env_var(sys, "ENDE_PATCH_REACT_CVE")
      .map(|v| !v.is_empty() && v != "0")
      .unwrap_or(false)
  })
}

/// Load time source patches that neutralize two known React Server Components
/// vulnerabilities shipped in affected `react-server-dom-*` builds:
///
/// * CVE-2025-55182 (RCE): deserialized model keys are not filtered, letting a
///   crafted payload reach `constructor` / `prototype` / `_response`.
/// * CVE-2025-55184 (DoS): a cyclic thenable makes chunk fulfillment loop
///   forever.
///
/// Callers must gate this behind [`is_react_cve_patch_enabled`] (the patches
/// are opt-in via the `ENDE_PATCH_REACT_CVE` environment variable) and only
/// apply it to JavaScript source.
pub fn patch_react_cves<'a>(
  filename: &str,
  code: Cow<'a, str>,
) -> Cow<'a, str> {
  let mut modified = Cow::Borrowed(&*code);
  let mut offset = 0;

  for (index, _) in code.match_indices("split(") {
    let index = index + offset;

    let len = "split(".len();
    if !modified[index + len..].starts_with("':')")
      && !modified[index + len..].starts_with("\":\")")
    {
      continue;
    }

    let Some((end, _)) = modified[index..].char_indices().nth(100) else {
      break;
    };

    if !modified[index..index + end].contains("resolved_model") {
      continue;
    }

    let insert =
      ".filter(x => !(['constructor', 'prototype', '_response'].includes(x)))";
    let mut s = String::with_capacity(modified.len() + insert.len());
    s.push_str(&modified[0..index + len + 4]);
    s.push_str(insert);
    offset += insert.len();
    s.push_str(&modified[(index + len + 4)..]);
    modified = Cow::Owned(s);
  }

  if offset > 0 {
    log::debug!("Patched React RCE (CVE-2025-55182) in {filename}");
  }

  // Collapse the stage 1 (RCE) result back into a `Cow<'a>` before stage 2
  // borrows it: the original input when nothing was patched, otherwise the
  // owned patched string. Stage 2 must fall back to this (not `code`) so a
  // stage 1 patch is preserved even when stage 2 makes no change.
  let stage1: Cow<'a, str> = match modified {
    Cow::Borrowed(_) => code,
    Cow::Owned(s) => Cow::Owned(s),
  };

  let code2 = &*stage1;
  let mut modified = Cow::Borrowed(&*stage1);
  let mut offset = 0;

  const PROTOTYPE_THEN: &str = ".prototype.then";
  'outer: for (index, _) in code2.match_indices(PROTOTYPE_THEN) {
    let index = index + offset;

    let after_equals;
    if modified[index + PROTOTYPE_THEN.len()..].starts_with(" =") {
      after_equals = index + PROTOTYPE_THEN.len() + 2;
    } else if modified[index + PROTOTYPE_THEN.len()..].starts_with("=") {
      after_equals = index + PROTOTYPE_THEN.len() + 1;
    } else {
      continue;
    };

    let Some((end, _)) = modified[index..].char_indices().nth(300) else {
      break;
    };

    if !modified[index..index + end].contains("resolved_model") {
      continue;
    }

    if !modified[index..index + end].contains("fulfilled") {
      continue;
    }

    // now match { until we end up at the next }
    let mut brace_count = 0;
    let mut insert_index = None;
    for (i, c) in modified[index..].char_indices() {
      if c == '{' {
        brace_count += 1;
      } else if c == '}' {
        if brace_count == 0 {
          continue 'outer;
        }
        brace_count -= 1;
        if brace_count == 0 {
          insert_index = Some(index + i + 1);
          break;
        }
      }
    }
    let Some(insert_index) = insert_index else {
      continue;
    };

    const INSERT_BEFORE: &str = "(()=>{const __old=";
    const INSERT_AFTER: &str = ";const __new=function(res, rej){return __old.call(this,(v)=>{let w=v;let l=0;while(w&&typeof w==='object'&&w.then===__new){l++;if(w===this||l>1000){if(typeof rej==='function')rej(new Error('Cannot have cyclic thenables.'));return}if(w.status==='fulfilled')w=w.value;else break;}res(v)},rej)};return __new})()";

    // after the = insert the before, and after the } insert the after
    let mut s = String::with_capacity(
      modified.len() + INSERT_BEFORE.len() + INSERT_AFTER.len(),
    );
    s.push_str(&modified[0..after_equals]);
    s.push_str(INSERT_BEFORE);
    s.push_str(&modified[after_equals..insert_index]);
    s.push_str(INSERT_AFTER);
    s.push_str(&modified[insert_index..]);
    offset += INSERT_BEFORE.len() + INSERT_AFTER.len();
    modified = Cow::Owned(s);
  }

  if offset > 0 {
    log::debug!("Patched React DoS (CVE-2025-55184) in {filename}");
  }

  match modified {
    Cow::Borrowed(_) => stage1,
    Cow::Owned(s) => Cow::Owned(s),
  }
}
