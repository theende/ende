// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::sync::Arc;

use deno_error::JsErrorBox;
use deno_media_type::MediaType;
use ende_core::url::Url;
use ende_resolver::loader::NpmModuleLoader;
use ende_resolver::npm::DenoInNpmPackageChecker;
use ende_resolver::npm::NpmReqResolver;
use node_resolver::DenoIsBuiltInNodeModuleChecker;
use node_resolver::analyze::CjsAnalysis;
use node_resolver::analyze::CjsAnalysisExports;
use node_resolver::analyze::EsmAnalysisMode;
use node_resolver::analyze::NodeCodeTranslator;

use crate::file_system::EndeRtSys;

pub type EndeRtCjsTracker =
  ende_resolver::cjs::CjsTracker<DenoInNpmPackageChecker, EndeRtSys>;
pub type EndeRtNpmResolver = ende_resolver::npm::NpmResolver<EndeRtSys>;
pub type EndeRtNpmModuleLoader = NpmModuleLoader<
  CjsCodeAnalyzer,
  DenoInNpmPackageChecker,
  DenoIsBuiltInNodeModuleChecker,
  EndeRtNpmResolver,
  EndeRtSys,
>;
pub type EndeRtNodeCodeTranslator = NodeCodeTranslator<
  CjsCodeAnalyzer,
  DenoInNpmPackageChecker,
  DenoIsBuiltInNodeModuleChecker,
  EndeRtNpmResolver,
  EndeRtSys,
>;
pub type EndeRtNodeResolver = ende_runtime::ende_node::NodeResolver<
  DenoInNpmPackageChecker,
  EndeRtNpmResolver,
  EndeRtSys,
>;
pub type EndeRtNpmReqResolver = NpmReqResolver<
  DenoInNpmPackageChecker,
  DenoIsBuiltInNodeModuleChecker,
  EndeRtNpmResolver,
  EndeRtSys,
>;

pub struct CjsCodeAnalyzer {
  cjs_tracker: Arc<EndeRtCjsTracker>,
}

impl CjsCodeAnalyzer {
  pub fn new(cjs_tracker: Arc<EndeRtCjsTracker>) -> Self {
    Self { cjs_tracker }
  }

  fn inner_cjs_analysis<'a>(
    &self,
    specifier: &Url,
    source: Cow<'a, str>,
  ) -> Result<CjsAnalysis<'a>, JsErrorBox> {
    let media_type = MediaType::from_specifier(specifier);
    if media_type == MediaType::Json {
      return Ok(CjsAnalysis::Cjs(CjsAnalysisExports {
        exports: vec![],
        reexports: vec![],
        member_reexports: vec![],
      }));
    }

    let cjs_tracker = &self.cjs_tracker;
    let is_maybe_cjs = cjs_tracker
      .is_maybe_cjs(specifier, media_type)
      .map_err(JsErrorBox::from_err)?;
    let analysis = if is_maybe_cjs {
      // `ende compile` doesn't embed a CJS export analysis, so analyze the
      // source when it's loaded.
      analyze_cjs_live(cjs_tracker, specifier, media_type, source)?
    } else {
      CjsAnalysis::Esm(source, None)
    };

    Ok(analysis)
  }
}

/// Parse `source` with deno_ast and classify it as CJS (with its named
/// exports) or ESM. Used at runtime in the standalone executable because
/// `ende compile` doesn't embed a pre-computed analysis.
///
/// Mirrors what `EndeCjsCodeAnalyzer` does at compile time, minus the
/// re-export resolution (the loader walks reexports separately). The
/// `compute_is_script` + `is_cjs_with_known_is_script` two-step is the
/// same one the compile-time analyzer uses; without it, ESM files that
/// happen to look CJS-ish would be mis-classified.
fn analyze_cjs_live<'a>(
  cjs_tracker: &EndeRtCjsTracker,
  specifier: &Url,
  media_type: MediaType,
  source: Cow<'a, str>,
) -> Result<CjsAnalysis<'a>, JsErrorBox> {
  let parsed = deno_ast::parse_program(deno_ast::ParseParams {
    specifier: specifier.clone(),
    text: source.to_string().into(),
    media_type,
    capture_tokens: true,
    scope_analysis: false,
    maybe_syntax: None,
  })
  .map_err(JsErrorBox::from_err)?;
  let is_script = parsed.compute_is_script();
  let is_cjs = cjs_tracker
    .is_cjs_with_known_is_script(specifier, media_type, is_script)
    .map_err(JsErrorBox::from_err)?;
  if is_cjs {
    let analysis = parsed.analyze_cjs();
    cjs_tracker.set_is_known_script(specifier, true);
    Ok(CjsAnalysis::Cjs(CjsAnalysisExports {
      exports: analysis.exports,
      reexports: analysis.reexports,
      // `deno_ast::analyze_cjs` doesn't surface member-level
      // reexports; the loader walks them via its own reexport
      // resolution.
      member_reexports: Vec::new(),
    }))
  } else {
    cjs_tracker.set_is_known_script(specifier, false);
    Ok(CjsAnalysis::Esm(source, None))
  }
}

#[async_trait::async_trait(?Send)]
impl node_resolver::analyze::CjsCodeAnalyzer for CjsCodeAnalyzer {
  async fn analyze_cjs<'a>(
    &self,
    specifier: &Url,
    source: Option<Cow<'a, str>>,
    _esm_analysis_mode: EsmAnalysisMode,
  ) -> Result<CjsAnalysis<'a>, JsErrorBox> {
    let source = match source {
      Some(source) => source,
      // Recursive analysis must use the source selected by the active module
      // loader. If no loader-owned source is available, leave the export set
      // empty and let runtime resolution handle the target.
      None => {
        return Ok(CjsAnalysis::Cjs(CjsAnalysisExports {
          exports: vec![],
          reexports: vec![],
          member_reexports: vec![],
        }));
      }
    };
    self.inner_cjs_analysis(specifier, source)
  }

  async fn analyze_cjs_member_props<'a>(
    &self,
    _specifier: &Url,
    _maybe_source: Option<Cow<'a, str>>,
    _member: &str,
  ) -> Result<Option<Vec<String>>, JsErrorBox> {
    // The compiled runtime does not have access to swc; CJS export
    // analysis is precomputed at compile time. Member-shape narrowing
    // is therefore unavailable here, and the wrapper falls back to
    // advertising only the default and module.exports.
    Ok(None)
  }
}
