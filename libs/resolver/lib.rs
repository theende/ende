// Copyright 2018-2026 the Deno authors. MIT license.

#![deny(clippy::print_stderr)]
#![deny(clippy::print_stdout)]

use std::borrow::Cow;
use std::path::PathBuf;

use boxed_error::Boxed;
use deno_error::JsError;
use thiserror::Error;
use url::Url;

use crate::workspace::MappedResolution;
use crate::workspace::MappedResolutionDiagnostic;
use crate::workspace::MappedResolutionError;
use crate::workspace::ResolutionKind;
use crate::workspace::WorkspaceResolver;

pub mod cache;
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
#[cfg(all(feature = "sync", feature = "deno_ast"))]
mod rt;
pub mod workspace;

/// A specifier for something Ende doesn't support: Node built-in modules,
/// npm packages and CommonJS modules.
#[derive(Debug, Clone, Error, JsError)]
#[class(type)]
pub enum UnsupportedSpecifierError {
  #[error("\"{0}\" — Node built-in modules are not supported in Ende.")]
  NodeBuiltin(String),
  #[error("\"{0}\" — npm packages are not supported in Ende.")]
  Npm(String),
  #[error("\"{0}\" — CommonJS modules are not supported in Ende.")]
  CommonJs(String),
}

/// Errors for a resolved specifier Ende can't load: a `node:` built-in, an
/// `npm:` package or a CommonJS (`.cjs`/`.cts`) module.
pub fn check_specifier_supported(
  specifier: &Url,
) -> Result<(), UnsupportedSpecifierError> {
  match specifier.scheme() {
    "node" => Err(UnsupportedSpecifierError::NodeBuiltin(
      specifier.to_string(),
    )),
    "npm" => Err(UnsupportedSpecifierError::Npm(specifier.to_string())),
    _ => {
      if matches!(
        deno_media_type::MediaType::from_specifier(specifier),
        deno_media_type::MediaType::Cjs | deno_media_type::MediaType::Cts
      ) {
        Err(UnsupportedSpecifierError::CommonJs(specifier.to_string()))
      } else {
        Ok(())
      }
    }
  }
}

/// Node.js built-in module names, used only to give bare imports of them
/// (e.g. `import fs from "fs"`) a clear "not supported in Ende" error.
static NODE_BUILTIN_MODULE_NAMES: &[&str] = &[
  "_http_agent",
  "_http_common",
  "_http_outgoing",
  "_http_server",
  "_stream_duplex",
  "_stream_passthrough",
  "_stream_readable",
  "_stream_transform",
  "_stream_writable",
  "_tls_common",
  "_tls_wrap",
  "assert",
  "assert/strict",
  "async_hooks",
  "buffer",
  "child_process",
  "cluster",
  "console",
  "constants",
  "crypto",
  "dgram",
  "diagnostics_channel",
  "dns",
  "dns/promises",
  "domain",
  "events",
  "fs",
  "fs/promises",
  "http",
  "http2",
  "https",
  "inspector",
  "inspector/promises",
  "module",
  "net",
  "os",
  "path",
  "path/posix",
  "path/win32",
  "perf_hooks",
  "process",
  "punycode",
  "querystring",
  "readline",
  "readline/promises",
  "repl",
  "sqlite",
  "stream",
  "stream/consumers",
  "stream/promises",
  "stream/web",
  "string_decoder",
  "sys",
  "test",
  "test/reporters",
  "timers",
  "timers/promises",
  "tls",
  "trace_events",
  "tty",
  "url",
  "util",
  "util/types",
  "v8",
  "vm",
  "wasi",
  "worker_threads",
  "zlib",
];

/// Whether the bare specifier names a Node.js built-in module.
pub fn is_node_builtin_module_name(specifier: &str) -> bool {
  NODE_BUILTIN_MODULE_NAMES.contains(&specifier)
}

#[allow(clippy::disallowed_types, reason = "definition")]
pub type WorkspaceResolverRc<TSys> =
  ende_maybe_sync::MaybeArc<WorkspaceResolver<TSys>>;

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
  UnsupportedSpecifier(#[from] UnsupportedSpecifierError),
}

#[sys_traits::auto_impl]
pub trait EndeResolverSys:
  sys_traits::FsCanonicalize
  + sys_traits::FsMetadata
  + sys_traits::FsRead
  + sys_traits::FsReadDir
  + std::fmt::Debug
  + ende_maybe_sync::MaybeSend
  + ende_maybe_sync::MaybeSync
  + Clone
  + 'static
{
}

pub struct EndeResolverOptions<'a, TSys: EndeResolverSys> {
  pub workspace_resolver: WorkspaceResolverRc<TSys>,
  pub maybe_vendor_dir: Option<&'a PathBuf>,
}

#[allow(clippy::disallowed_types, reason = "definition")]
pub type RawEndeResolverRc<TSys> =
  ende_maybe_sync::MaybeArc<RawEndeResolver<TSys>>;

/// A resolver that takes care of resolution, taking into account loaded
/// import map, JSX settings.
#[derive(Debug)]
pub struct RawEndeResolver<TSys: EndeResolverSys> {
  workspace_resolver: WorkspaceResolverRc<TSys>,
  maybe_vendor_specifier: Option<Url>,
}

impl<TSys: EndeResolverSys> RawEndeResolver<TSys> {
  pub fn new(options: EndeResolverOptions<TSys>) -> Self {
    Self {
      workspace_resolver: options.workspace_resolver,
      maybe_vendor_specifier: options
        .maybe_vendor_dir
        .and_then(|v| deno_path_util::url_from_directory_path(v).ok()),
    }
  }

  pub fn resolve(
    &self,
    raw_specifier: &str,
    referrer: &Url,
    resolution_kind: ResolutionKind,
  ) -> Result<EndeResolution, EndeResolveError> {
    let mut maybe_diagnostic = None;

    // Attempt to resolve with the workspace resolver
    let result =
      self
        .workspace_resolver
        .resolve(raw_specifier, referrer, resolution_kind);
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
      Err(err) => Err(err),
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

    // Node built-ins, npm packages and CommonJS modules are not supported.
    match result {
      Ok(specifier) => {
        check_specifier_supported(&specifier).map_err(|e| {
          EndeResolveErrorKind::UnsupportedSpecifier(e).into_box()
        })?;
        Ok(EndeResolution {
          url: specifier,
          maybe_diagnostic,
        })
      }
      Err(err) => {
        if is_node_builtin_module_name(raw_specifier) {
          Err(
            EndeResolveErrorKind::UnsupportedSpecifier(
              UnsupportedSpecifierError::NodeBuiltin(raw_specifier.to_string()),
            )
            .into_box(),
          )
        } else {
          Err(EndeResolveErrorKind::MappedResolution(err).into_box())
        }
      }
    }
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
