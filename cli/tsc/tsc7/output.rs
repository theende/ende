// Copyright 2018-2026 the Deno authors. MIT license.

//! Parses the diagnostics TypeScript 7's `tsc --pretty false` prints.
//!
//! Based on `cli/tsc/native.rs` from denoland/deno#36104.

use std::sync::LazyLock;

use regex::Regex;

/// `path(line,col): error TS####: message`.
static DIAGNOSTIC_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(
    r"^(?P<file>.+?)\((?P<line>\d+),(?P<col>\d+)\): (?P<cat>error|warning|message|suggestion) TS(?P<code>\d+): (?P<msg>.*)$",
  )
  .unwrap()
});

/// The position-less form, for option and program errors.
static DIAGNOSTIC_NO_POS_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(
    r"^(?P<cat>error|warning|message|suggestion) TS(?P<code>\d+): (?P<msg>.*)$",
  )
  .unwrap()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
  Error,
  Warning,
  Suggestion,
  Message,
}

/// One diagnostic as tsc printed it.
#[derive(Debug, PartialEq, Eq)]
pub struct RawDiagnostic {
  pub category: Category,
  pub code: u64,
  /// The path as printed (relative to tsc's cwd unless absolute), and the
  /// 1-based line and UTF-16 column.
  pub location: Option<(String, u64, u64)>,
  /// The message, with the indented elaboration lines tsc prints under it
  /// (the message chain) kept as they are.
  pub message: String,
}

/// Parses `tsc --pretty false` output. Lines that aren't diagnostics or
/// their elaborations are ignored.
pub fn parse(output: &str) -> Vec<RawDiagnostic> {
  let mut diagnostics: Vec<RawDiagnostic> = Vec::new();
  for line in output.lines() {
    let line = line.trim_end_matches('\r');
    if let Some(caps) = DIAGNOSTIC_RE.captures(line) {
      diagnostics.push(RawDiagnostic {
        category: category(&caps["cat"]),
        code: caps["code"].parse().unwrap_or(0),
        location: Some((
          caps["file"].to_string(),
          caps["line"].parse().unwrap_or(1),
          caps["col"].parse().unwrap_or(1),
        )),
        message: caps["msg"].to_string(),
      });
    } else if let Some(caps) = DIAGNOSTIC_NO_POS_RE.captures(line) {
      diagnostics.push(RawDiagnostic {
        category: category(&caps["cat"]),
        code: caps["code"].parse().unwrap_or(0),
        location: None,
        message: caps["msg"].to_string(),
      });
    } else if line.starts_with([' ', '\t'])
      && !line.trim().is_empty()
      && let Some(last) = diagnostics.last_mut()
    {
      last.message.push('\n');
      last.message.push_str(line.trim_end());
    }
  }
  diagnostics
}

fn category(name: &str) -> Category {
  match name {
    "warning" => Category::Warning,
    "suggestion" => Category::Suggestion,
    "message" => Category::Message,
    _ => Category::Error,
  }
}

/// How many UTF-16 units to underline from `start` (a UTF-16 column) on
/// `line`. The CLI output has no end position, so this guesses the token
/// there: a quoted string with its quotes, an identifier or number, or one
/// character.
pub fn underline_len(line: &str, start: u64) -> u64 {
  let mut chars = Vec::new();
  let mut column = 0;
  let mut start_index = None;
  for c in line.chars() {
    if column == start {
      start_index = Some(chars.len());
    }
    chars.push(c);
    column += c.len_utf16() as u64;
  }
  let Some(start_index) = start_index else {
    return 1;
  };
  let first = chars[start_index];
  let token: &[char] = if matches!(first, '\'' | '"' | '`') {
    match chars[start_index + 1..].iter().position(|&c| c == first) {
      Some(offset) => &chars[start_index..start_index + offset + 2],
      None => &chars[start_index..=start_index],
    }
  } else if is_ident_char(first) {
    let len = chars[start_index..]
      .iter()
      .take_while(|&&c| is_ident_char(c))
      .count();
    &chars[start_index..start_index + len]
  } else {
    &chars[start_index..=start_index]
  };
  token.iter().map(|c| c.len_utf16() as u64).sum()
}

fn is_ident_char(c: char) -> bool {
  c.is_alphanumeric() || c == '_' || c == '$'
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn parses_diagnostics() {
    let output = "../proj/b.ts(4,5): error TS2322: Type 'Promise<string>' is not assignable to type 'Promise<number>'.\n\
                  \x20 Type 'string' is not assignable to type 'number'.\n\
                  \x20   Deeper.\n\
                  C:/proj/c.ts(1,21): warning TS2726: Cannot find lib definition for 'ende.unstable'.\r\n\
                  error TS5023: Unknown compiler option 'x'.\n\
                  \n\
                  Found 3 errors.\n";
    assert_eq!(
      parse(output),
      vec![
        RawDiagnostic {
          category: Category::Error,
          code: 2322,
          location: Some(("../proj/b.ts".into(), 4, 5)),
          message: "Type 'Promise<string>' is not assignable to type \
                    'Promise<number>'.\n  Type 'string' is not assignable \
                    to type 'number'.\n    Deeper."
            .into(),
        },
        RawDiagnostic {
          category: Category::Warning,
          code: 2726,
          location: Some(("C:/proj/c.ts".into(), 1, 21)),
          message: "Cannot find lib definition for 'ende.unstable'.".into(),
        },
        RawDiagnostic {
          category: Category::Error,
          code: 5023,
          location: None,
          message: "Unknown compiler option 'x'.".into(),
        },
      ]
    );
  }

  #[test]
  fn parses_paths_with_parens() {
    let diagnostics =
      parse("/a (b)/c(d).ts(2,3): error TS1005: ';' expected.\n");
    assert_eq!(
      diagnostics[0].location,
      Some(("/a (b)/c(d).ts".into(), 2, 3))
    );
  }

  #[test]
  fn underlines_a_token() {
    let line = r#"const n: number = "a😀b"; f(foo_bar, 12);"#;
    assert_eq!(underline_len(line, 6), 1);
    // The quotes, "a", the emoji (2 units) and "b".
    assert_eq!(underline_len(line, 18), 6);
    assert_eq!(underline_len(line, 28), 7);
    assert_eq!(underline_len(line, 37), 2);
    assert_eq!(underline_len(line, 16), 1);
    assert_eq!(underline_len(line, 100), 1);
  }
}
