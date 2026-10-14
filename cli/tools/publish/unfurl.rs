// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::sync::Arc;

use deno_ast::ParsedSource;
use deno_ast::SourcePos;
use deno_ast::SourceRange;
use deno_ast::SourceTextInfo;
use deno_ast::SourceTextProvider;
use deno_ast::TextChange;
use deno_ast::diagnostics::Diagnostic;
use deno_ast::diagnostics::DiagnosticLevel;
use deno_ast::diagnostics::DiagnosticLocation;
use deno_ast::diagnostics::DiagnosticSnippet;
use deno_ast::diagnostics::DiagnosticSnippetHighlight;
use deno_ast::diagnostics::DiagnosticSnippetHighlightStyle;
use deno_ast::diagnostics::DiagnosticSourcePos;
use deno_ast::diagnostics::DiagnosticSourceRange;
use deno_ast::swc::ecma_visit::VisitWith;
use deno_graph::analysis::DependencyDescriptor;
use deno_graph::analysis::DynamicTemplatePart;
use deno_graph::analysis::StaticDependencyKind;
use deno_graph::analysis::TypeScriptReference;
use ende_core::ModuleSpecifier;
use ende_resolver::workspace::MappedResolution;
use ende_resolver::workspace::WorkspaceResolver;

use crate::sys::CliSys;
use crate::tools::unfurl_utils::ImportMetaResolveCollector;
use crate::tools::unfurl_utils::SpecifierTextContext;
use crate::tools::unfurl_utils::dynamic_argument_prefix_range;
use crate::tools::unfurl_utils::specifier_text_change;
use crate::tools::unfurl_utils::to_range;

#[derive(Debug, Clone)]
pub enum SpecifierUnfurlerDiagnostic {
  UnanalyzableDynamicImport {
    specifier: ModuleSpecifier,
    text_info: SourceTextInfo,
    range: SourceRange,
  },
  UnanalyzableImportMetaResolve {
    specifier: ModuleSpecifier,
    text_info: SourceTextInfo,
    range: SourceRange,
  },
  UnsupportedNpmSpecifier {
    specifier: ModuleSpecifier,
    npm_specifier: String,
    /// The import's position, or `None` when the import comes from the
    /// configuration (ex. `compilerOptions.jsxImportSource`).
    position: Option<(SourceTextInfo, SourceRange)>,
  },
}

impl Diagnostic for SpecifierUnfurlerDiagnostic {
  fn level(&self) -> DiagnosticLevel {
    match self {
      SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport { .. } => {
        DiagnosticLevel::Warning
      }
      SpecifierUnfurlerDiagnostic::UnanalyzableImportMetaResolve { .. } => {
        DiagnosticLevel::Warning
      }
      SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier { .. } => {
        DiagnosticLevel::Error
      }
    }
  }

  fn code(&self) -> Cow<'_, str> {
    match self {
      Self::UnanalyzableDynamicImport { .. } => "unanalyzable-dynamic-import",
      Self::UnanalyzableImportMetaResolve { .. } => {
        "unanalyzable-import-meta-resolve"
      }
      Self::UnsupportedNpmSpecifier { .. } => "unsupported-npm-specifier",
    }
    .into()
  }

  fn message(&self) -> Cow<'_, str> {
    match self {
      Self::UnanalyzableDynamicImport { .. } => {
        "unable to analyze dynamic import".into()
      }
      Self::UnanalyzableImportMetaResolve { .. } => {
        "unable to analyze import.meta.resolve".into()
      }
      Self::UnsupportedNpmSpecifier { npm_specifier, .. } => format!(
        "npm packages are not supported in Ende ('{}')",
        npm_specifier
      )
      .into(),
    }
  }

  fn location(&self) -> deno_ast::diagnostics::DiagnosticLocation<'_> {
    match self {
      SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport {
        specifier,
        text_info,
        range,
      } => DiagnosticLocation::ModulePosition {
        specifier: Cow::Borrowed(specifier),
        text_info: Cow::Borrowed(text_info),
        source_pos: DiagnosticSourcePos::SourcePos(range.start),
      },
      SpecifierUnfurlerDiagnostic::UnanalyzableImportMetaResolve {
        specifier,
        text_info,
        range,
      } => DiagnosticLocation::ModulePosition {
        specifier: Cow::Borrowed(specifier),
        text_info: Cow::Borrowed(text_info),
        source_pos: DiagnosticSourcePos::SourcePos(range.start),
      },
      SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier {
        specifier,
        position,
        ..
      } => match position {
        Some((text_info, range)) => DiagnosticLocation::ModulePosition {
          specifier: Cow::Borrowed(specifier),
          text_info: Cow::Borrowed(text_info),
          source_pos: DiagnosticSourcePos::SourcePos(range.start),
        },
        None => DiagnosticLocation::Module {
          specifier: Cow::Borrowed(specifier),
        },
      },
    }
  }

  fn snippet(&self) -> Option<deno_ast::diagnostics::DiagnosticSnippet<'_>> {
    match self {
      SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport {
        text_info,
        range,
        ..
      } => Some(DiagnosticSnippet {
        source: Cow::Borrowed(text_info),
        highlights: vec![DiagnosticSnippetHighlight {
          style: DiagnosticSnippetHighlightStyle::Warning,
          range: DiagnosticSourceRange {
            start: DiagnosticSourcePos::SourcePos(range.start),
            end: DiagnosticSourcePos::SourcePos(range.end),
          },
          description: Some("the unanalyzable dynamic import".into()),
        }],
      }),
      SpecifierUnfurlerDiagnostic::UnanalyzableImportMetaResolve {
        text_info,
        range,
        ..
      } => Some(DiagnosticSnippet {
        source: Cow::Borrowed(text_info),
        highlights: vec![DiagnosticSnippetHighlight {
          style: DiagnosticSnippetHighlightStyle::Warning,
          range: DiagnosticSourceRange {
            start: DiagnosticSourcePos::SourcePos(range.start),
            end: DiagnosticSourcePos::SourcePos(range.end),
          },
          description: Some("the unanalyzable import.meta.resolve call".into()),
        }],
      }),
      SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier {
        position: Some((text_info, range)),
        ..
      } => Some(DiagnosticSnippet {
        source: Cow::Borrowed(text_info),
        highlights: vec![DiagnosticSnippetHighlight {
          style: DiagnosticSnippetHighlightStyle::Error,
          range: DiagnosticSourceRange {
            start: DiagnosticSourcePos::SourcePos(range.start),
            end: DiagnosticSourcePos::SourcePos(range.end),
          },
          description: Some("the npm import".into()),
        }],
      }),
      SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier {
        position: None,
        ..
      } => None,
    }
  }

  fn hint(&self) -> Option<Cow<'_, str>> {
    match self {
      SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport { .. }
      | SpecifierUnfurlerDiagnostic::UnanalyzableImportMetaResolve { .. } => {
        None
      }
      SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier { .. } => {
        Some("use a JSR package instead".into())
      }
    }
  }

  fn snippet_fixed(
    &self,
  ) -> Option<deno_ast::diagnostics::DiagnosticSnippet<'_>> {
    None
  }

  fn info(&self) -> Cow<'_, [Cow<'_, str>]> {
    match self {
      SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport { .. } => {
        Cow::Borrowed(&[
          Cow::Borrowed(
            "after publishing this package, imports from the local import map do not work",
          ),
          Cow::Borrowed(
            "dynamic imports that can not be analyzed at publish time will not be rewritten automatically",
          ),
          Cow::Borrowed(
            "make sure the dynamic import is resolvable at runtime without an import map",
          ),
        ])
      }
      SpecifierUnfurlerDiagnostic::UnanalyzableImportMetaResolve { .. } => {
        Cow::Borrowed(&[
          Cow::Borrowed(
            "after publishing this package, import.meta.resolve calls from the local import map do not work",
          ),
          Cow::Borrowed(
            "import.meta.resolve calls that can not be analyzed at publish time will not be rewritten automatically",
          ),
          Cow::Borrowed(
            "make sure the import.meta.resolve call is resolvable at runtime without an import map",
          ),
        ])
      }
      SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier { .. } => {
        Cow::Borrowed(&[])
      }
    }
  }

  fn docs_url(&self) -> Option<Cow<'_, str>> {
    None
  }
}

enum UnfurlSpecifierError {
  UnsupportedNpmSpecifier { npm_specifier: String },
}

#[derive(Copy, Clone)]
pub enum PositionOrSourceRangeRef<'a> {
  PositionRange(&'a deno_graph::PositionRange),
  SourceRange(SourceRange<SourcePos>),
}

#[sys_traits::auto_impl]
pub trait SpecifierUnfurlerSys: ende_resolver::EndeResolverSys {}

pub struct SpecifierUnfurler<TSys: SpecifierUnfurlerSys = CliSys> {
  workspace_resolver: Arc<WorkspaceResolver<TSys>>,
}

impl<TSys: SpecifierUnfurlerSys> SpecifierUnfurler<TSys> {
  pub fn new(workspace_resolver: Arc<WorkspaceResolver<TSys>>) -> Self {
    Self { workspace_resolver }
  }

  pub fn unfurl_specifier_reporting_diagnostic(
    &self,
    referrer: &ModuleSpecifier,
    specifier: &str,
    resolution_kind: ende_resolver::workspace::ResolutionKind,
    text_info: &SourceTextInfo,
    range: PositionOrSourceRangeRef<'_>,
    diagnostic_reporter: &mut dyn FnMut(SpecifierUnfurlerDiagnostic),
  ) -> Option<String> {
    match self.unfurl_specifier(referrer, specifier, resolution_kind) {
      Ok(maybe_unfurled) => maybe_unfurled,
      Err(diagnostic) => {
        let range = match range {
          PositionOrSourceRangeRef::PositionRange(position_range) => {
            let range = to_range(text_info, position_range);
            SourceRange::new(
              text_info.start_pos() + range.start,
              text_info.start_pos() + range.end,
            )
          }
          PositionOrSourceRangeRef::SourceRange(source_range) => source_range,
        };
        match diagnostic {
          UnfurlSpecifierError::UnsupportedNpmSpecifier { npm_specifier } => {
            diagnostic_reporter(
              SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier {
                specifier: referrer.clone(),
                npm_specifier,
                position: Some((text_info.clone(), range)),
              },
            );
            None
          }
        }
      }
    }
  }

  /// Unfurls a `jsxImportSource` from the configuration, reporting npm
  /// packages on the module that uses it.
  pub fn unfurl_jsx_import_source(
    &self,
    module_specifier: &ModuleSpecifier,
    import_source: &str,
    base: &ModuleSpecifier,
    resolution_kind: ende_resolver::workspace::ResolutionKind,
    diagnostic_reporter: &mut dyn FnMut(SpecifierUnfurlerDiagnostic),
  ) -> Option<String> {
    match self.unfurl_specifier(base, import_source, resolution_kind) {
      Ok(maybe_unfurled) => maybe_unfurled,
      Err(UnfurlSpecifierError::UnsupportedNpmSpecifier { npm_specifier }) => {
        diagnostic_reporter(
          SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier {
            specifier: module_specifier.clone(),
            npm_specifier,
            position: None,
          },
        );
        None
      }
    }
  }

  fn unfurl_specifier(
    &self,
    referrer: &ModuleSpecifier,
    specifier: &str,
    resolution_kind: ende_resolver::workspace::ResolutionKind,
  ) -> Result<Option<String>, UnfurlSpecifierError> {
    let resolved = match self.workspace_resolver.resolve(
      specifier,
      referrer,
      resolution_kind,
    ) {
      Ok(resolved) => match resolved {
        MappedResolution::Normal { specifier, .. } => Some(specifier),
        MappedResolution::WorkspaceJsrPackage { pkg_req_ref, .. } => {
          Some(ModuleSpecifier::parse(&pkg_req_ref.to_string()).unwrap())
        }
      },
      Err(_) => None,
    };
    let resolved = match resolved {
      Some(resolved) => resolved,
      None if ende_resolver::is_node_builtin_module_name(specifier) => {
        format!("node:{specifier}").parse().unwrap()
      }
      None => match ModuleSpecifier::options()
        .base_url(Some(referrer))
        .parse(specifier)
        .ok()
      {
        Some(value) => value,
        None => return Ok(None),
      },
    };
    if resolved.scheme() == "npm" {
      return Err(UnfurlSpecifierError::UnsupportedNpmSpecifier {
        npm_specifier: resolved.to_string(),
      });
    }
    // TODO(lucacasonato): this requires integration in deno_graph first
    // let resolved = if let Ok(specifier) =
    //   NpmPackageReqReference::from_specifier(&resolved)
    // {
    //   if let Some(scope_name) = specifier.req().name.strip_prefix("@jsr/") {
    //     let (scope, name) = scope_name.split_once("__")?;
    //     let new_specifier = JsrPackageReqReference::new(PackageReqReference {
    //       req: PackageReq {
    //         name: format!("@{scope}/{name}"),
    //         version_req: specifier.req().version_req.clone(),
    //       },
    //       sub_path: specifier.sub_path().map(ToOwned::to_owned),
    //     })
    //     .to_string();
    //     ModuleSpecifier::parse(&new_specifier).unwrap()
    //   } else {
    //     resolved
    //   }
    // } else {
    //   resolved
    // };
    let relative_resolved = relative_url(&resolved, referrer);
    if relative_resolved == specifier {
      Ok(None) // nothing to unfurl
    } else {
      log::debug!(
        "Unfurled specifier: {} from {} -> {}",
        specifier,
        referrer,
        relative_resolved
      );
      Ok(Some(relative_resolved))
    }
  }

  /// Attempts to unfurl the dynamic dependency returning `true` on success
  /// or `false` when the import was not analyzable.
  fn try_unfurl_dynamic_dep(
    &self,
    module_url: &ModuleSpecifier,
    text_info: &SourceTextInfo,
    dep: &deno_graph::analysis::DynamicDependencyDescriptor,
    text_changes: &mut Vec<deno_ast::TextChange>,
    diagnostic_reporter: &mut dyn FnMut(SpecifierUnfurlerDiagnostic),
  ) -> bool {
    match &dep.argument {
      deno_graph::analysis::DynamicArgument::String(specifier) => {
        let maybe_unfurled = self.unfurl_specifier_reporting_diagnostic(
          module_url,
          specifier,
          ende_resolver::workspace::ResolutionKind::Execution, // dynamic imports are always execution
          text_info,
          PositionOrSourceRangeRef::PositionRange(&dep.argument_range),
          diagnostic_reporter,
        );
        if let Some(unfurled) = maybe_unfurled {
          let Some(range) = dynamic_argument_prefix_range(
            text_info,
            &dep.argument_range,
            specifier,
          ) else {
            return false;
          };
          let Some(text_change) = specifier_text_change(
            text_info,
            range,
            &unfurled,
            SpecifierTextContext::JavaScript,
          ) else {
            return false;
          };
          text_changes.push(text_change);
        }
        true
      }
      deno_graph::analysis::DynamicArgument::Template(parts) => {
        match parts.first() {
          Some(DynamicTemplatePart::String { value: specifier }) => {
            // relative doesn't need to be modified
            if deno_path_util::is_relative_specifier(specifier) {
              return true;
            }
            if !specifier.ends_with('/') {
              return false;
            }
            let unfurled = self.unfurl_specifier_reporting_diagnostic(
              module_url,
              specifier,
              ende_resolver::workspace::ResolutionKind::Execution, // dynamic imports are always execution
              text_info,
              PositionOrSourceRangeRef::PositionRange(&dep.argument_range),
              diagnostic_reporter,
            );
            let Some(unfurled) = unfurled else {
              return true; // nothing to unfurl
            };
            let Some(range) = dynamic_argument_prefix_range(
              text_info,
              &dep.argument_range,
              specifier,
            ) else {
              return false;
            };
            let Some(text_change) = specifier_text_change(
              text_info,
              range,
              &unfurled,
              SpecifierTextContext::JavaScript,
            ) else {
              return false;
            };
            text_changes.push(text_change);
            true
          }
          Some(DynamicTemplatePart::Expr) => {
            false // failed analyzing
          }
          None => {
            true // ignore
          }
        }
      }
      deno_graph::analysis::DynamicArgument::Expr => {
        false // failed analyzing
      }
    }
  }

  pub fn unfurl_to_changes(
    &self,
    url: &ModuleSpecifier,
    parsed_source: &ParsedSource,
    module_info: &deno_graph::analysis::ModuleInfo,
    text_changes: &mut Vec<TextChange>,
    diagnostic_reporter: &mut dyn FnMut(SpecifierUnfurlerDiagnostic),
  ) {
    let text_info = parsed_source.text_info_lazy();
    let analyze_specifier =
      |specifier: &str,
       range: PositionOrSourceRangeRef,
       resolution_kind: ende_resolver::workspace::ResolutionKind,
       context: SpecifierTextContext,
       text_changes: &mut Vec<deno_ast::TextChange>,
       diagnostic_reporter: &mut dyn FnMut(SpecifierUnfurlerDiagnostic)| {
        if let Some(unfurled) = self.unfurl_specifier_reporting_diagnostic(
          url,
          specifier,
          resolution_kind,
          text_info,
          range,
          diagnostic_reporter,
        ) {
          let range = match range {
            PositionOrSourceRangeRef::PositionRange(position_range) => {
              to_range(text_info, position_range)
            }
            PositionOrSourceRangeRef::SourceRange(source_range) => {
              source_range.as_byte_range(parsed_source.start_pos())
            }
          };
          if let Some(text_change) =
            specifier_text_change(text_info, range, &unfurled, context)
          {
            text_changes.push(text_change);
          }
        }
      };
    for dep in &module_info.dependencies {
      match dep {
        DependencyDescriptor::Static(dep) => {
          let resolution_kind = if parsed_source.media_type().is_declaration() {
            ende_resolver::workspace::ResolutionKind::Types
          } else {
            match dep.kind {
              StaticDependencyKind::Export
              | StaticDependencyKind::Import
              | StaticDependencyKind::ImportDefer
              | StaticDependencyKind::ImportSource
              | StaticDependencyKind::ExportEquals
              | StaticDependencyKind::ImportEquals => {
                ende_resolver::workspace::ResolutionKind::Execution
              }
              StaticDependencyKind::ExportType
              | StaticDependencyKind::ImportType
              | StaticDependencyKind::MaybeTsModuleAugmentation => {
                ende_resolver::workspace::ResolutionKind::Types
              }
            }
          };

          analyze_specifier(
            &dep.specifier,
            PositionOrSourceRangeRef::PositionRange(&dep.specifier_range),
            resolution_kind,
            SpecifierTextContext::JavaScript,
            text_changes,
            diagnostic_reporter,
          );
        }
        DependencyDescriptor::Dynamic(dep) => {
          let success = self.try_unfurl_dynamic_dep(
            url,
            text_info,
            dep,
            text_changes,
            diagnostic_reporter,
          );

          if !success {
            let start_pos = text_info.line_start(dep.argument_range.start.line)
              + dep.argument_range.start.character;
            let end_pos = text_info.line_start(dep.argument_range.end.line)
              + dep.argument_range.end.character;
            diagnostic_reporter(
              SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport {
                specifier: url.to_owned(),
                range: SourceRange::new(start_pos, end_pos),
                text_info: text_info.clone(),
              },
            );
          }
        }
      }
    }
    for ts_ref in &module_info.ts_references {
      let specifier_with_range = match ts_ref {
        TypeScriptReference::Path(s) => s,
        TypeScriptReference::Types { specifier, .. } => specifier,
      };
      analyze_specifier(
        &specifier_with_range.text,
        PositionOrSourceRangeRef::PositionRange(&specifier_with_range.range),
        ende_resolver::workspace::ResolutionKind::Types,
        SpecifierTextContext::QuotedComment,
        text_changes,
        diagnostic_reporter,
      );
    }
    for jsdoc in &module_info.jsdoc_imports {
      analyze_specifier(
        &jsdoc.specifier.text,
        PositionOrSourceRangeRef::PositionRange(&jsdoc.specifier.range),
        ende_resolver::workspace::ResolutionKind::Types,
        SpecifierTextContext::QuotedComment,
        text_changes,
        diagnostic_reporter,
      );
    }
    if let Some(specifier_with_range) = &module_info.jsx_import_source {
      analyze_specifier(
        &specifier_with_range.text,
        PositionOrSourceRangeRef::PositionRange(&specifier_with_range.range),
        ende_resolver::workspace::ResolutionKind::Execution,
        SpecifierTextContext::UnquotedComment,
        text_changes,
        diagnostic_reporter,
      );
    }
    if let Some(specifier_with_range) = &module_info.jsx_import_source_types {
      analyze_specifier(
        &specifier_with_range.text,
        PositionOrSourceRangeRef::PositionRange(&specifier_with_range.range),
        ende_resolver::workspace::ResolutionKind::Types,
        SpecifierTextContext::UnquotedComment,
        text_changes,
        diagnostic_reporter,
      );
    }

    let mut collector = ImportMetaResolveCollector::default();
    parsed_source.program().visit_with(&mut collector);
    for (range, specifier) in collector.specifiers {
      analyze_specifier(
        &specifier,
        PositionOrSourceRangeRef::SourceRange(range),
        ende_resolver::workspace::ResolutionKind::Execution,
        SpecifierTextContext::JavaScript,
        text_changes,
        diagnostic_reporter,
      );
    }
    for range in collector.diagnostic_ranges {
      diagnostic_reporter(
        SpecifierUnfurlerDiagnostic::UnanalyzableImportMetaResolve {
          specifier: url.to_owned(),
          range,
          text_info: text_info.clone(),
        },
      );
    }
  }
}

fn relative_url(
  resolved: &ModuleSpecifier,
  referrer: &ModuleSpecifier,
) -> String {
  if resolved.scheme() == "file" {
    let relative = referrer.make_relative(resolved).unwrap();
    if relative.is_empty() {
      let last = resolved.path_segments().unwrap().next_back().unwrap();
      format!("./{last}")
    } else if relative.starts_with("../") {
      relative
    } else {
      format!("./{relative}")
    }
  } else {
    resolved.to_string()
  }
}

#[allow(clippy::disallowed_methods, reason = "test code")]
#[cfg(test)]
mod tests {
  use std::path::Path;
  use std::path::PathBuf;
  use std::sync::Arc;

  use deno_ast::MediaType;
  use deno_ast::ModuleSpecifier;
  use deno_graph::ast::ParserModuleAnalyzer;
  use ende_core::serde_json::json;
  use ende_core::url::Url;
  use ende_resolver::factory::ResolverFactory;
  use ende_resolver::factory::ResolverFactoryOptions;
  use ende_resolver::factory::WorkspaceFactory;
  use ende_resolver::factory::WorkspaceFactoryOptions;
  use pretty_assertions::assert_eq;
  use sys_traits::impls::InMemorySys;

  use super::*;

  fn parse_ast(specifier: &Url, source_code: &str) -> ParsedSource {
    let media_type = MediaType::from_specifier(specifier);
    deno_ast::parse_module(deno_ast::ParseParams {
      specifier: specifier.clone(),
      media_type,
      capture_tokens: false,
      maybe_syntax: None,
      scope_analysis: false,
      text: source_code.into(),
    })
    .unwrap()
  }

  #[tokio::test]
  async fn test_unfurling() {
    let cwd = get_cwd();
    let memory_sys = InMemorySys::new_with_cwd(&cwd);
    memory_sys.fs_insert_json(
      cwd.join("ende.json"),
      json!({
        "workspace": [
          "./jsr-package"
        ],
        "imports": {
          "express": "npm:express@5",
          "lib/": "./lib/",
          "fizz": "./fizz/mod.ts",
          "@std/path": "jsr:@std/path@1",
        }
      }),
    );
    memory_sys.fs_insert_json(
      cwd.join("jsr-package/ende.json"),
      json!({
        "name": "@denotest/example",
        "version": "1.0.0",
        "exports": "./mod.ts"
      }),
    );
    memory_sys.fs_insert(cwd.join("b.ts"), "");
    memory_sys.fs_insert(cwd.join("c.js"), "");
    memory_sys.fs_insert(cwd.join("c.d.ts"), "");
    memory_sys.fs_insert(cwd.join("baz").join("index.js"), "");
    memory_sys.fs_insert(cwd.join("jsr-package/mod.ts"), "export default 1;");
    let unfurler = build_unfurler(memory_sys, &cwd).await;

    // Unfurling TS file should apply changes.
    {
      let source_code = r#"import foo from "lib/foo.ts";
import bar from "lib/bar.ts";
import fizz from "fizz";
import baz from "./baz";
import b from "./b.js";
import b2 from "./b";
import "./mod.ts";
import { } from "./c";
import type { } from "./c";
import url from "url";
import "@denotest/example";
import "@std/path";

const test1 = await import("lib/foo.ts");
const test2 = await import(`lib/foo.ts`);
const test3 = await import(`lib/${expr}`);
const test4 = await import(`./lib/${expr}`);
const test5 = await import("./lib/something.ts");
const test6 = await import(`./lib/something.ts`);
// will warn
const warn1 = await import(`lib${expr}`);
const warn2 = await import(`${expr}`);

import.meta.resolve("fizz");
import.meta.resolve(nonAnalyzable);
"#;
      let (unfurled_source, d) = unfurl_text_with_diagnostics(
        &cwd.join("mod.ts"),
        source_code,
        &unfurler,
      );
      assert_eq!(d.len(), 3);
      assert!(
        matches!(
          d[0],
          SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport { .. }
        ),
        "{:?}",
        d[0]
      );
      assert!(
        matches!(
          d[1],
          SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport { .. }
        ),
        "{:?}",
        d[1]
      );
      assert!(
        matches!(
          d[2],
          SpecifierUnfurlerDiagnostic::UnanalyzableImportMetaResolve { .. }
        ),
        "{:?}",
        d[2]
      );
      let expected_source = r#"import foo from "./lib/foo.ts";
import bar from "./lib/bar.ts";
import fizz from "./fizz/mod.ts";
import baz from "./baz/index.js";
import b from "./b.ts";
import b2 from "./b.ts";
import "./mod.ts";
import { } from "./c.js";
import type { } from "./c.d.ts";
import url from "node:url";
import "jsr:@denotest/example@^1.0.0";
import "jsr:@std/path@1";

const test1 = await import("./lib/foo.ts");
const test2 = await import(`./lib/foo.ts`);
const test3 = await import(`./lib/${expr}`);
const test4 = await import(`./lib/${expr}`);
const test5 = await import("./lib/something.ts");
const test6 = await import(`./lib/something.ts`);
// will warn
const warn1 = await import(`lib${expr}`);
const warn2 = await import(`${expr}`);

import.meta.resolve("./fizz/mod.ts");
import.meta.resolve(nonAnalyzable);
"#;
      assert_eq!(unfurled_source, expected_source);
    }

    // npm specifiers are an error
    {
      let source_code = r#"import express from "express";
import chalk from "npm:chalk@5";
const dynamic = await import("express");
import.meta.resolve("express");
"#;
      let (unfurled_source, d) = unfurl_text_with_diagnostics(
        &cwd.join("mod.ts"),
        source_code,
        &unfurler,
      );
      assert_eq!(d.len(), 4);
      for diagnostic in &d {
        assert!(
          matches!(
            diagnostic,
            SpecifierUnfurlerDiagnostic::UnsupportedNpmSpecifier { .. }
          ),
          "{:?}",
          diagnostic
        );
      }
      assert_eq!(
        d[0].message(),
        "npm packages are not supported in Ende ('npm:express@5')"
      );
      // the specifiers are left as-is
      assert_eq!(unfurled_source, source_code);
    }

    // Unfurling .d.ts file should use types resolution.
    {
      let source_code = r#"import fizz from "fizz";
export type * from "./c";
"#;
      let unfurled_source =
        unfurl_text(&cwd.join("mod.d.ts"), source_code, &unfurler);
      let expected_source = r#"import fizz from "./fizz/mod.ts";
export type * from "./c.d.ts";
"#;
      assert_eq!(unfurled_source, expected_source);
    }
  }

  #[tokio::test]
  async fn test_unfurling_dynamic_prefix_source_ranges() {
    let cwd = get_cwd();
    let memory_sys = InMemorySys::new_with_cwd(&cwd);
    memory_sys.fs_insert_json(
      cwd.join("ende.json"),
      json!({
        "imports": {
          "lib/": "./lib/",
        }
      }),
    );
    let unfurler = build_unfurler(memory_sys, &cwd).await;

    let source_code = r#"const escapedTemplate = await import(`\x6cib/${name}`);
const interpolationDecoy = await import(`\x6cib/${"lib/"}${name}`);
const concatenationDecoy = await import("\x6cib/" + "lib/" + name);
const unchangedEscaped = await import("\x6eode:fs");
const validTemplate = await import(`lib/${name}`);
const validConcatenation = await import("lib/" + name);
const laterUnrelatedText = "lib/";
"#;
    let (unfurled_source, diagnostics) =
      unfurl_text_with_diagnostics(&cwd.join("mod.ts"), source_code, &unfurler);

    assert_eq!(diagnostics.len(), 3);
    assert!(diagnostics.iter().all(|diagnostic| matches!(
      diagnostic,
      SpecifierUnfurlerDiagnostic::UnanalyzableDynamicImport { .. }
    )));
    assert_eq!(
      unfurled_source,
      r#"const escapedTemplate = await import(`\x6cib/${name}`);
const interpolationDecoy = await import(`\x6cib/${"lib/"}${name}`);
const concatenationDecoy = await import("\x6cib/" + "lib/" + name);
const unchangedEscaped = await import("\x6eode:fs");
const validTemplate = await import(`./lib/${name}`);
const validConcatenation = await import("./lib/" + name);
const laterUnrelatedText = "lib/";
"#
    );
  }

  #[tokio::test]
  async fn test_unfurling_escapes_generated_specifiers() {
    let cwd = get_cwd();
    let memory_sys = InMemorySys::new_with_cwd(&cwd);
    memory_sys.fs_insert_json(
      cwd.join("ende.json"),
      json!({
        "imports": {
          "double": "jsr:@scope/package/path\"double",
          "single": "jsr:@scope/package/path'single",
          "templated": "jsr:@scope/package/path`tick${literal}",
        }
      }),
    );
    let unfurler = build_unfurler(memory_sys, &cwd).await;

    let source_code = r#"import double from "double";
import single from 'single';
const templated = await import(`templated`);
import.meta.resolve("double");
"#;
    let unfurled_source =
      unfurl_text(&cwd.join("mod.ts"), source_code, &unfurler);
    assert_eq!(
      unfurled_source,
      r#"import double from "jsr:@scope/package/path\"double";
import single from 'jsr:@scope/package/path\'single';
const templated = await import(`jsr:@scope/package/path\`tick\${literal}`);
import.meta.resolve("jsr:@scope/package/path\"double");
"#
    );
    parse_ast(
      &ModuleSpecifier::from_file_path(cwd.join("mod.ts")).unwrap(),
      &unfurled_source,
    );
  }

  fn get_cwd() -> PathBuf {
    if cfg!(windows) {
      PathBuf::from("C:\\unfurl")
    } else {
      PathBuf::from("/unfurl")
    }
  }

  async fn build_unfurler(
    sys: InMemorySys,
    cwd: &Path,
  ) -> SpecifierUnfurler<InMemorySys> {
    let workspace_factory = Arc::new(WorkspaceFactory::new(
      sys,
      cwd.to_path_buf(),
      WorkspaceFactoryOptions {
        maybe_custom_ende_dir_root: Some(cwd.join("deno_dir")),
        ..Default::default()
      },
    ));
    let resolver_factory = ResolverFactory::new(
      workspace_factory,
      ResolverFactoryOptions {
        unstable_sloppy_imports: true,
        ..Default::default()
      },
    );

    SpecifierUnfurler::new(
      resolver_factory.workspace_resolver().await.unwrap().clone(),
    )
  }

  fn unfurl_text(
    path: &Path,
    source_code: &str,
    unfurler: &SpecifierUnfurler<InMemorySys>,
  ) -> String {
    let (unfurled_source, d) =
      unfurl_text_with_diagnostics(path, source_code, unfurler);
    assert_eq!(d.len(), 0);
    unfurled_source
  }

  fn unfurl_text_with_diagnostics(
    path: &Path,
    source_code: &str,
    unfurler: &SpecifierUnfurler<InMemorySys>,
  ) -> (String, Vec<SpecifierUnfurlerDiagnostic>) {
    let specifier = ModuleSpecifier::from_file_path(path).unwrap();
    let source = parse_ast(&specifier, source_code);
    let mut d = Vec::new();
    let mut reporter = |diagnostic| d.push(diagnostic);
    let unfurled_source = unfurl(unfurler, &specifier, &source, &mut reporter);
    (unfurled_source, d)
  }

  fn unfurl(
    unfurler: &SpecifierUnfurler<InMemorySys>,
    url: &ModuleSpecifier,
    parsed_source: &ParsedSource,
    diagnostic_reporter: &mut dyn FnMut(SpecifierUnfurlerDiagnostic),
  ) -> String {
    let text_info = parsed_source.text_info_lazy();
    let mut text_changes = Vec::new();
    let module_info = ParserModuleAnalyzer::module_info(parsed_source);
    unfurler.unfurl_to_changes(
      url,
      parsed_source,
      &module_info,
      &mut text_changes,
      diagnostic_reporter,
    );

    deno_ast::apply_text_changes(text_info.text_str(), text_changes)
  }
}
