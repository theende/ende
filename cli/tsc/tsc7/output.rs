// Copyright 2018-2026 the Deno authors. MIT license.

//! Parses the diagnostics TypeScript 7's `tsc --pretty true` prints.
//!
//! `--pretty false` has neither related information ("'x' is declared here")
//! nor end positions, so this reads the pretty format, which has both: the
//! end comes from the `~` underline under each source line, which TypeScript
//! draws in UTF-16 units (a tab counts as one).
//!
//! ```text
//! a.ts:2:13 - error TS2552: Cannot find name 'b'. Did you mean 'a'?
//!   <message chain>
//!
//! 2 console.log(b);
//!               ~
//!
//!   a.ts:1:13 - 'a' is declared here.
//!     1 declare var a: string;
//!                   ~
//! ```
//!
//! A range over more than five lines shows the first two and last two, with
//! `...` between. Based on `cli/tsc/native.rs` from denoland/deno#36104.

use std::sync::LazyLock;

use regex::Regex;

/// `path:line:col - error TS####: message`.
static HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(
    r"^(?P<file>\S.*?):(?P<line>\d+):(?P<col>\d+) - (?P<cat>error|warning|message|suggestion) TS(?P<code>\d+): (?P<msg>.*)$",
  )
  .unwrap()
});

/// The position-less form, for option and program errors.
static NO_POS_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(
    r"^(?P<cat>error|warning|message|suggestion) TS(?P<code>\d+): (?P<msg>.*)$",
  )
  .unwrap()
});

/// Related information: `  path:line:col - message`.
static RELATED_RE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"^  (?P<file>\S.*?):(?P<line>\d+):(?P<col>\d+) - (?P<msg>.*)$")
    .unwrap()
});

/// A source line in a snippet: the line number in the gutter, then the text.
static SNIPPET_LINE_RE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"^ *(?P<line>\d+) ").unwrap());

static ANSI_RE: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
  Error,
  Warning,
  Suggestion,
  Message,
}

/// One diagnostic as tsc printed it. Related information has category
/// `Message` and code 0.
#[derive(Debug, PartialEq, Eq)]
pub struct RawDiagnostic {
  pub category: Category,
  pub code: u64,
  /// The path as printed (relative to tsc's cwd unless absolute), and the
  /// 1-based line and UTF-16 column.
  pub location: Option<(String, u64, u64)>,
  /// The 1-based line and UTF-16 column the range ends at, from the
  /// underline.
  pub end: Option<(u64, u64)>,
  /// The message, with the indented elaboration lines tsc prints under it
  /// (the message chain) kept as they are.
  pub message: String,
  pub related: Vec<RawDiagnostic>,
}

impl RawDiagnostic {
  fn new(
    category: Category,
    code: u64,
    location: Option<(String, u64, u64)>,
    message: &str,
  ) -> Self {
    Self {
      category,
      code,
      location,
      end: None,
      message: message.to_string(),
      related: Vec::new(),
    }
  }
}

#[derive(Clone, Copy, PartialEq)]
enum State {
  Between,
  /// After a header, until the blank line: the message chain.
  Message,
  /// Source and underline lines.
  Snippet,
}

/// Parses `tsc --pretty true` output.
pub fn parse(output: &str) -> Vec<RawDiagnostic> {
  let mut diagnostics: Vec<RawDiagnostic> = Vec::new();
  let mut state = State::Between;
  // The line number of a source line whose underline comes next.
  let mut pending_line = None;
  // (line number, `~` count) of each underline of the current snippet.
  let mut underlines = Vec::new();
  for line in output.lines() {
    let line = ANSI_RE.replace_all(line.trim_end_matches('\r'), "");
    if let Some(line_number) = pending_line.take() {
      underlines.push((line_number, line.matches('~').count() as u64));
      continue;
    }
    match state {
      State::Message => {
        if line.trim().is_empty() {
          state = State::Snippet;
        } else if let Some(last) = diagnostics.last_mut() {
          last.message.push('\n');
          last.message.push_str(line.trim_end());
        }
        continue;
      }
      State::Snippet => {
        if let Some(caps) = SNIPPET_LINE_RE.captures(&line) {
          pending_line = caps["line"].parse().ok();
          continue;
        }
        if line.trim() == "..." {
          continue;
        }
        if let Some(target) = current(&mut diagnostics) {
          set_end(target, &underlines);
        }
        underlines.clear();
        state = State::Between;
      }
      State::Between => {}
    }
    if line.starts_with("Found ") {
      // The summary, and the per-file table after it.
      break;
    }
    if let Some(caps) = HEADER_RE.captures(&line) {
      diagnostics.push(RawDiagnostic::new(
        category(&caps["cat"]),
        caps["code"].parse().unwrap_or(0),
        Some(location(&caps)),
        &caps["msg"],
      ));
      state = State::Message;
    } else if let Some(caps) = NO_POS_RE.captures(&line) {
      diagnostics.push(RawDiagnostic::new(
        category(&caps["cat"]),
        caps["code"].parse().unwrap_or(0),
        None,
        &caps["msg"],
      ));
      state = State::Message;
    } else if let Some(caps) = RELATED_RE.captures(&line)
      && let Some(last) = diagnostics.last_mut()
    {
      last.related.push(RawDiagnostic::new(
        Category::Message,
        0,
        Some(location(&caps)),
        &caps["msg"],
      ));
      // Its snippet follows without a blank line.
      state = State::Snippet;
    }
  }
  if let Some(target) = current(&mut diagnostics) {
    set_end(target, &underlines);
  }
  diagnostics
}

/// The diagnostic or related information being read.
fn current(diagnostics: &mut [RawDiagnostic]) -> Option<&mut RawDiagnostic> {
  let last = diagnostics.last_mut()?;
  if last.related.is_empty() {
    Some(last)
  } else {
    last.related.last_mut()
  }
}

/// The end from the underlines: on one line, the start plus the `~`s; over
/// several, the last line's `~`s, which start at its first column.
fn set_end(target: &mut RawDiagnostic, underlines: &[(u64, u64)]) {
  let (Some((_, line, column)), Some(&(last_line, tildes))) =
    (&target.location, underlines.last())
  else {
    return;
  };
  if target.end.is_some() {
    return;
  }
  target.end = Some(if underlines.len() == 1 && last_line == *line {
    (*line, column + tildes)
  } else {
    (last_line, tildes + 1)
  });
}

fn location(caps: &regex::Captures) -> (String, u64, u64) {
  (
    caps["file"].to_string(),
    caps["line"].parse().unwrap_or(1),
    caps["col"].parse().unwrap_or(1),
  )
}

fn category(name: &str) -> Category {
  match name {
    "warning" => Category::Warning,
    "suggestion" => Category::Suggestion,
    "message" => Category::Message,
    _ => Category::Error,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn diagnostic(
    code: u64,
    location: Option<(&str, u64, u64)>,
    end: Option<(u64, u64)>,
    message: &str,
  ) -> RawDiagnostic {
    RawDiagnostic {
      category: if code == 0 {
        Category::Message
      } else {
        Category::Error
      },
      code,
      location: location.map(|(f, l, c)| (f.to_string(), l, c)),
      end,
      message: message.into(),
      related: Vec::new(),
    }
  }

  #[test]
  fn parses_diagnostics() {
    // What tsc 7.0.2 prints (colors kept for the first one).
    let output = "\x1b[96m../proj/b.ts\x1b[0m:\x1b[93m4\x1b[0m:\x1b[93m5\x1b[0m - \x1b[91merror\x1b[0m\x1b[90m TS2322: \x1b[0mType 'Promise<string>' is not assignable to type 'Promise<number>'.\n\
                  \x20 Type 'string' is not assignable to type 'number'.\n\
                  \x20   Deeper.\n\
                  \n\
                  \x1b[7m4\x1b[0m let n: Promise<number> = s;\n\
                  \x1b[7m \x1b[0m \x1b[91m    ~\x1b[0m\n\
                  \n\
                  C:/proj/c.ts:2:13 - error TS2552: Cannot find name 'b'. Did you mean 'a'?\r\n\
                  \n\
                  2 console.log(b);\n\
                  \x20             ~\n\
                  \n\
                  \x20 C:/proj/c.ts:1:13 - 'a' is declared here.\n\
                  \x20   1 declare var a: string;\n\
                  \x20                 ~~~~~~~~~\n\
                  \x20 lib.es5.d.ts:742:20 - An argument for 'y' was not provided.\n\
                  \x20   742     pow(x: number, y: number): number;\n\
                  \x20                          ~~~~~~~~~\n\
                  \n\
                  error TS5023: Unknown compiler option 'x'.\n\
                  \n\
                  \n\
                  Found 3 errors in 2 files.\n\
                  \n\
                  Errors  Files\n\
                  \x20    1  ../proj/b.ts:4\n";
    let mut related = diagnostic(
      2552,
      Some(("C:/proj/c.ts", 2, 13)),
      Some((2, 14)),
      "Cannot find name 'b'. Did you mean 'a'?",
    );
    related.related = vec![
      diagnostic(
        0,
        Some(("C:/proj/c.ts", 1, 13)),
        Some((1, 22)),
        "'a' is declared here.",
      ),
      diagnostic(
        0,
        Some(("lib.es5.d.ts", 742, 20)),
        Some((742, 29)),
        "An argument for 'y' was not provided.",
      ),
    ];
    assert_eq!(
      parse(output),
      vec![
        diagnostic(
          2322,
          Some(("../proj/b.ts", 4, 5)),
          Some((4, 6)),
          "Type 'Promise<string>' is not assignable to type \
           'Promise<number>'.\n  Type 'string' is not assignable to type \
           'number'.\n    Deeper.",
        ),
        related,
        diagnostic(5023, None, None, "Unknown compiler option 'x'."),
      ]
    );
  }

  #[test]
  fn parses_multi_line_ranges() {
    // Five lines or more: the first two, `...`, the last two.
    let output = "c.ts:2:11 - error TS2365: Operator '+' cannot be applied.\n\
                  \n\
                  2 const r = true\n\
                  \x20           ~~~~\n\
                  3   + 1;\n\
                  \x20 ~~~~~\n\
                  \n\
                  c.ts:4:11 - error TS2365: Operator '+' cannot be applied.\n\
                  \n\
                  \x20 4 const s = true\n\
                  \x20             ~~~~\n\
                  \x20 5   +\n\
                  \x20   ~~~\n\
                  ... \n\
                  \x20 7     1\n\
                  \x20   ~~~~~\n\
                  \x20 8   )\n\
                  \x20   ~~~\n\
                  \n\
                  c.ts:9:1 - error TS1005: ';' expected.\n\
                  \n\
                  9 \n\
                  \x20 \n\
                  \n";
    let ends = parse(output).into_iter().map(|d| d.end).collect::<Vec<_>>();
    assert_eq!(ends, [Some((3, 6)), Some((8, 4)), Some((9, 1))]);
  }

  #[test]
  fn parses_paths_with_parens_and_colons() {
    let diagnostics =
      parse("/a (b)/c:d.ts:2:3 - error TS1005: ';' expected.\n");
    assert_eq!(
      diagnostics[0].location,
      Some(("/a (b)/c:d.ts".into(), 2, 3))
    );
  }
}
