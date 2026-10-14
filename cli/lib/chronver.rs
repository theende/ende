// Copyright 2026 the Ende authors. MIT license.

//! Ende's product version: a ChronVer date, `YYYY.MM.DD[.N]`.
//!
//! The date is the UTC day of the release. `N` (1 or more) numbers additional
//! releases made on the same day. Versions carry no compatibility meaning;
//! they only order releases in time.
//!
//! This file is also compiled into `build.rs` (via `#[path]`) so an invalid
//! `version.txt` fails the build, which is why it only depends on `std`.

use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

/// A parsed ChronVer version. Ordered by year, month, day, then `N`, with a
/// version without `N` ordered before any version with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChronVer {
  year: u16,
  month: u8,
  day: u8,
  n: Option<NonZeroU32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChronVerParseError {
  input: String,
}

impl fmt::Display for ChronVerParseError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "Invalid version '{}'. Ende versions are release dates in the form \
       YYYY.MM.DD[.N], like 2026.10.14 or 2026.10.14.1",
      self.input
    )
  }
}

impl std::error::Error for ChronVerParseError {}

impl ChronVer {
  /// Parses `YYYY.MM.DD` or `YYYY.MM.DD.N`, with an optional leading `v`.
  pub fn parse(text: &str) -> Result<Self, ChronVerParseError> {
    Self::parse_parts(text.strip_prefix('v').unwrap_or(text)).ok_or_else(|| {
      ChronVerParseError {
        input: text.to_string(),
      }
    })
  }

  fn parse_parts(text: &str) -> Option<Self> {
    let mut parts = text.split('.');
    let year = parse_digits(parts.next()?, 4)?;
    let month = parse_digits(parts.next()?, 2)?;
    let day = parse_digits(parts.next()?, 2)?;
    let n = match parts.next() {
      None => None,
      // no leading zeros, so each version has one spelling
      Some(n) if n.starts_with('0') => return None,
      Some(n) => Some(NonZeroU32::new(parse_digits(n, n.len())?)?),
    };
    if parts.next().is_some() {
      return None;
    }
    if !(1..=12).contains(&month)
      || !(1..=days_in_month(year, month)).contains(&day)
    {
      return None;
    }
    Some(Self {
      year: year as u16,
      month: month as u8,
      day: day as u8,
      n,
    })
  }
}

/// Parses exactly `len` ASCII digits.
fn parse_digits(text: &str, len: usize) -> Option<u32> {
  if text.is_empty()
    || text.len() != len
    || !text.bytes().all(|b| b.is_ascii_digit())
  {
    return None;
  }
  text.parse().ok()
}

fn days_in_month(year: u32, month: u32) -> u32 {
  match month {
    2 if year.is_multiple_of(4)
      && (!year.is_multiple_of(100) || year.is_multiple_of(400)) =>
    {
      29
    }
    2 => 28,
    4 | 6 | 9 | 11 => 30,
    _ => 31,
  }
}

impl FromStr for ChronVer {
  type Err = ChronVerParseError;

  fn from_str(text: &str) -> Result<Self, Self::Err> {
    Self::parse(text)
  }
}

impl fmt::Display for ChronVer {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{:04}.{:02}.{:02}", self.year, self.month, self.day)?;
    if let Some(n) = self.n {
      write!(f, ".{n}")?;
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn cv(text: &str) -> ChronVer {
    ChronVer::parse(text).unwrap()
  }

  #[test]
  fn parses_date() {
    let version = cv("2026.10.14");
    assert_eq!(version.to_string(), "2026.10.14");
  }

  #[test]
  fn parses_same_day_release_number() {
    assert_eq!(cv("2026.10.14.1").to_string(), "2026.10.14.1");
    assert_eq!(cv("2026.10.14.12").to_string(), "2026.10.14.12");
  }

  #[test]
  fn accepts_leading_v() {
    assert_eq!(cv("v2026.10.14"), cv("2026.10.14"));
    assert_eq!(cv("v2026.10.14.2"), cv("2026.10.14.2"));
  }

  #[test]
  fn display_zero_pads_month_and_day() {
    assert_eq!(cv("2027.01.05").to_string(), "2027.01.05");
    assert_eq!(cv("v2027.01.05.3").to_string(), "2027.01.05.3");
  }

  #[test]
  fn accepts_leap_day_in_leap_years() {
    assert_eq!(cv("2028.02.29").to_string(), "2028.02.29");
    assert_eq!(cv("2000.02.29").to_string(), "2000.02.29");
  }

  #[test]
  fn rejects_invalid_input() {
    for text in [
      "",
      "v",
      "2026",
      "2026.10",
      // SemVer
      "2.9.7",
      "1.45.0",
      "v2.9.7",
      "2.9.7-rc.1",
      // not zero padded
      "2026.1.14",
      "2026.10.4",
      "26.10.14",
      "02026.10.14",
      "2026.010.14",
      // invalid dates
      "2026.00.14",
      "2026.13.01",
      "2026.10.00",
      "2026.10.32",
      "2026.04.31",
      "2026.02.29",
      "1900.02.29",
      // N must be 1 or more, without leading zeros
      "2026.10.14.0",
      "2026.10.14.01",
      "2026.10.14.",
      "2026.10.14.-1",
      "2026.10.14.+1",
      // extra parts or characters
      "2026.10.14.1.1",
      "2026.10.14-rc",
      "2026.10.14+build",
      " 2026.10.14",
      "2026.10.14 ",
      "vv2026.10.14",
      "V2026.10.14",
      "2026-10-14",
      "+2026.10.14",
      "２０２６.10.14",
    ] {
      assert!(ChronVer::parse(text).is_err(), "should reject {text:?}");
    }
  }

  #[test]
  fn parse_error_explains_the_format() {
    let err = ChronVer::parse("2.9.7").unwrap_err().to_string();
    assert_eq!(
      err,
      "Invalid version '2.9.7'. Ende versions are release dates in the form \
       YYYY.MM.DD[.N], like 2026.10.14 or 2026.10.14.1"
    );
  }

  #[test]
  fn orders_by_date_then_release_number() {
    let ordered = [
      "2025.12.31",
      "2025.12.31.1",
      "2026.01.01",
      "2026.01.02",
      "2026.02.01",
      "2026.10.14",
      "2026.10.14.1",
      "2026.10.14.2",
      "2026.10.14.10",
      "2026.10.15",
      "2027.01.01",
    ];
    for pair in ordered.windows(2) {
      assert!(cv(pair[0]) < cv(pair[1]), "{} < {}", pair[0], pair[1]);
    }
    assert_eq!(cv("2026.10.14"), cv("v2026.10.14"));
    assert!(cv("2026.10.14") >= cv("2026.10.14"));
  }

  #[test]
  fn from_str_matches_parse() {
    assert_eq!(
      "2026.10.14.1".parse::<ChronVer>().unwrap(),
      cv("2026.10.14.1")
    );
    assert!("2.9.7".parse::<ChronVer>().is_err());
  }
}
