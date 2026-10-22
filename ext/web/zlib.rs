// Copyright 2026 the Ende authors. MIT license.

//! Synchronous zlib, raw deflate and gzip for `Ende.Inflater` and
//! `Ende.deflateSync`. Unlike `DecompressionStream`, the inflater stops at
//! the end of one stream and reports how many input bytes it used, so callers
//! can find where a stream ends inside a larger buffer (as in git packs).

use std::cell::RefCell;
use std::io::Write;

use ende_core::convert::Uint8Array;
use ende_core::op2;
use flate2::Compression;
use flate2::Crc;
use flate2::Decompress;
use flate2::FlushDecompress;
use flate2::Status;
use flate2::write::DeflateEncoder;
use flate2::write::GzEncoder;
use flate2::write::ZlibEncoder;

#[derive(Debug, thiserror::Error, deno_error::JsError)]
pub enum ZlibError {
  #[class(type)]
  #[error("Unsupported format")]
  UnsupportedFormat,
  #[class(type)]
  #[error("Invalid {format} data: {message}")]
  InvalidData {
    format: &'static str,
    message: String,
  },
  #[class(range)]
  #[error("Inflated data exceeds maxOutputBytes ({0})")]
  MaxOutputBytesExceeded(u64),
  #[class(type)]
  #[error("The inflater failed on earlier input")]
  Failed,
  #[class(type)]
  #[error(transparent)]
  Io(std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
  Deflate,
  DeflateRaw,
  Gzip,
}

impl Format {
  fn parse(format: &str) -> Result<Self, ZlibError> {
    match format {
      "deflate" => Ok(Self::Deflate),
      "deflate-raw" => Ok(Self::DeflateRaw),
      "gzip" => Ok(Self::Gzip),
      _ => Err(ZlibError::UnsupportedFormat),
    }
  }

  fn name(self) -> &'static str {
    match self {
      Self::Deflate => "deflate",
      Self::DeflateRaw => "deflate-raw",
      Self::Gzip => "gzip",
    }
  }
}

const GZIP_FHCRC: u8 = 0x02;
const GZIP_FEXTRA: u8 = 0x04;
const GZIP_FNAME: u8 = 0x08;
const GZIP_FCOMMENT: u8 = 0x10;
const GZIP_TRAILER_LEN: usize = 8;

/// The length of the gzip member header (RFC 1952) at the start of `buf`, or
/// `None` if `buf` ends before the header does.
fn gzip_header_len(buf: &[u8]) -> Result<Option<usize>, &'static str> {
  if buf.first().is_some_and(|b| *b != 0x1f)
    || buf.get(1).is_some_and(|b| *b != 0x8b)
  {
    return Err("incorrect header magic");
  }
  if buf.get(2).is_some_and(|b| *b != 8) {
    return Err("unknown compression method");
  }
  if buf.get(3).is_some_and(|b| b & 0xe0 != 0) {
    return Err("reserved header flags set");
  }
  if buf.len() < 10 {
    return Ok(None);
  }
  let flags = buf[3];
  let mut len = 10;
  if flags & GZIP_FEXTRA != 0 {
    let Some(xlen) = buf.get(len..len + 2) else {
      return Ok(None);
    };
    len += 2 + u16::from_le_bytes([xlen[0], xlen[1]]) as usize;
    if buf.len() < len {
      return Ok(None);
    }
  }
  for flag in [GZIP_FNAME, GZIP_FCOMMENT] {
    if flags & flag != 0 {
      match buf[len..].iter().position(|b| *b == 0) {
        Some(end) => len += end + 1,
        None => return Ok(None),
      }
    }
  }
  if flags & GZIP_FHCRC != 0 {
    let Some(stored) = buf.get(len..len + 2) else {
      return Ok(None);
    };
    let mut crc = Crc::new();
    crc.update(&buf[..len]);
    if crc.sum() as u16 != u16::from_le_bytes([stored[0], stored[1]]) {
      return Err("header crc mismatch");
    }
    len += 2;
  }
  Ok(Some(len))
}

enum Stage {
  /// Reading a gzip header; holds the bytes seen so far.
  GzipHeader(Vec<u8>),
  Body,
  /// Reading the gzip CRC-32 and size; holds the bytes seen so far.
  GzipTrailer(Vec<u8>),
  Done,
  Failed,
}

struct InflaterState {
  format: Format,
  stage: Stage,
  /// Raw deflate for gzip, whose framing is handled here so that it works
  /// with every flate2 backend.
  decompress: Decompress,
  /// CRC-32 of the output, for the gzip trailer.
  crc: Crc,
  max_output_bytes: u64,
}

impl InflaterState {
  fn new(format: Format, max_output_bytes: u64) -> Self {
    let (stage, zlib_header) = match format {
      Format::Deflate => (Stage::Body, true),
      Format::DeflateRaw => (Stage::Body, false),
      Format::Gzip => (Stage::GzipHeader(Vec::new()), false),
    };
    Self {
      format,
      stage,
      decompress: Decompress::new(zlib_header),
      crc: Crc::new(),
      max_output_bytes,
    }
  }

  fn invalid(&self, message: impl Into<String>) -> ZlibError {
    ZlibError::InvalidData {
      format: self.format.name(),
      message: message.into(),
    }
  }

  /// Returns how many bytes of `input` were used and the output.
  fn push(&mut self, input: &[u8]) -> Result<(usize, Vec<u8>), ZlibError> {
    let mut pos = 0;
    let mut out = Vec::new();

    if let Stage::GzipHeader(seen) = &mut self.stage {
      let before = seen.len();
      seen.extend_from_slice(input);
      match gzip_header_len(seen) {
        Err(message) => return Err(self.invalid(message)),
        Ok(None) => return Ok((input.len(), out)),
        Ok(Some(len)) => {
          pos = len - before;
          self.stage = Stage::Body;
        }
      }
    }

    if matches!(self.stage, Stage::Body) {
      pos += self.inflate(&input[pos..], &mut out)?;
    }

    if let Stage::GzipTrailer(seen) = &mut self.stage {
      let take = (GZIP_TRAILER_LEN - seen.len()).min(input.len() - pos);
      seen.extend_from_slice(&input[pos..pos + take]);
      pos += take;
      if seen.len() == GZIP_TRAILER_LEN {
        let crc = u32::from_le_bytes([seen[0], seen[1], seen[2], seen[3]]);
        let size = u32::from_le_bytes([seen[4], seen[5], seen[6], seen[7]]);
        if crc != self.crc.sum() {
          return Err(self.invalid("incorrect data check"));
        }
        if size != self.decompress.total_out() as u32 {
          return Err(self.invalid("incorrect length check"));
        }
        self.stage = Stage::Done;
      }
    }

    Ok((pos, out))
  }

  /// Inflates deflate data from `input` into `out` until the input runs out
  /// or the stream ends, and returns how many bytes of `input` were used.
  fn inflate(
    &mut self,
    input: &[u8],
    out: &mut Vec<u8>,
  ) -> Result<usize, ZlibError> {
    const MIN_GROWTH: usize = 1024;
    const MAX_INITIAL: usize = 4 * 1024 * 1024;

    let start_in = self.decompress.total_in();
    let mut len = 0;
    loop {
      let read = (self.decompress.total_in() - start_in) as usize;
      // Grow the output, but never past one byte more than the limit allows,
      // which is enough to tell that the limit was exceeded.
      let room = self
        .max_output_bytes
        .saturating_sub(self.decompress.total_out())
        .saturating_add(1);
      let wanted = if out.is_empty() {
        input.len().saturating_mul(4).clamp(MIN_GROWTH, MAX_INITIAL)
      } else {
        out.len()
      };
      let grow = (wanted as u64).min(room) as usize;
      out.resize(len + grow, 0);

      let before_out = self.decompress.total_out();
      let status = self
        .decompress
        .decompress(&input[read..], &mut out[len..], FlushDecompress::None)
        .map_err(|err| self.invalid(err.to_string()))?;
      let written = (self.decompress.total_out() - before_out) as usize;
      if self.format == Format::Gzip {
        self.crc.update(&out[len..len + written]);
      }
      len += written;
      if self.decompress.total_out() > self.max_output_bytes {
        return Err(ZlibError::MaxOutputBytesExceeded(self.max_output_bytes));
      }

      if status == Status::StreamEnd {
        self.stage = if self.format == Format::Gzip {
          Stage::GzipTrailer(Vec::with_capacity(GZIP_TRAILER_LEN))
        } else {
          Stage::Done
        };
        break;
      }
      // Room left over means the input ran out (or can't be used yet); a full
      // buffer may mean more output is pending.
      if len < out.len() {
        break;
      }
    }
    out.truncate(len);
    Ok((self.decompress.total_in() - start_in) as usize)
  }
}

pub struct Inflater(RefCell<InflaterState>);

// SAFETY: we're sure `Inflater` can be GCed
unsafe impl ende_core::GarbageCollected for Inflater {
  fn trace(&self, _visitor: &mut ende_core::v8::cppgc::Visitor) {}

  fn get_name(&self) -> &'static std::ffi::CStr {
    c"Inflater"
  }
}

#[op2]
#[cppgc]
pub fn op_zlib_inflater_new(
  #[string] format: &str,
  #[number] max_output_bytes: u64,
) -> Result<Inflater, ZlibError> {
  let format = Format::parse(format)?;
  Ok(Inflater(RefCell::new(InflaterState::new(
    format,
    max_output_bytes,
  ))))
}

/// Writes the number of bytes used to `result[0]`, and 1 to `result[1]` once
/// the stream has ended.
#[op2]
pub fn op_zlib_inflater_push(
  #[cppgc] inflater: &Inflater,
  #[anybuffer] input: &[u8],
  #[buffer] result: &mut [f64],
) -> Result<Uint8Array, ZlibError> {
  let mut state = inflater.0.borrow_mut();
  let (consumed, out) = match state.stage {
    Stage::Done => (0, Vec::new()),
    Stage::Failed => return Err(ZlibError::Failed),
    _ => match state.push(input) {
      Ok(pushed) => pushed,
      Err(err) => {
        state.stage = Stage::Failed;
        return Err(err);
      }
    },
  };
  result[0] = consumed as f64;
  result[1] = if matches!(state.stage, Stage::Done) {
    1.0
  } else {
    0.0
  };
  Ok(out.into())
}

#[op2]
pub fn op_zlib_deflate(
  #[anybuffer] input: &[u8],
  #[string] format: &str,
  level: u32,
) -> Result<Uint8Array, ZlibError> {
  let level = Compression::new(level);
  let out = Vec::with_capacity(input.len() / 2 + 64);
  let out = match Format::parse(format)? {
    Format::Deflate => {
      let mut encoder = ZlibEncoder::new(out, level);
      encoder.write_all(input).and_then(|_| encoder.finish())
    }
    Format::DeflateRaw => {
      let mut encoder = DeflateEncoder::new(out, level);
      encoder.write_all(input).and_then(|_| encoder.finish())
    }
    Format::Gzip => {
      let mut encoder = GzEncoder::new(out, level);
      encoder.write_all(input).and_then(|_| encoder.finish())
    }
  }
  .map_err(ZlibError::Io)?;
  Ok(out.into())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn gzip_header_len_handles_partial_and_optional_fields() {
    let plain = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    assert_eq!(gzip_header_len(&plain), Ok(Some(10)));
    assert_eq!(gzip_header_len(&plain[..9]), Ok(None));
    assert_eq!(gzip_header_len(&[]), Ok(None));

    let named = [0x1f, 0x8b, 8, GZIP_FNAME, 0, 0, 0, 0, 0, 0xff, b'a', 0, 9];
    assert_eq!(gzip_header_len(&named), Ok(Some(12)));
    assert_eq!(gzip_header_len(&named[..11]), Ok(None));

    assert!(gzip_header_len(&[0x1f, 0x8c]).is_err());
    assert!(gzip_header_len(&[0x1f, 0x8b, 7]).is_err());
    assert!(gzip_header_len(&[0x1f, 0x8b, 8, 0x20]).is_err());
  }

  // The unit tests in tests/unit/zlib_test.ts run on the zlib-ng backend of
  // the ende binary; this checks the stream end on the backend `ende_web`
  // gets on its own.
  #[test]
  fn inflater_stops_at_stream_end() {
    let input: Vec<u8> = (0..20_000u32).map(|i| (i * i % 251) as u8).collect();
    let level = Compression::default();
    for format in [Format::Deflate, Format::DeflateRaw, Format::Gzip] {
      let stream = match format {
        Format::Deflate => {
          let mut encoder = ZlibEncoder::new(Vec::new(), level);
          encoder.write_all(&input).and_then(|_| encoder.finish())
        }
        Format::DeflateRaw => {
          let mut encoder = DeflateEncoder::new(Vec::new(), level);
          encoder.write_all(&input).and_then(|_| encoder.finish())
        }
        Format::Gzip => {
          let mut encoder = GzEncoder::new(Vec::new(), level);
          encoder.write_all(&input).and_then(|_| encoder.finish())
        }
      }
      .unwrap();
      let with_junk = [&stream[..], b"junk"].concat();

      let mut whole = InflaterState::new(format, u64::MAX);
      let (consumed, out) = whole.push(&with_junk).unwrap();
      assert_eq!((consumed, &out), (stream.len(), &input), "{format:?}");
      assert!(matches!(whole.stage, Stage::Done), "{format:?}");

      let mut split = InflaterState::new(format, u64::MAX);
      let (mut consumed, mut out) = (0, Vec::new());
      for byte in with_junk.chunks(1) {
        if matches!(split.stage, Stage::Done) {
          break;
        }
        let (n, data) = split.push(byte).unwrap();
        consumed += n;
        out.extend(data);
      }
      assert_eq!((consumed, &out), (stream.len(), &input), "{format:?}");
    }
  }
}
