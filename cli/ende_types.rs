// Copyright 2026 the Ende authors. MIT license.

//! `// @ende-types="..."`, Ende's alias of `// @ts-types="..."`.
//!
//! deno_graph only knows `@ts-types` and `@deno-types`, and its dependency
//! analysis is private. So Ende gives it comments in which each leading
//! `@ende-types` is spelled `@ts-types` plus two spaces: the same length, and
//! the pragma allows spaces before `=`. Everything else stays deno_graph's:
//! which comment counts (the last one before an import), the precedence
//! between the pragmas (by that order) and the ranges, which don't move.

use deno_ast::MultiThreadedComments;
use deno_ast::ParsedSource;
use deno_ast::ProgramRef;
use deno_ast::SourceRangedForSpanned;
use deno_ast::TextChange;
use deno_graph::JsModule;
use deno_graph::ModuleTextSource;
use deno_graph::ParseModuleFromAstOptions;
use deno_graph::analysis::ModuleInfo;
use deno_graph::ast::ParserModuleAnalyzer;

const ENDE_TYPES: &str = "@ende-types";
const TS_TYPES: &str = "@ts-types";
/// `@ts-types` as long as `@ende-types`.
const PADDED_TS_TYPES: &str = "@ts-types  ";

/// Where `@ende-types` starts in a comment's text, if the comment starts with
/// it (case-insensitive, like deno_graph's `@ts-types`).
fn find_in_comment(text: &str) -> Option<usize> {
  let start = text.len() - text.trim_start().len();
  text
    .get(start..start + ENDE_TYPES.len())?
    .eq_ignore_ascii_case(ENDE_TYPES)
    .then_some(start)
}

/// Byte indexes in the source text of the `@ende-types` that are read as
/// `@ts-types`.
fn find_in_source(parsed_source: &ParsedSource) -> Vec<usize> {
  let start_pos = parsed_source.range().start;
  let mut indexes = parsed_source
    .comments()
    .leading_map()
    .values()
    .flatten()
    .filter_map(|comment| {
      let offset = find_in_comment(&comment.text)?;
      // The text starts after the `//` or `/*`.
      Some(comment.start().as_byte_index(start_pos) + 2 + offset)
    })
    .collect::<Vec<_>>();
  indexes.sort_unstable();
  indexes
}

fn text_changes(indexes: &[usize], new_text: &str) -> Vec<TextChange> {
  indexes
    .iter()
    .map(|index| TextChange {
      range: *index..*index + ENDE_TYPES.len(),
      new_text: new_text.to_string(),
    })
    .collect()
}

/// [`ParserModuleAnalyzer::module_info`], with `@ende-types` read as
/// `@ts-types`.
pub fn module_info(parsed_source: &ParsedSource) -> ModuleInfo {
  let comments = parsed_source.comments();
  let has_alias = comments
    .leading_map()
    .values()
    .flatten()
    .any(|comment| find_in_comment(&comment.text).is_some());
  if !has_alias {
    return ParserModuleAnalyzer::module_info(parsed_source);
  }
  let mut leading = comments.leading_map().clone();
  for comment in leading.values_mut().flatten() {
    if let Some(start) = find_in_comment(&comment.text) {
      let mut text = comment.text.to_string();
      text.replace_range(start..start + ENDE_TYPES.len(), PADDED_TS_TYPES);
      comment.text = text.into();
    }
  }
  let comments = MultiThreadedComments::from_leading_and_trailing(
    leading,
    comments.trailing_map().clone(),
  );
  ParserModuleAnalyzer::module_info_from_swc(
    parsed_source.media_type(),
    parsed_source.program_ref(),
    parsed_source.text_info_lazy(),
    &comments,
  )
}

/// [`deno_graph::parse_module_from_ast`], with `@ende-types` read as
/// `@ts-types`.
pub fn parse_module_from_ast(options: ParseModuleFromAstOptions) -> JsModule {
  let parsed_source = options.parsed_source;
  let indexes = find_in_source(parsed_source);
  if indexes.is_empty() {
    return deno_graph::parse_module_from_ast(options);
  }
  // deno_graph reads the comments from the ParsedSource, so parse a copy
  // spelled `@ts-types  `. Only comments change, so the ranges are the same.
  let text = deno_ast::apply_text_changes(
    parsed_source.text(),
    text_changes(&indexes, PADDED_TS_TYPES),
  );
  let params = deno_ast::ParseParams {
    specifier: parsed_source.specifier().clone(),
    text: text.into(),
    media_type: parsed_source.media_type(),
    capture_tokens: false,
    scope_analysis: false,
    maybe_syntax: None,
  };
  // Parse it the way the original was.
  let aliased = if let ProgramRef::Script(_) = parsed_source.program_ref() {
    deno_ast::parse_script(params)
  } else {
    deno_ast::parse_module(params)
  };
  let Ok(aliased) = aliased else {
    return deno_graph::parse_module_from_ast(options);
  };
  let text = parsed_source.text().clone();
  let mut module =
    deno_graph::parse_module_from_ast(ParseModuleFromAstOptions {
      parsed_source: &aliased,
      ..options
    });
  module.source = ModuleTextSource::new_unknown(text);
  module
}

/// Changes that spell each `@ende-types` `@ts-types`, for code that leaves
/// Ende (a package published to JSR is read by JSR and Deno too).
pub fn to_ts_types_text_changes(
  parsed_source: &ParsedSource,
) -> Vec<TextChange> {
  text_changes(&find_in_source(parsed_source), TS_TYPES)
}

#[cfg(test)]
mod tests {
  use deno_ast::MediaType;
  use deno_ast::ModuleSpecifier;
  use deno_graph::Position;
  use deno_graph::PositionRange;
  use deno_graph::analysis::DependencyDescriptor;
  use deno_graph::analysis::SpecifierWithRange;

  use super::*;

  fn parse(text: &str) -> ParsedSource {
    deno_ast::parse_module(deno_ast::ParseParams {
      specifier: ModuleSpecifier::parse("file:///mod.ts").unwrap(),
      text: text.into(),
      media_type: MediaType::TypeScript,
      capture_tokens: false,
      scope_analysis: false,
      maybe_syntax: None,
    })
    .unwrap()
  }

  fn types_specifiers(text: &str) -> Vec<Option<SpecifierWithRange>> {
    module_info(&parse(text))
      .dependencies
      .into_iter()
      .map(|dep| match dep {
        DependencyDescriptor::Static(dep) => dep.types_specifier,
        DependencyDescriptor::Dynamic(dep) => dep.types_specifier,
      })
      .collect()
  }

  fn specifier(text: &str, line: usize, start: usize) -> SpecifierWithRange {
    SpecifierWithRange {
      text: text.to_string(),
      // The range includes the quotes.
      range: PositionRange {
        start: Position {
          line,
          character: start,
        },
        end: Position {
          line,
          character: start + text.len() + 2,
        },
      },
    }
  }

  #[test]
  fn static_import() {
    assert_eq!(
      types_specifiers(
        "// @ende-types=\"./a.d.ts\"\nimport a from \"./a.js\";"
      ),
      vec![Some(specifier("./a.d.ts", 0, 15))],
    );
  }

  #[test]
  fn same_rules_as_ts_types() {
    // Case-insensitive, either quote, spaces around `=`.
    assert_eq!(
      types_specifiers("// @Ende-Types = './a.d.ts'\nexport * from './a.js';"),
      vec![Some(specifier("./a.d.ts", 0, 17))],
    );
    // A block comment before a dynamic import.
    assert_eq!(
      types_specifiers(
        "const a = await /* @ende-types=\"./a.d.ts\" */ import(\"./a.js\");"
      ),
      vec![Some(specifier("./a.d.ts", 0, 31))],
    );
    // Only the last comment counts.
    assert_eq!(
      types_specifiers(
        "// @ende-types=\"./a.d.ts\"\n// note\nimport a from \"./a.js\";"
      ),
      vec![None],
    );
    // Not a pragma anywhere else in the comment, and no unquoted form (that is
    // `@deno-types` only).
    assert_eq!(
      types_specifiers(
        "// see @ende-types=\"./a.d.ts\"\nimport a from \"./a.js\";\n// @ende-types=./b.d.ts\nimport b from \"./b.js\";"
      ),
      vec![None, None],
    );
  }

  #[test]
  fn last_comment_wins() {
    assert_eq!(
      types_specifiers(
        "// @ts-types=\"./a.d.ts\"\n// @ende-types=\"./b.d.ts\"\nimport a from \"./a.js\";\n// @ende-types=\"./c.d.ts\"\n// @deno-types=\"./d.d.ts\"\nimport c from \"./c.js\";"
      ),
      vec![
        Some(specifier("./b.d.ts", 1, 15)),
        Some(specifier("./d.d.ts", 4, 15)),
      ],
    );
  }

  #[test]
  fn without_the_alias() {
    let text = "// @ts-types=\"./a.d.ts\"\nimport a from \"./a.js\";\n";
    let parsed_source = parse(text);
    assert_eq!(
      module_info(&parsed_source),
      ParserModuleAnalyzer::module_info(&parsed_source),
    );
    assert!(to_ts_types_text_changes(&parsed_source).is_empty());
  }

  #[test]
  fn to_ts_types() {
    let text = "// @ende-types=\"./a.d.ts\"\nimport a from \"./a.js\";\n/* @ENDE-TYPES='./b.d.ts' */\nexport * from \"./b.js\";\n// @ende-types=\"./c.d.ts\" note\nimport c from \"./c.js\";\n";
    let parsed_source = parse(text);
    assert_eq!(
      deno_ast::apply_text_changes(
        text,
        to_ts_types_text_changes(&parsed_source)
      ),
      "// @ts-types=\"./a.d.ts\"\nimport a from \"./a.js\";\n/* @ts-types='./b.d.ts' */\nexport * from \"./b.js\";\n// @ts-types=\"./c.d.ts\" note\nimport c from \"./c.js\";\n",
    );
  }

  #[test]
  fn module_from_ast() {
    let text = "// @ende-types=\"./a.d.ts\"\nimport a from \"./a.js\";\n";
    let parsed_source = parse(text);
    let module = parse_module_from_ast(ParseModuleFromAstOptions {
      graph_kind: deno_graph::GraphKind::All,
      specifier: parsed_source.specifier().clone(),
      maybe_headers: None,
      mtime: None,
      parsed_source: &parsed_source,
      file_system: &deno_graph::source::NullFileSystem,
      jsr_url_provider: Default::default(),
      maybe_resolver: None,
    });
    let dep = &module.dependencies["./a.js"];
    assert_eq!(dep.maybe_deno_types_specifier.as_deref(), Some("./a.d.ts"));
    assert_eq!(dep.get_type().unwrap().as_str(), "file:///a.d.ts");
    assert_eq!(dep.get_code().unwrap().as_str(), "file:///a.js");
    let range = dep.maybe_type.maybe_range().unwrap();
    assert_eq!(range.range, specifier("./a.d.ts", 0, 15).range);
    assert_eq!(module.source.text.as_ref(), text);
  }
}
