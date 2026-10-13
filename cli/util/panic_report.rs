// Copyright 2018-2026 the Deno authors. MIT license.

//! Builds a prefilled GitHub "new issue" link for panic reports.

use percent_encoding::NON_ALPHANUMERIC;
use percent_encoding::utf8_percent_encode;

const NEW_ISSUE_URL: &str = "https://github.com/theende/ende/issues/new";
/// Long URLs get rejected by browsers and GitHub, so stay well below 8 KB.
pub const MAX_URL_LEN: usize = 4000;
const MAX_TITLE_CHARS: usize = 100;

pub struct PanicReport<'a> {
  pub message: &'a str,
  pub version: &'a str,
  pub target: &'a str,
  pub platform: &'a str,
  pub location: Option<String>,
  pub trace: Option<String>,
}

pub fn panic_issue_url(report: &PanicReport) -> String {
  let first_line = report.message.lines().next().unwrap_or("");
  let title: String = format!("panic: {first_line}")
    .chars()
    .take(MAX_TITLE_CHARS)
    .collect();

  let mut body = format!(
    "Version: {}\nTarget: {}\nPlatform: {}\n",
    report.version, report.target, report.platform
  );
  if let Some(location) = &report.location {
    body.push_str(&format!("Location: {location}\n"));
  }
  if let Some(trace) = &report.trace {
    body.push_str(&format!("Trace: {trace}\n"));
  }
  body.push_str(&format!("\nMessage:\n```\n{}\n```\n", report.message));

  let mut url = format!("{NEW_ISSUE_URL}?title={}&body=", encode(&title));
  let room = MAX_URL_LEN.saturating_sub(url.len());
  url.push_str(&encode_within(&body, room));
  url
}

fn encode(text: &str) -> String {
  utf8_percent_encode(text, NON_ALPHANUMERIC).to_string()
}

/// Percent-encodes `text`, stopping before the encoded output would exceed
/// `max` bytes. Cuts only between characters, so escapes stay intact.
fn encode_within(text: &str, max: usize) -> String {
  let mut out = String::new();
  let mut buf = [0; 4];
  for ch in text.chars() {
    let encoded = encode(ch.encode_utf8(&mut buf));
    if out.len() + encoded.len() > max {
      break;
    }
    out.push_str(&encoded);
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  fn report(message: &str) -> PanicReport<'_> {
    PanicReport {
      message,
      version: "2.7.8",
      target: "aarch64-apple-darwin",
      platform: "macos aarch64",
      location: Some("cli/main.rs:10:5".to_string()),
      trace: Some("g7y2Non_5uBw".to_string()),
    }
  }

  fn query_param(url: &str, name: &str) -> String {
    let url = ende_core::url::Url::parse(url).unwrap();
    url
      .query_pairs()
      .find(|(key, _)| key == name)
      .map(|(_, value)| value.into_owned())
      .unwrap()
  }

  #[test]
  fn points_at_new_issue_page() {
    let url = panic_issue_url(&report("boom"));
    assert!(
      url.starts_with("https://github.com/theende/ende/issues/new?title=")
    );
  }

  #[test]
  fn encodes_title_from_first_message_line() {
    let url = panic_issue_url(&report("oh no & more\nsecond line"));
    assert!(url.contains("?title=panic%3A%20oh%20no%20%26%20more&body="));
    assert_eq!(query_param(&url, "title"), "panic: oh no & more");
  }

  #[test]
  fn body_includes_version_platform_location_and_trace() {
    let body = query_param(&panic_issue_url(&report("boom")), "body");
    assert!(body.contains("Version: 2.7.8\n"));
    assert!(body.contains("Target: aarch64-apple-darwin\n"));
    assert!(body.contains("Platform: macos aarch64\n"));
    assert!(body.contains("Location: cli/main.rs:10:5\n"));
    assert!(body.contains("Trace: g7y2Non_5uBw\n"));
    assert!(body.contains("```\nboom\n```"));
  }

  #[test]
  fn long_messages_stay_within_cap_and_decode_cleanly() {
    let message = "é".repeat(10_000);
    let url = panic_issue_url(&report(&message));
    assert!(url.len() <= MAX_URL_LEN, "{} > {MAX_URL_LEN}", url.len());
    let body = query_param(&url, "body");
    assert!(body.starts_with("Version: 2.7.8\n"));
    assert!(!body.contains('\u{FFFD}'));
    assert_eq!(query_param(&url, "title").chars().count(), MAX_TITLE_CHARS);
  }
}
