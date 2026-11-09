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
use deno_graph::analysis::ModuleInfo;
use deno_graph::ast::ParserModuleAnalyzer;

const ENDE_TYPES: &str = "@ende-types";
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
  }
}
