// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;

use boxed_error::Boxed;
use deno_error::JsErrorClass;
use deno_graph::JsrLoadError;
use deno_graph::Module;
use deno_graph::ModuleError;
use deno_graph::ModuleErrorKind;
use deno_graph::ModuleGraphError;
use deno_graph::ModuleLoadError;
use deno_graph::Resolution;
use deno_graph::ResolutionError;
use deno_graph::SpecifierError;
use deno_graph::source::ResolveError;
use deno_media_type::MediaType;
use deno_semver::package::PackageReq;
use import_map::ImportMapErrorKind;
use url::Url;

use crate::EndeResolveError;
use crate::EndeResolverSys;
use crate::RawEndeResolverRc;
use crate::ende_json::JsxImportSourceConfigResolver;
use crate::workspace::MappedResolutionDiagnostic;
use crate::workspace::ResolutionKind;
use crate::workspace::sloppy_imports_resolve;

#[derive(Debug, deno_error::JsError, Boxed)]
pub struct ResolveWithGraphError(pub Box<ResolveWithGraphErrorKind>);

impl ResolveWithGraphError {
  pub fn maybe_specifier(&self) -> Option<&Url> {
    match self.as_kind() {
      ResolveWithGraphErrorKind::Resolution(err) => match err {
        deno_graph::ResolutionError::InvalidDowngrade { specifier, .. }
        | deno_graph::ResolutionError::InvalidJsrHttpsTypesImport {
          specifier,
          ..
        }
        | deno_graph::ResolutionError::InvalidLocalImport {
          specifier, ..
        } => Some(specifier),
        deno_graph::ResolutionError::ResolverError { .. }
        | deno_graph::ResolutionError::InvalidSpecifier { .. } => None,
      },
      ResolveWithGraphErrorKind::Resolve(_) => None,
    }
  }

  pub fn into_deno_graph_error(self) -> deno_graph::source::ResolveError {
    use deno_graph::source::ResolveError;

    match self.into_kind() {
      ResolveWithGraphErrorKind::Resolve(ende_resolve_error) => {
        ende_resolve_error.into_deno_graph_error()
      }
      err => ResolveError::from_err(err),
    }
  }
}

#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[allow(
  clippy::large_enum_variant,
  reason = "always boxed in ResolveWithGraphError"
)]
pub enum ResolveWithGraphErrorKind {
  #[error(transparent)]
  #[class(inherit)]
  Resolution(#[from] deno_graph::ResolutionError),
  #[error(transparent)]
  #[class(inherit)]
  Resolve(#[from] EndeResolveError),
}

pub struct MappedResolutionDiagnosticWithPosition<'a> {
  pub diagnostic: &'a MappedResolutionDiagnostic,
  pub referrer: &'a Url,
  pub start: deno_graph::Position,
}

#[allow(clippy::disallowed_types, reason = "definition")]
pub type OnMappedResolutionDiagnosticFn = ende_maybe_sync::MaybeArc<
  dyn Fn(MappedResolutionDiagnosticWithPosition) + Send + Sync,
>;

#[allow(clippy::disallowed_types, reason = "definition")]
pub type EndeResolverRc<TSys> = ende_maybe_sync::MaybeArc<EndeResolver<TSys>>;

/// The resolver used in the CLI for resolving and interfacing
/// with deno_graph.
pub struct EndeResolver<TSys: EndeResolverSys> {
  resolver: RawEndeResolverRc<TSys>,
  warned_pkgs: ende_maybe_sync::MaybeDashSet<PackageReq>,
  on_warning: Option<OnMappedResolutionDiagnosticFn>,
}

impl<TSys: EndeResolverSys> std::fmt::Debug for EndeResolver<TSys> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("EndeResolver").finish()
  }
}

impl<TSys: EndeResolverSys> EndeResolver<TSys> {
  pub fn new(
    resolver: RawEndeResolverRc<TSys>,
    on_warning: Option<OnMappedResolutionDiagnosticFn>,
  ) -> Self {
    Self {
      resolver,
      warned_pkgs: Default::default(),
      on_warning,
    }
  }

  pub fn resolve_with_graph(
    &self,
    graph: &deno_graph::ModuleGraph,
    raw_specifier: &str,
    referrer: &Url,
    referrer_range_start: deno_graph::Position,
    kind: ResolutionKind,
  ) -> Result<Url, ResolveWithGraphError> {
    let resolution = match graph.get(referrer) {
      Some(Module::Js(module)) => module
        .dependencies
        .get(raw_specifier)
        .map(|d| &d.maybe_code)
        .unwrap_or(&Resolution::None),
      _ => &Resolution::None,
    };

    let specifier = match resolution {
      Resolution::Ok(resolved) => Cow::Borrowed(&resolved.specifier),
      Resolution::Err(err) => {
        return Err(
          ResolveWithGraphErrorKind::Resolution((**err).clone()).into(),
        );
      }
      Resolution::None => Cow::Owned(self.resolve(
        raw_specifier,
        referrer,
        referrer_range_start,
        kind,
      )?),
    };

    let specifier = match graph.get(&specifier) {
      Some(module) => module.specifier().clone(),
      None => specifier.into_owned(),
    };
    Ok(specifier)
  }

  pub fn resolve(
    &self,
    raw_specifier: &str,
    referrer: &Url,
    referrer_range_start: deno_graph::Position,
    resolution_kind: ResolutionKind,
  ) -> Result<Url, EndeResolveError> {
    let resolution =
      self
        .resolver
        .resolve(raw_specifier, referrer, resolution_kind)?;

    if let Some(diagnostic) = resolution.maybe_diagnostic {
      let diagnostic = &*diagnostic;
      match diagnostic {
        MappedResolutionDiagnostic::ConstraintNotMatchedLocalVersion {
          reference,
          ..
        } => {
          if let Some(on_warning) = &self.on_warning
            && self.warned_pkgs.insert(reference.req().clone())
          {
            on_warning(MappedResolutionDiagnosticWithPosition {
              diagnostic,
              referrer,
              start: referrer_range_start,
            });
          }
        }
      }
    }

    Ok(resolution.url)
  }

  pub fn as_graph_resolver<'a>(
    &'a self,
    jsx_import_source_config_resolver: &'a JsxImportSourceConfigResolver,
    maybe_graph: Option<&'a deno_graph::ModuleGraph>,
  ) -> DenoGraphResolverAdapter<'a, TSys> {
    DenoGraphResolverAdapter {
      resolver: self,
      jsx_import_source_config_resolver,
      maybe_graph,
    }
  }
}

pub struct DenoGraphResolverAdapter<'a, TSys: EndeResolverSys> {
  resolver: &'a EndeResolver<TSys>,
  jsx_import_source_config_resolver: &'a JsxImportSourceConfigResolver,
  maybe_graph: Option<&'a deno_graph::ModuleGraph>,
}

impl<TSys: EndeResolverSys> std::fmt::Debug
  for DenoGraphResolverAdapter<'_, TSys>
{
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("DenoGraphResolverAdapter").finish()
  }
}

impl<TSys: EndeResolverSys> deno_graph::source::Resolver
  for DenoGraphResolverAdapter<'_, TSys>
{
  fn default_jsx_import_source(&self, referrer: &Url) -> Option<String> {
    self
      .jsx_import_source_config_resolver
      .for_specifier(referrer)
      .and_then(|c| c.import_source.as_ref().map(|s| s.specifier.clone()))
  }

  fn default_jsx_import_source_types(&self, referrer: &Url) -> Option<String> {
    self
      .jsx_import_source_config_resolver
      .for_specifier(referrer)
      .and_then(|c| c.import_source_types.as_ref().map(|s| s.specifier.clone()))
  }

  fn jsx_import_source_module(&self, referrer: &Url) -> &str {
    self
      .jsx_import_source_config_resolver
      .for_specifier(referrer)
      .map(|c| c.module.as_str())
      .unwrap_or(deno_graph::source::DEFAULT_JSX_IMPORT_SOURCE_MODULE)
  }

  fn resolve(
    &self,
    raw_specifier: &str,
    referrer_range: &deno_graph::Range,
    resolution_kind: deno_graph::source::ResolutionKind,
  ) -> Result<Url, ResolveError> {
    let kind = ResolutionKind::from_deno_graph(resolution_kind);
    match self.maybe_graph {
      Some(graph) => self
        .resolver
        .resolve_with_graph(
          graph,
          raw_specifier,
          &referrer_range.specifier,
          referrer_range.range.start,
          kind,
        )
        .map_err(|err| err.into_deno_graph_error()),
      None => self
        .resolver
        .resolve(
          raw_specifier,
          &referrer_range.specifier,
          referrer_range.range.start,
          kind,
        )
        .map_err(|err| err.into_deno_graph_error()),
    }
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnhanceGraphErrorMode {
  ShowRange,
  HideRange,
}

#[derive(Debug, deno_error::JsError)]
#[class(inherit)]
pub struct EnhancedGraphError {
  #[inherit]
  original: ModuleGraphError,
  message: String,
  mode: EnhanceGraphErrorMode,
}

impl EnhancedGraphError {
  pub fn original(&self) -> &ModuleGraphError {
    &self.original
  }
}

impl std::fmt::Display for EnhancedGraphError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(&self.message)?;
    if let Some(range) = self.original.maybe_range()
      && self.mode == EnhanceGraphErrorMode::ShowRange
      && !range.specifier.as_str().contains("/$ende$eval")
    {
      write!(f, "\n    at {}", format_range_with_colors(range))?;
    }
    Ok(())
  }
}

impl std::error::Error for EnhancedGraphError {}

pub fn enhance_graph_error(
  sys: &(impl sys_traits::FsMetadata + Clone),
  error: ModuleGraphError,
  mode: EnhanceGraphErrorMode,
  // Names of packages importable by bare specifier (workspace members and
  // packages linked via the "links" field). Used to produce a better hint
  // when a bare import almost matches one of them.
  bare_importable_pkg_names: &[String],
) -> EnhancedGraphError {
  let mut message = match &error {
    ModuleGraphError::ResolutionError(resolution_error) => {
      enhanced_resolution_error_message(
        resolution_error,
        bare_importable_pkg_names,
      )
    }
    ModuleGraphError::TypesResolutionError(resolution_error) => {
      format!(
        "Failed resolving types. {}",
        enhanced_resolution_error_message(
          resolution_error,
          bare_importable_pkg_names,
        )
      )
    }
    ModuleGraphError::ModuleError(error) => {
      enhanced_integrity_error_message(error)
        .or_else(|| enhanced_sloppy_imports_error_message(sys, error))
        .or_else(|| enhanced_unsupported_import_attribute(error))
        .unwrap_or_else(|| format_deno_graph_error(error))
    }
  };

  if let Some(docs_url) = maybe_docs_url_for_graph_error(&error) {
    message.push_str(&format!(
      "\n  {} {}",
      deno_terminal::colors::cyan("docs:"),
      docs_url,
    ));
  }

  EnhancedGraphError {
    original: error,
    message,
    mode,
  }
}

/// Returns a link to relevant Ende documentation for known module graph
/// errors so users can quickly find the recommended fix.
fn maybe_docs_url_for_graph_error(
  error: &ModuleGraphError,
) -> Option<&'static str> {
  let ModuleGraphError::ModuleError(error) = error else {
    return None;
  };
  match error.as_kind() {
    ModuleErrorKind::UnsupportedMediaType {
      media_type: MediaType::Json,
      ..
    }
    | ModuleErrorKind::InvalidTypeAssertion {
      actual_media_type: MediaType::Json,
      ..
    } => Some("https://docs.ende.sh/examples/importing_json/"),
    _ => None,
  }
}

/// Adds more explanatory information to a resolution error.
pub fn enhanced_resolution_error_message(
  error: &ResolutionError,
  bare_importable_pkg_names: &[String],
) -> String {
  let mut message = format_deno_graph_error(error);

  let maybe_hint =
    get_import_prefix_missing_error(error).and_then(|specifier| {
      // If the specifier looks like a workspace member or linked package whose
      // declared name is scoped (e.g. importing "foo" when "@scope/foo" is
      // linked), point at the real name instead of suggesting `ende add`.
      if let Some(name) =
        maybe_matching_bare_pkg_name(specifier, bare_importable_pkg_names)
      {
        Some(format!(
          "\"{name}\" is available in this workspace (via a workspace member or the \"links\" field). Import it by its full name.",
        ))
      } else if specifier.starts_with('@') {
        Some(format!(
          "If you want to use the JSR package, try running `ende add jsr:{}`",
          specifier
        ))
      } else {
        None
      }
    });

  if let Some(hint) = maybe_hint {
    message.push_str(&format!(
      "\n  {} {}",
      deno_terminal::colors::cyan("hint:"),
      hint
    ));
  }

  message
}

static RUN_WITH_SLOPPY_IMPORTS_MSG: &str = "or run with --sloppy-imports";

fn enhanced_sloppy_imports_error_message(
  sys: &(impl sys_traits::FsMetadata + Clone),
  error: &ModuleError,
) -> Option<String> {
  match error.as_kind() {
    ModuleErrorKind::Load { specifier, err: ModuleLoadError::Loader(_), .. } // ex. "Is a directory" error
    | ModuleErrorKind::Missing { specifier, .. } => {
      let additional_message = maybe_additional_sloppy_imports_message(sys, specifier)?;
      Some(format!(
        "{} {}",
        error,
        additional_message,
      ))
    }
    _ => None,
  }
}

pub fn maybe_additional_sloppy_imports_message(
  sys: &(impl sys_traits::FsMetadata + Clone),
  specifier: &Url,
) -> Option<String> {
  let (resolved, sloppy_reason) = sloppy_imports_resolve(
    specifier,
    crate::workspace::ResolutionKind::Execution,
    sys.clone(),
  )?;
  Some(format!(
    "{} {}",
    sloppy_reason.suggestion_message_for_specifier(&resolved),
    RUN_WITH_SLOPPY_IMPORTS_MSG
  ))
}

pub fn enhanced_integrity_error_message(err: &ModuleError) -> Option<String> {
  match err.as_kind() {
    ModuleErrorKind::Load {
      specifier,
      err:
        ModuleLoadError::Jsr(JsrLoadError::ContentChecksumIntegrity(checksum_err)),
      ..
    } => Some(format!(
      concat!(
        "Integrity check failed in package. The package may have been tampered with.\n\n",
        "  Specifier: {}\n",
        "  Actual: {}\n",
        "  Expected: {}\n\n",
        "If you modified your global cache, run again with the --reload flag to restore ",
        "its state. If you want to modify dependencies locally run again with the ",
        "--vendor flag or specify `\"vendor\": true` in an ende.json then modify the contents ",
        "of the vendor/ folder."
      ),
      specifier, checksum_err.actual, checksum_err.expected,
    )),
    ModuleErrorKind::Load {
      err:
        ModuleLoadError::Jsr(
          JsrLoadError::PackageVersionManifestChecksumIntegrity(
            package_nv,
            checksum_err,
          ),
        ),
      ..
    } => Some(format!(
      concat!(
        "Integrity check failed for package. The source code is invalid, as it does not match the expected hash in the lock file.\n\n",
        "  Package: {}\n",
        "  Actual: {}\n",
        "  Expected: {}\n\n",
        "This could be caused by:\n",
        "  * the lock file may be corrupt\n",
        "  * the source itself may be corrupt\n\n",
        "Investigate the lockfile; delete it to regenerate the lockfile or --reload to reload the source code from the server."
      ),
      package_nv, checksum_err.actual, checksum_err.expected,
    )),
    ModuleErrorKind::Load {
      specifier,
      err: ModuleLoadError::HttpsChecksumIntegrity(checksum_err),
      ..
    } => Some(format!(
      concat!(
        "Integrity check failed for remote specifier. The source code is invalid, as it does not match the expected hash in the lock file.\n\n",
        "  Specifier: {}\n",
        "  Actual: {}\n",
        "  Expected: {}\n\n",
        "This could be caused by:\n",
        "  * the lock file may be corrupt\n",
        "  * the source itself may be corrupt\n\n",
        "Investigate the lockfile; delete it to regenerate the lockfile or --reload to reload the source code from the server."
      ),
      specifier, checksum_err.actual, checksum_err.expected,
    )),
    _ => None,
  }
}

fn enhanced_unsupported_import_attribute(err: &ModuleError) -> Option<String> {
  match err.as_kind() {
    ModuleErrorKind::UnsupportedImportAttributeType { kind, .. }
      if kind == "bytes" || kind == "css" =>
    {
      let mut text = format_deno_graph_error(err);
      text.push_str(&format!(
        "\n  {} run with --unstable-raw-imports",
        deno_terminal::colors::cyan("hint:")
      ));
      Some(text)
    }
    _ => None,
  }
}

/// If `specifier` (a bare import that failed to resolve) matches the unscoped
/// name of a known bare-importable package, returns that package's full name.
///
/// This catches the common mistake of importing a scoped package by its
/// unscoped tail, e.g. importing `"my-pkg"` when `"@scope/my-pkg"` is linked.
///
/// If two packages share an unscoped tail (e.g. `@a/foo` and `@b/foo`), the
/// first match wins; that's acceptable since this only produces a hint.
fn maybe_matching_bare_pkg_name<'a>(
  specifier: &str,
  bare_importable_pkg_names: &'a [String],
) -> Option<&'a str> {
  // Compare only the package portion, ignoring any imported subpath.
  let specifier_head = specifier.split('/').next().unwrap_or(specifier);
  bare_importable_pkg_names
    .iter()
    .map(|name| name.as_str())
    .find(|name| {
      // The unscoped tail of `@scope/foo` is `foo`; an unscoped name is its
      // own tail.
      let unscoped = name
        .strip_prefix('@')
        .and_then(|rest| rest.split_once('/'))
        .map(|(_scope, tail)| tail)
        .unwrap_or(name);
      // Only suggest when the user didn't already type the full name (that
      // would have resolved), but the unscoped tail matches.
      *name != specifier_head && unscoped == specifier_head
    })
}

fn get_import_prefix_missing_error(error: &ResolutionError) -> Option<&str> {
  // not exact, but ok because this is just a hint
  let media_type =
    MediaType::from_specifier_and_headers(&error.range().specifier, None);
  if media_type == MediaType::Wasm {
    return None;
  }

  let mut maybe_specifier = None;
  if let ResolutionError::InvalidSpecifier {
    error: SpecifierError::ImportPrefixMissing { specifier, .. },
    range,
  } = error
  {
    if range.specifier.scheme() == "file" {
      maybe_specifier = Some(specifier);
    }
  } else if let ResolutionError::ResolverError { error, range, .. } = error
    && range.specifier.scheme() == "file"
  {
    match error.as_ref() {
      ResolveError::Specifier(specifier_error) => {
        if let SpecifierError::ImportPrefixMissing { specifier, .. } =
          specifier_error
        {
          maybe_specifier = Some(specifier);
        }
      }
      ResolveError::Other(other_error) => {
        if let Some(SpecifierError::ImportPrefixMissing { specifier, .. }) =
          other_error.get_ref().downcast_ref::<SpecifierError>()
        {
          maybe_specifier = Some(specifier);
        } else if let Some(err) = other_error
          .get_ref()
          .downcast_ref::<import_map::ImportMapError>()
          && let ImportMapErrorKind::UnmappedBareSpecifier(specifier, _referrer) =
            err.as_kind()
        {
          maybe_specifier = Some(specifier);
        }
      }
    }
  }

  if let Some(specifier) = maybe_specifier {
    // NOTE(bartlomieju): For now, return None if a specifier contains a dot or a space. This is because
    // suggesting to `ende add bad-module.ts` makes no sense and is worse than not providing
    // a suggestion at all. This should be improved further in the future
    if specifier.contains('.') || specifier.contains(' ') {
      return None;
    }
    // Do not return a hint for specifiers starting with `@`, but not containing a `/`
    if specifier.starts_with('@') && !specifier.contains('/') {
      return None;
    }
  }

  maybe_specifier.map(|s| s.as_str())
}

pub fn format_range_with_colors(referrer: &deno_graph::Range) -> String {
  use deno_terminal::colors;
  format!(
    "{}:{}:{}",
    colors::cyan(referrer.specifier.as_str()),
    colors::yellow(&(referrer.range.start.line + 1).to_string()),
    colors::yellow(&(referrer.range.start.character + 1).to_string())
  )
}

pub fn format_deno_graph_error(err: &dyn std::error::Error) -> String {
  use std::fmt::Write;

  let mut message = format!("{}", err);
  let mut maybe_source = err.source();

  if maybe_source.is_some() {
    let mut past_message = message.clone();
    let mut count = 0;
    let mut display_count = 0;
    while let Some(source) = maybe_source {
      let current_message = format!("{}", source);
      maybe_source = source.source();

      // sometimes an error might be repeated due to
      // being boxed multiple times in another AnyError
      if current_message != past_message {
        write!(message, "\n    {}: ", display_count,).unwrap();
        for (i, line) in current_message.lines().enumerate() {
          if i > 0 {
            write!(message, "\n       {}", line).unwrap();
          } else {
            write!(message, "{}", line).unwrap();
          }
        }
        display_count += 1;
      }

      if count > 8 {
        write!(message, "\n    {}: ...", count).unwrap();
        break;
      }

      past_message = current_message;
      count += 1;
    }
  }

  message
}

#[cfg(test)]
mod test {
  use super::*;

  #[test]
  fn matching_bare_pkg_name() {
    let names = [
      "@scope/my-pkg".to_string(),
      "plain-pkg".to_string(),
      "@org/utils".to_string(),
    ];
    // Unscoped tail of a scoped package matches.
    assert_eq!(
      maybe_matching_bare_pkg_name("my-pkg", &names),
      Some("@scope/my-pkg")
    );
    // Importing a subpath still matches on the package head.
    assert_eq!(
      maybe_matching_bare_pkg_name("utils/helpers", &names),
      Some("@org/utils")
    );
    // The full scoped name resolves on its own, so no suggestion.
    assert_eq!(maybe_matching_bare_pkg_name("@scope/my-pkg", &names), None);
    // An unscoped name that already matches exactly would have resolved.
    assert_eq!(maybe_matching_bare_pkg_name("plain-pkg", &names), None);
    // No relation at all.
    assert_eq!(maybe_matching_bare_pkg_name("unrelated", &names), None);
    // Nothing linked.
    assert_eq!(maybe_matching_bare_pkg_name("my-pkg", &[]), None);
  }
}
