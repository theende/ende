// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::future::Future;
use std::hash::Hash;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::Poll;
use std::task::Waker;

use deno_ast::EmitOptions;
use deno_ast::ModuleSpecifier;
use deno_ast::SourceMapOption;
use deno_ast::TranspileModuleOptions;
use deno_ast::TranspileOptions;
use deno_graph::ModuleGraph;
use deno_graph::ast::CapturingEsParser;
use deno_graph::ast::EsParser;
use deno_graph::ast::ParseOptions;
use futures::future::poll_fn;
use futures::io::AsyncReadExt;
use hashlink::linked_hash_map::LinkedHashMap;
pub use url::Url;

use crate::Module;
use crate::ModuleInner;
pub use crate::ModuleKind;
use crate::error::ParseError;

const ESZIP_V2_MAGIC: &[u8; 8] = b"ESZIP_V2";
const ESZIP_V2_1_MAGIC: &[u8; 8] = b"ESZIP2.1";
const ESZIP_V2_2_MAGIC: &[u8; 8] = b"ESZIP2.2";
const ESZIP_V2_3_MAGIC: &[u8; 8] = b"ESZIP2.3";
const LATEST_VERSION: EszipVersion = EszipVersion::V2_3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd)]
pub(crate) enum EszipVersion {
  // these numbers are just for ordering
  V2 = 0,
  V2_1 = 1,
  V2_2 = 2,
  V2_3 = 3,
}

impl EszipVersion {
  pub fn from_magic(magic: &[u8; 8]) -> Option<Self> {
    match magic {
      ESZIP_V2_MAGIC => Some(Self::V2),
      ESZIP_V2_1_MAGIC => Some(Self::V2_1),
      ESZIP_V2_2_MAGIC => Some(Self::V2_2),
      ESZIP_V2_3_MAGIC => Some(Self::V2_3),
      _ => None,
    }
  }

  pub fn to_magic(self) -> &'static [u8; 8] {
    match self {
      Self::V2 => ESZIP_V2_MAGIC,
      Self::V2_1 => ESZIP_V2_1_MAGIC,
      Self::V2_2 => ESZIP_V2_2_MAGIC,
      Self::V2_3 => ESZIP_V2_3_MAGIC,
    }
  }
}

#[derive(Debug, PartialEq)]
#[repr(u8)]
enum HeaderFrameKind {
  Module = 0,
  Redirect = 1,
  // 2 was used for npm specifiers, which are skipped when reading eszips
  // written by older versions.
}

#[derive(Debug, Default, Clone)]
pub struct EszipV2Modules(Arc<Mutex<LinkedHashMap<String, EszipV2Module>>>);

impl EszipV2Modules {
  pub(crate) async fn get_module_source(
    &self,
    specifier: &str,
  ) -> Option<Arc<[u8]>> {
    poll_fn(|cx| {
      let mut modules = self.0.lock().unwrap();
      let module = modules.get_mut(specifier).unwrap();
      let slot = match module {
        EszipV2Module::Module { source, .. } => source,
        EszipV2Module::Redirect { .. } => {
          panic!("redirects are already resolved")
        }
      };
      match slot {
        EszipV2SourceSlot::Pending { wakers, .. } => {
          wakers.push(cx.waker().clone());
          Poll::Pending
        }
        EszipV2SourceSlot::Ready(bytes) => Poll::Ready(Some(bytes.clone())),
        EszipV2SourceSlot::Taken => Poll::Ready(None),
      }
    })
    .await
  }

  pub(crate) async fn take_module_source(
    &self,
    specifier: &str,
  ) -> Option<Arc<[u8]>> {
    poll_fn(|cx| {
      let mut modules = self.0.lock().unwrap();
      let module = modules.get_mut(specifier).unwrap();
      let slot = match module {
        EszipV2Module::Module { source, .. } => source,
        EszipV2Module::Redirect { .. } => {
          panic!("redirects are already resolved")
        }
      };
      match slot {
        EszipV2SourceSlot::Pending { wakers, .. } => {
          wakers.push(cx.waker().clone());
          return Poll::Pending;
        }
        EszipV2SourceSlot::Ready(_) => {}
        EszipV2SourceSlot::Taken => return Poll::Ready(None),
      };
      let EszipV2SourceSlot::Ready(bytes) =
        std::mem::replace(slot, EszipV2SourceSlot::Taken)
      else {
        unreachable!()
      };
      Poll::Ready(Some(bytes))
    })
    .await
  }

  pub(crate) async fn get_module_source_map(
    &self,
    specifier: &str,
  ) -> Option<Arc<[u8]>> {
    poll_fn(|cx| {
      let mut modules = self.0.lock().unwrap();
      let module = modules.get_mut(specifier).unwrap();
      let slot = match module {
        EszipV2Module::Module { source_map, .. } => source_map,
        EszipV2Module::Redirect { .. } => {
          panic!("redirects are already resolved")
        }
      };
      match slot {
        EszipV2SourceSlot::Pending { wakers, .. } => {
          wakers.push(cx.waker().clone());
          Poll::Pending
        }
        EszipV2SourceSlot::Ready(bytes) => Poll::Ready(Some(bytes.clone())),
        EszipV2SourceSlot::Taken => Poll::Ready(None),
      }
    })
    .await
  }

  pub(crate) async fn take_module_source_map(
    &self,
    specifier: &str,
  ) -> Option<Arc<[u8]>> {
    let source = poll_fn(|cx| {
      let mut modules = self.0.lock().unwrap();
      let module = modules.get_mut(specifier).unwrap();
      let slot = match module {
        EszipV2Module::Module { source_map, .. } => source_map,
        EszipV2Module::Redirect { .. } => {
          panic!("redirects are already resolved")
        }
      };
      match slot {
        EszipV2SourceSlot::Pending { wakers, .. } => {
          wakers.push(cx.waker().clone());
          Poll::Pending
        }
        EszipV2SourceSlot::Ready(bytes) => Poll::Ready(Some(bytes.clone())),
        EszipV2SourceSlot::Taken => Poll::Ready(None),
      }
    })
    .await;

    // Drop the source map from memory.
    let mut modules = self.0.lock().unwrap();
    let module = modules.get_mut(specifier).unwrap();
    match module {
      EszipV2Module::Module { source_map, .. } => {
        *source_map = EszipV2SourceSlot::Taken;
      }
      EszipV2Module::Redirect { .. } => {
        panic!("redirects are already resolved")
      }
    };
    source
  }
}

#[derive(Debug, Clone, Copy)]
struct Options {
  /// Hash Function used to checksum the contents of the eszip when encoding/decoding
  ///
  /// If the eszip does not include the option, it defaults to `[Checksum::NoChecksum]` in >=v2.2
  /// and `[Checksum::Sha256]` in older versions.  It is `None` when the eszip header includes a
  /// checksum that this version of the library does not know.
  checksum: Option<Checksum>,

  /// Size in Bytes of the hash function digest.
  ///
  /// Defaults to the known length of the configured hash function. Useful in order to ensure forwards compatibility,
  /// otherwise the parser does not know how many bytes to read.
  checksum_size: Option<u8>,
}

impl Options {
  fn default_for_version(version: EszipVersion) -> Self {
    let defaults = Self {
      checksum: Some(Checksum::NoChecksum),
      checksum_size: Default::default(),
    };
    #[cfg(feature = "sha256")]
    let mut defaults = defaults;
    if matches!(version, EszipVersion::V2 | EszipVersion::V2_1) {
      // versions prior to v2.2 default to checksuming with SHA256
      #[cfg(feature = "sha256")]
      {
        defaults.checksum = Some(Checksum::Sha256);
      }
    }
    defaults
  }
}

impl Default for Options {
  fn default() -> Self {
    Self::default_for_version(LATEST_VERSION)
  }
}

impl Options {
  /// Get the size in Bytes of the source hashes
  ///
  /// If the eszip has an explicit digest size, returns that. Otherwise, returns
  /// the default digest size of the [`Self::checksum`]. If the eszip
  /// does not have either, returns `None`.
  fn checksum_size(self) -> Option<u8> {
    self
      .checksum_size
      .or_else(|| Some(self.checksum?.digest_size()))
  }
}

/// A URL that can be designated as the base for relative URLs
/// in an eszip.
///
/// After creation, this URL may be used to get the key for a
/// module in the eszip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EszipRelativeFileBaseUrl<'a>(&'a Url);

impl<'a> From<&'a Url> for EszipRelativeFileBaseUrl<'a> {
  fn from(url: &'a Url) -> Self {
    Self(url)
  }
}

impl<'a> EszipRelativeFileBaseUrl<'a> {
  pub fn new(url: &'a Url) -> Self {
    debug_assert_eq!(url.scheme(), "file");
    Self(url)
  }

  /// Gets the eszip module map key of the provided specifier.
  ///
  /// * Descendant file specifiers will be made relative to the base.
  /// * Non-descendant file specifiers will stay as-is (absolute).
  /// * Non-file specifiers will stay as-is.
  pub fn specifier_key<'b>(&self, target: &'b Url) -> Cow<'b, str> {
    if target.scheme() != "file" {
      return Cow::Borrowed(target.as_str());
    }

    match self.0.make_relative(target) {
      Some(relative) => {
        if relative.starts_with("../") {
          Cow::Borrowed(target.as_str())
        } else {
          Cow::Owned(relative)
        }
      }
      None => Cow::Borrowed(target.as_str()),
    }
  }

  pub fn inner(&self) -> &Url {
    self.0
  }
}

/// Resolves whether the module is ESM or CommonJS for transpilation.
pub trait ModuleKindResolver {
  fn module_kind(
    &self,
    module: &deno_graph::JsModule,
  ) -> Option<deno_ast::ModuleKind>;
}

impl<'a> Default for &'a dyn ModuleKindResolver {
  fn default() -> &'a dyn ModuleKindResolver {
    &NullModuleKindResolver
  }
}

pub struct NullModuleKindResolver;

impl ModuleKindResolver for NullModuleKindResolver {
  fn module_kind(
    &self,
    _module: &deno_graph::JsModule,
  ) -> Option<deno_ast::ModuleKind> {
    None
  }
}

pub struct FromGraphOptions<'a> {
  pub graph: ModuleGraph,
  pub parser: CapturingEsParser<'a>,
  pub module_kind_resolver: &'a dyn ModuleKindResolver,
  pub transpile_options: TranspileOptions,
  pub emit_options: EmitOptions,
  /// Base to make all descendant file:/// modules relative to.
  ///
  /// Note: When a path is above the base it will be left absolute.
  pub relative_file_base: Option<EszipRelativeFileBaseUrl<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Checksum {
  NoChecksum = 0,
  #[cfg(feature = "sha256")]
  Sha256 = 1,
  #[cfg(feature = "xxhash3")]
  XxHash3 = 2,
}

impl Checksum {
  const fn digest_size(self) -> u8 {
    match self {
      Self::NoChecksum => 0,
      #[cfg(feature = "sha256")]
      Self::Sha256 => 32,
      #[cfg(feature = "xxhash3")]
      Self::XxHash3 => 8,
    }
  }

  fn from_u8(discriminant: u8) -> Option<Self> {
    Some(match discriminant {
      0 => Self::NoChecksum,
      #[cfg(feature = "sha256")]
      1 => Self::Sha256,
      #[cfg(feature = "xxhash3")]
      2 => Self::XxHash3,
      _ => return None,
    })
  }
  fn hash(
    self,
    #[cfg_attr(
      not(any(feature = "sha256", feature = "xxhash3")),
      allow(unused)
    )]
    bytes: &[u8],
  ) -> Vec<u8> {
    match self {
      Self::NoChecksum => Vec::new(),
      #[cfg(feature = "sha256")]
      Self::Sha256 => <sha2::Sha256 as sha2::Digest>::digest(bytes)
        .as_slice()
        .to_vec(),
      #[cfg(feature = "xxhash3")]
      Self::XxHash3 => xxhash_rust::xxh3::xxh3_64(bytes).to_be_bytes().into(),
    }
  }
}

/// Version 2 of the Eszip format. This format supports streaming sources and
/// source maps.
#[derive(Debug, Default)]
pub struct EszipV2 {
  modules: EszipV2Modules,
  options: Options,
}

#[derive(Debug)]
pub enum EszipV2Module {
  Module {
    kind: ModuleKind,
    source: EszipV2SourceSlot,
    source_map: EszipV2SourceSlot,
  },
  Redirect {
    target: String,
  },
}

#[derive(Debug)]
pub enum EszipV2SourceSlot {
  Pending {
    offset: usize,
    length: usize,
    wakers: Vec<Waker>,
  },
  Ready(Arc<[u8]>),
  Taken,
}

impl EszipV2SourceSlot {
  fn bytes(&self) -> &[u8] {
    match self {
      EszipV2SourceSlot::Ready(v) => v,
      _ => panic!("EszipV2SourceSlot::bytes() called on a pending slot"),
    }
  }
}

impl EszipV2 {
  pub fn has_magic(buffer: &[u8]) -> bool {
    if buffer.len() < 8 {
      false
    } else {
      EszipVersion::from_magic(&buffer[0..8].try_into().unwrap()).is_some()
    }
  }

  /// Parse a EszipV2 from an AsyncRead stream. This function returns once the
  /// header section of the eszip has been parsed. Once this function returns,
  /// the data section will not necessarially have been parsed yet. To parse
  /// the data section, poll/await the future returned in the second tuple slot.
  pub async fn parse<R: futures::io::AsyncRead + Unpin>(
    mut reader: futures::io::BufReader<R>,
  ) -> Result<
    (
      EszipV2,
      impl Future<Output = Result<futures::io::BufReader<R>, ParseError>>,
    ),
    ParseError,
  > {
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic).await?;

    let Some(version) = EszipVersion::from_magic(&magic) else {
      return Err(ParseError::InvalidV2);
    };

    Self::parse_with_version(version, reader).await
  }

  pub(super) async fn parse_with_version<R: futures::io::AsyncRead + Unpin>(
    version: EszipVersion,
    mut reader: futures::io::BufReader<R>,
  ) -> Result<
    (
      EszipV2,
      impl Future<Output = Result<futures::io::BufReader<R>, ParseError>>,
    ),
    ParseError,
  > {
    let supports_npm = version != EszipVersion::V2;
    let supports_options = version >= EszipVersion::V2_2;

    let mut options = Options::default_for_version(version);

    if supports_options {
      let mut pre_options = options;
      // First read options without checksum, then reread and validate if necessary
      pre_options.checksum = Some(Checksum::NoChecksum);
      pre_options.checksum_size = None;
      let options_header = Section::read(&mut reader, pre_options).await?;
      if options_header.content_len() % 2 != 0 {
        return Err(ParseError::InvalidV22OptionsHeader(String::from(
          "options are expected to be byte tuples",
        )));
      }

      for option in options_header.content().chunks(2) {
        let (option, value) = (option[0], option[1]);
        match option {
          0 => {
            options.checksum = Checksum::from_u8(value);
          }
          1 => {
            options.checksum_size = Some(value);
          }
          _ => {} // Ignore unknown options for forward compatibility
        }
      }
      if options.checksum_size().is_none() {
        return Err(ParseError::InvalidV22OptionsHeader(String::from(
          "checksum size must be known",
        )));
      }

      if let Some(1..) = options.checksum_size() {
        // If the eszip has some checksum configured, the options header is also checksumed. Reread
        // it again with the checksum and validate it
        let options_header_with_checksum = Section::read_with_size(
          options_header.content().chain(&mut reader),
          options,
          options_header.content_len(),
        )
        .await?;
        if !options_header_with_checksum.is_checksum_valid() {
          return Err(ParseError::InvalidV22OptionsHeaderHash);
        }
      }
    }

    let modules_header = Section::read(&mut reader, options).await?;
    if !modules_header.is_checksum_valid() {
      return Err(ParseError::InvalidV2HeaderHash);
    }

    let mut modules = LinkedHashMap::<String, EszipV2Module>::new();

    let mut read = 0;

    // This macro reads n number of bytes from the header section. If the header
    // section is not long enough, this function will be early exited with an
    // error.
    macro_rules! read {
      ($n:expr, $err:expr) => {{
        if read + $n > modules_header.content_len() {
          return Err(ParseError::InvalidV2Header($err));
        }
        let start = read;
        read += $n;
        &modules_header.content()[start..read]
      }};
    }

    while read < modules_header.content_len() {
      let specifier_len =
        u32::from_be_bytes(read!(4, "specifier len").try_into().unwrap())
          as usize;
      let specifier =
        String::from_utf8(read!(specifier_len, "specifier").to_vec())
          .map_err(|_| ParseError::InvalidV2Specifier(read))?;

      let entry_kind = read!(1, "entry kind")[0];
      match entry_kind {
        0 => {
          let source_offset =
            u32::from_be_bytes(read!(4, "source offset").try_into().unwrap());
          let source_len =
            u32::from_be_bytes(read!(4, "source len").try_into().unwrap());
          let source_map_offset = u32::from_be_bytes(
            read!(4, "source map offset").try_into().unwrap(),
          );
          let source_map_len =
            u32::from_be_bytes(read!(4, "source map len").try_into().unwrap());
          let kind = match read!(1, "module kind")[0] {
            0 => ModuleKind::JavaScript,
            1 => ModuleKind::Json,
            2 => ModuleKind::Jsonc,
            3 => ModuleKind::OpaqueData,
            4 => ModuleKind::Wasm,
            n => return Err(ParseError::InvalidV2ModuleKind(n, read)),
          };
          let source = if source_offset == 0 && source_len == 0 {
            EszipV2SourceSlot::Ready(Arc::new([]))
          } else {
            EszipV2SourceSlot::Pending {
              offset: source_offset as usize,
              length: source_len as usize,
              wakers: vec![],
            }
          };
          let source_map = if source_map_offset == 0 && source_map_len == 0 {
            EszipV2SourceSlot::Ready(Arc::new([]))
          } else {
            EszipV2SourceSlot::Pending {
              offset: source_map_offset as usize,
              length: source_map_len as usize,
              wakers: vec![],
            }
          };
          let module = EszipV2Module::Module {
            kind,
            source,
            source_map,
          };
          modules.insert(specifier, module);
        }
        1 => {
          let target_len =
            u32::from_be_bytes(read!(4, "target len").try_into().unwrap())
              as usize;
          let target = String::from_utf8(read!(target_len, "target").to_vec())
            .map_err(|_| ParseError::InvalidV2Specifier(read))?;
          modules.insert(specifier, EszipV2Module::Redirect { target });
        }
        2 if supports_npm => {
          // npm specifier written by an older version; npm packages are not
          // supported, so it's skipped
          let _ = read!(4, "npm package id");
        }
        n => return Err(ParseError::InvalidV2EntryKind(n, read)),
      };
    }

    if supports_npm {
      skip_npm_section(&mut reader, options).await?;
    }

    let mut source_offsets = modules
      .iter()
      .filter_map(|(specifier, m)| {
        if let EszipV2Module::Module {
          source: EszipV2SourceSlot::Pending { offset, length, .. },
          ..
        } = m
        {
          Some((*offset, (*length, specifier.clone())))
        } else {
          None
        }
      })
      .collect::<HashMap<_, _>>();

    let mut source_map_offsets = modules
      .iter()
      .filter_map(|(specifier, m)| {
        if let EszipV2Module::Module {
          source_map: EszipV2SourceSlot::Pending { offset, length, .. },
          ..
        } = m
        {
          Some((*offset, (*length, specifier.clone())))
        } else {
          None
        }
      })
      .collect::<HashMap<_, _>>();

    let modules = Arc::new(Mutex::new(modules));
    let modules_ = modules.clone();

    let fut = async move {
      let modules = modules_;

      let sources_len = read_u32(&mut reader).await? as usize;
      let mut read = 0;

      while read < sources_len {
        let (length, specifier) = source_offsets
          .remove(&read)
          .ok_or(ParseError::InvalidV2SourceOffset(read))?;

        let source_bytes =
          Section::read_with_size(&mut reader, options, length).await?;

        if !source_bytes.is_checksum_valid() {
          return Err(ParseError::InvalidV2SourceHash(specifier));
        }
        read += source_bytes.total_len();

        let wakers = {
          let mut modules = modules.lock().unwrap();
          let module = modules.get_mut(&specifier).expect("module not found");
          match module {
            EszipV2Module::Module { source, .. } => {
              let slot = std::mem::replace(
                source,
                EszipV2SourceSlot::Ready(Arc::from(
                  source_bytes.into_content(),
                )),
              );

              match slot {
                EszipV2SourceSlot::Pending { wakers, .. } => wakers,
                _ => panic!("already populated source slot"),
              }
            }
            _ => panic!("invalid module type"),
          }
        };
        for w in wakers {
          w.wake();
        }
      }

      let source_maps_len = read_u32(&mut reader).await? as usize;
      let mut read = 0;

      while read < source_maps_len {
        let (length, specifier) = source_map_offsets
          .remove(&read)
          .ok_or(ParseError::InvalidV2SourceOffset(read))?;

        let source_map_bytes =
          Section::read_with_size(&mut reader, options, length).await?;
        if !source_map_bytes.is_checksum_valid() {
          return Err(ParseError::InvalidV2SourceHash(specifier));
        }
        read += source_map_bytes.total_len();

        let wakers = {
          let mut modules = modules.lock().unwrap();
          let module = modules.get_mut(&specifier).expect("module not found");
          match module {
            EszipV2Module::Module { source_map, .. } => {
              let slot = std::mem::replace(
                source_map,
                EszipV2SourceSlot::Ready(Arc::from(
                  source_map_bytes.into_content(),
                )),
              );

              match slot {
                EszipV2SourceSlot::Pending { wakers, .. } => wakers,
                _ => panic!("already populated source_map slot"),
              }
            }
            _ => panic!("invalid module type"),
          }
        };
        for w in wakers {
          w.wake();
        }
      }

      Ok(reader)
    };

    Ok((
      EszipV2 {
        modules: EszipV2Modules(modules),
        options,
      },
      fut,
    ))
  }

  /// Add an import map to the eszip archive. The import map will always be
  /// placed at the top of the archive, so it can be read before any other
  /// modules are loaded.
  ///
  /// If a module with this specifier is already present, its source is replaced
  /// with the new source.
  pub fn add_import_map(
    &mut self,
    kind: ModuleKind,
    specifier: String,
    source: Arc<[u8]>,
  ) {
    debug_assert!(matches!(kind, ModuleKind::Json | ModuleKind::Jsonc));
    self.add_to_front(kind, specifier.clone(), source, []);
  }

  /// Add an opaque data to the eszip.
  pub fn add_opaque_data(&mut self, specifier: String, data: Arc<[u8]>) {
    let mut modules = self.modules.0.lock().unwrap();
    modules.insert(
      specifier,
      EszipV2Module::Module {
        kind: ModuleKind::OpaqueData,
        source: EszipV2SourceSlot::Ready(data),
        source_map: EszipV2SourceSlot::Ready(Arc::new([])),
      },
    );
  }

  // Add a module to the front of the eszip
  pub fn add_to_front(
    &mut self,
    kind: ModuleKind,
    specifier: String,
    data: impl Into<Arc<[u8]>>,
    source_map: impl Into<Arc<[u8]>>,
  ) {
    let mut modules = self.modules.0.lock().unwrap();
    modules.insert(
      specifier.clone(),
      EszipV2Module::Module {
        kind,
        source: EszipV2SourceSlot::Ready(data.into()),
        source_map: EszipV2SourceSlot::Ready(source_map.into()),
      },
    );
    modules.to_front(&specifier);
  }

  /// Configure the hash function with which to checksum the source of the modules
  ///
  /// Defaults to `[Checksum::NoChecksum]`.
  pub fn set_checksum(&mut self, checksum: Checksum) {
    self.options.checksum = Some(checksum);
  }

  /// Check if the eszip contents have been (or can be) checksumed
  ///
  /// Returns false if the parsed eszip is not configured with checksum or if it is configured with
  /// a checksum function that the current version of the library does not know (see
  /// [`Self::should_be_checksumed()`]). In that case, the parsing has continued without checksuming
  /// the module's source, therefore proceed with caution.
  pub fn is_checksumed(&self) -> bool {
    self.should_be_checksumed() && self.options.checksum.is_some()
  }

  /// Check if the eszip contents are expected to be checksumed
  ///
  /// Returns false if the eszip is not configured with checksum. if a parsed eszip is configured
  /// with a checksum function that the current version of the library does not know, this method
  /// returns true, and [`Self::is_checksumed()`] returns false. In that case, the parsing has
  /// continued without checksuming the module's source, therefore proceed with caution.
  pub fn should_be_checksumed(&self) -> bool {
    self.options.checksum != Some(Checksum::NoChecksum)
  }

  /// Serialize the eszip archive into a byte buffer.
  pub fn into_bytes(self) -> Vec<u8> {
    fn append_string(bytes: &mut Vec<u8>, string: &str) {
      let len = string.len() as u32;
      bytes.extend_from_slice(&len.to_be_bytes());
      bytes.extend_from_slice(string.as_bytes());
    }

    let (checksum, checksum_size) = self
      .options
      .checksum
      .zip(self.options.checksum_size())
      .expect("checksum function should be known");

    debug_assert_eq!(
      checksum_size,
      checksum.digest_size(),
      "customizing the checksum size should not be posible"
    );

    let mut options_header = LATEST_VERSION.to_magic().to_vec();

    let options_header_length_pos = options_header.len();
    const OPTIONS_HEADER_LENGTH_SIZE: usize = size_of::<u32>();
    options_header.extend_from_slice(&[0; OPTIONS_HEADER_LENGTH_SIZE]); // Reserve for length

    let options_header_start = options_header.len();
    options_header.extend_from_slice(&[0, checksum as u8]);
    options_header.extend_from_slice(&[1, checksum_size]);

    let options_header_length =
      (options_header.len() - options_header_start) as u32;
    options_header[options_header_length_pos..options_header_start]
      .copy_from_slice(&options_header_length.to_be_bytes());
    let options_header_hash =
      checksum.hash(&options_header[options_header_start..]);
    options_header.extend_from_slice(&options_header_hash);

    let mut modules_header = options_header;
    let modules_header_length_pos = modules_header.len();
    modules_header.extend_from_slice(&[0u8; 4]); // add 4 bytes of space to put the header length in later
    let modules_header_start = modules_header.len();
    let mut sources: Vec<u8> = Vec::new();
    let mut source_maps: Vec<u8> = Vec::new();

    let modules = self.modules.0.lock().unwrap();

    for (specifier, module) in modules.iter() {
      append_string(&mut modules_header, specifier);

      match module {
        EszipV2Module::Module {
          kind,
          source,
          source_map,
        } => {
          modules_header.push(HeaderFrameKind::Module as u8);

          // add the source to the `sources` bytes
          let source_bytes = source.bytes();
          let source_length = source_bytes.len() as u32;
          if source_length > 0 {
            let source_offset = sources.len() as u32;
            sources.extend_from_slice(source_bytes);
            sources.extend_from_slice(&checksum.hash(source_bytes));

            modules_header.extend_from_slice(&source_offset.to_be_bytes());
            modules_header.extend_from_slice(&source_length.to_be_bytes());
          } else {
            modules_header.extend_from_slice(&0u32.to_be_bytes());
            modules_header.extend_from_slice(&0u32.to_be_bytes());
          }

          // add the source map to the `source_maps` bytes
          let source_map_bytes = source_map.bytes();
          let source_map_length = source_map_bytes.len() as u32;
          if source_map_length > 0 {
            let source_map_offset = source_maps.len() as u32;
            source_maps.extend_from_slice(source_map_bytes);
            source_maps.extend_from_slice(&checksum.hash(source_map_bytes));

            modules_header.extend_from_slice(&source_map_offset.to_be_bytes());
            modules_header.extend_from_slice(&source_map_length.to_be_bytes());
          } else {
            modules_header.extend_from_slice(&0u32.to_be_bytes());
            modules_header.extend_from_slice(&0u32.to_be_bytes());
          }

          // add module kind to the header
          modules_header.push(*kind as u8);
        }
        EszipV2Module::Redirect { target } => {
          modules_header.push(HeaderFrameKind::Redirect as u8);
          let target_bytes = target.as_bytes();
          let target_length = target_bytes.len() as u32;
          modules_header.extend_from_slice(&target_length.to_be_bytes());
          modules_header.extend_from_slice(target_bytes);
        }
      }
    }

    // populate header length
    let modules_header_length =
      (modules_header.len() - modules_header_start) as u32;
    modules_header[modules_header_length_pos..modules_header_start]
      .copy_from_slice(&modules_header_length.to_be_bytes());

    // add header hash
    let modules_header_bytes = &modules_header[modules_header_start..];
    modules_header.extend_from_slice(&checksum.hash(modules_header_bytes));

    let mut bytes = modules_header;

    // the npm section is always empty, but kept so the format stays the same
    let npm_bytes: &[u8] = &[];
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&checksum.hash(npm_bytes));

    // add sources
    let sources_len = sources.len() as u32;
    bytes.extend_from_slice(&sources_len.to_be_bytes());
    bytes.extend_from_slice(&sources);

    let source_maps_len = source_maps.len() as u32;
    bytes.extend_from_slice(&source_maps_len.to_be_bytes());
    bytes.extend_from_slice(&source_maps);

    bytes
  }

  /// Turn a [deno_graph::ModuleGraph] into an [EszipV2]. All modules from the
  /// graph will be transpiled and stored in the eszip archive.
  ///
  /// The ordering of the modules in the graph is dependant on the module graph
  /// tree. The root module is added to the top of the archive, and the leaves
  /// to the end. This allows for efficient deserialization of the archive right
  /// into an isolate.
  pub fn from_graph(opts: FromGraphOptions) -> Result<Self, anyhow::Error> {
    let mut emit_options = opts.emit_options;
    emit_options.inline_sources = true;
    if emit_options.source_map == SourceMapOption::Inline {
      emit_options.source_map = SourceMapOption::Separate;
    }

    let mut modules = LinkedHashMap::new();

    fn resolve_specifier_key<'a>(
      specifier: &'a Url,
      relative_file_base: Option<EszipRelativeFileBaseUrl>,
    ) -> Result<Cow<'a, str>, anyhow::Error> {
      if let Some(relative_file_base) = relative_file_base {
        Ok(relative_file_base.specifier_key(specifier))
      } else {
        Ok(Cow::Borrowed(specifier.as_str()))
      }
    }

    #[derive(Debug, Clone, Copy)]
    enum ToVisit<'a> {
      Module {
        specifier: &'a ModuleSpecifier,
        is_dynamic: bool,
      },
    }

    impl<'a> ToVisit<'a> {
      fn is_dynamic(self) -> bool {
        matches!(
          self,
          Self::Module {
            is_dynamic: true,
            ..
          }
        )
      }

      fn specifier(self) -> &'a ModuleSpecifier {
        let Self::Module { specifier, .. } = self;
        specifier
      }
    }

    #[allow(
      clippy::too_many_arguments,
      reason = "many parameters needed for module visiting"
    )]
    fn visit_module<'a>(
      graph: &'a ModuleGraph,
      module_kind_provider: &dyn ModuleKindResolver,
      parser: CapturingEsParser,
      transpile_options: &TranspileOptions,
      emit_options: &EmitOptions,
      modules: &mut LinkedHashMap<String, EszipV2Module>,
      visited: ToVisit,
      relative_file_base: Option<EszipRelativeFileBaseUrl>,
    ) -> Result<
      Option<Box<dyn DoubleEndedIterator<Item = ToVisit<'a>> + 'a>>,
      anyhow::Error,
    > {
      let module = match graph.try_get(visited.specifier()) {
        Ok(Some(module)) => module,
        Ok(None) => {
          return Err(anyhow::anyhow!(
            "module not found {}",
            visited.specifier()
          ));
        }
        Err(err) => {
          if visited.is_dynamic() {
            // dynamic imports are allowed to fail
            return Ok(None);
          }
          return Err(anyhow::anyhow!(
            "failed to load '{}': {}",
            visited.specifier(),
            err
          ));
        }
      };

      let specifier_key =
        resolve_specifier_key(module.specifier(), relative_file_base)?;
      if modules.contains_key(specifier_key.as_ref()) {
        return Ok(None);
      }

      match module {
        deno_graph::Module::Js(module) => {
          let source: Arc<[u8]>;
          let source_map: Arc<[u8]>;
          match module.media_type {
            deno_graph::MediaType::JavaScript | deno_graph::MediaType::Mjs => {
              source = Arc::from(module.source.text.clone());
              source_map = Arc::new([]);
            }
            deno_graph::MediaType::Jsx
            | deno_graph::MediaType::TypeScript
            | deno_graph::MediaType::Mts
            | deno_graph::MediaType::Tsx
            | deno_graph::MediaType::Dts
            | deno_graph::MediaType::Dmts => {
              let parsed_source = parser.parse_program(ParseOptions {
                specifier: &module.specifier,
                source: module.source.text.clone(),
                media_type: module.media_type,
                scope_analysis: false,
              })?;
              let emit_options = match relative_file_base {
                Some(relative_file_base)
                  if emit_options.source_map_base.is_none() =>
                {
                  Cow::Owned(EmitOptions {
                    source_map_base: Some(relative_file_base.inner().clone()),
                    ..emit_options.clone()
                  })
                }
                _ => Cow::Borrowed(emit_options),
              };
              let emit = parsed_source
                .transpile(
                  transpile_options,
                  &TranspileModuleOptions {
                    module_kind: module_kind_provider.module_kind(module),
                  },
                  &emit_options,
                )?
                .into_source();
              source = emit.text.into_bytes().into();
              source_map = Arc::from(
                emit.source_map.map(|s| s.into_bytes()).unwrap_or_default(),
              );
            }
            _ => {
              return Err(anyhow::anyhow!(
                "unsupported media type {} for {}",
                module.media_type,
                visited.specifier()
              ));
            }
          };

          let eszip_module = EszipV2Module::Module {
            kind: ModuleKind::JavaScript,
            source: EszipV2SourceSlot::Ready(source),
            source_map: EszipV2SourceSlot::Ready(source_map),
          };
          modules.insert(specifier_key.into_owned(), eszip_module);

          Ok(Some(Box::new(module.dependencies.values().filter_map(
            |dependency| {
              Some(ToVisit::Module {
                specifier: dependency.get_code()?,
                is_dynamic: dependency.is_dynamic,
              })
            },
          ))))
        }
        deno_graph::Module::Wasm(module) => {
          let eszip_module = EszipV2Module::Module {
            kind: ModuleKind::Wasm,
            source: EszipV2SourceSlot::Ready(module.source.clone()),
            source_map: EszipV2SourceSlot::Ready(Arc::new([])), // doesn't seem ideal
          };
          modules.insert(specifier_key.into_owned(), eszip_module);

          Ok(Some(Box::new(module.dependencies.values().filter_map(
            |dependency| {
              Some(ToVisit::Module {
                specifier: dependency.get_code()?,
                is_dynamic: dependency.is_dynamic,
              })
            },
          ))))
        }
        deno_graph::Module::Json(module) => {
          let eszip_module = EszipV2Module::Module {
            kind: ModuleKind::Json,
            source: EszipV2SourceSlot::Ready(module.source.text.clone().into()),
            source_map: EszipV2SourceSlot::Ready(Arc::new([])),
          };
          modules.insert(specifier_key.into_owned(), eszip_module);
          Ok(None)
        }
        deno_graph::Module::Npm(_) => Ok(None),
        deno_graph::Module::External(_) | deno_graph::Module::Node(_) => {
          Ok(None)
        }
      }
    }

    let mut to_visit =
      Vec::from_iter(opts.graph.roots.iter().rev().map(|specifier| {
        ToVisit::Module {
          specifier,
          is_dynamic: false,
        }
      }));
    let mut to_visit_dynamic = VecDeque::new();
    // ende_core's module loading traverses the dependencies breadth first. However, v8 evaluates
    // the source code depth-first. We prioritize module evaluation as it is performed sequentially,
    // thus modules are ordered depth-first within the eszip. Except dynamic
    // imports, which are loaded at runtime, if ever.
    while let Some(module) =
      to_visit.pop().or_else(|| to_visit_dynamic.pop_front())
    {
      let dependencies = visit_module(
        &opts.graph,
        opts.module_kind_resolver,
        opts.parser,
        &opts.transpile_options,
        &emit_options,
        &mut modules,
        module,
        opts.relative_file_base,
      )?;
      if let Some(dependencies) = dependencies {
        let mut level_deps = Vec::new();
        for module in dependencies {
          if module.is_dynamic() {
            to_visit_dynamic.push_back(module);
          } else {
            level_deps.push(module);
          }
        }
        to_visit.extend(level_deps.into_iter().rev());
      }
    }

    for (specifier, target) in &opts.graph.redirects {
      let module = EszipV2Module::Redirect {
        target: target.to_string(),
      };
      let specifier_key =
        resolve_specifier_key(specifier, opts.relative_file_base)?;
      modules.insert(specifier_key.into_owned(), module);
    }

    Ok(Self {
      modules: EszipV2Modules(Arc::new(Mutex::new(modules))),
      options: Options::default(),
    })
  }

  /// Get the module metadata for a given module specifier. This function will
  /// follow redirects. The returned module has functions that can be used to
  /// obtain the module source and source map. The module returned from this
  /// function is guaranteed to be a valid module, which can be loaded into v8.
  ///
  /// Note that this function should be used to obtain a module; if you wish to
  /// get an import map, use [`get_import_map`](Self::get_import_map) instead.
  pub fn get_module(&self, specifier: &str) -> Option<Module> {
    let module = self.lookup(specifier)?;

    // JSONC is contained in this eszip only for use as an import map. In
    // order for the caller to get this JSONC, call `get_import_map` instead.
    if module.kind == ModuleKind::Jsonc {
      return None;
    }

    Some(module)
  }

  /// Get the import map for a given specifier.
  ///
  /// Note that this function should be used to obtain an import map; the returned
  /// "Module" is not necessarily a valid module that can be loaded into v8 (in
  /// other words, JSONC may be returned). If you wish to get a valid module,
  /// use [`get_module`](Self::get_module) instead.
  pub fn get_import_map(&self, specifier: &str) -> Option<Module> {
    let import_map = self.lookup(specifier)?;

    // Import map must be either JSON or JSONC (but JSONC is a special case;
    // it's allowed when embedded in an Ende's config file)
    if !matches!(
      import_map.kind,
      ModuleKind::Json | ModuleKind::Jsonc | ModuleKind::OpaqueData
    ) {
      return None;
    }

    Some(import_map)
  }

  fn lookup(&self, specifier: &str) -> Option<Module> {
    let mut specifier = specifier;
    let mut visited = HashSet::new();
    let modules = self.modules.0.lock().unwrap();
    loop {
      visited.insert(specifier);
      let module = modules.get(specifier)?;
      match module {
        EszipV2Module::Module { kind, .. } => {
          return Some(Module {
            specifier: specifier.to_string(),
            kind: *kind,
            inner: ModuleInner::V2(self.modules.clone()),
          });
        }
        EszipV2Module::Redirect { target } => {
          specifier = target;
          if visited.contains(specifier) {
            return None;
          }
        }
      }
    }
  }

  /// Returns a list of all the module specifiers in this eszip archive.
  pub fn specifiers(&self) -> Vec<String> {
    let modules = self.modules.0.lock().unwrap();
    modules.keys().cloned().collect()
  }
}

/// Get an iterator over all the modules (including an import map, if any) in
/// this eszip archive.
///
/// Note that the iterator will iterate over the specifiers' "snapshot" of the
/// archive. If a new module is added to the archive after the iterator is
/// created via `into_iter()`, that module will not be iterated over.
impl IntoIterator for EszipV2 {
  type Item = (String, Module);
  type IntoIter = std::vec::IntoIter<Self::Item>;

  fn into_iter(self) -> Self::IntoIter {
    let specifiers = self.specifiers();
    let mut v = Vec::with_capacity(specifiers.len());
    for specifier in specifiers {
      let Some(module) = self.lookup(&specifier) else {
        continue;
      };
      v.push((specifier, module));
    }

    v.into_iter()
  }
}

/// Skips the npm section of eszips written by older versions. npm packages
/// are not supported, so its content is ignored.
async fn skip_npm_section<R: futures::io::AsyncRead + Unpin>(
  reader: &mut futures::io::BufReader<R>,
  options: Options,
) -> Result<(), ParseError> {
  let section = Section::read(reader, options).await?;
  if !section.is_checksum_valid() {
    return Err(ParseError::InvalidV2NpmSnapshotHash);
  }
  Ok(())
}

#[derive(Debug)]
struct Section(Vec<u8>, Options);

impl Section {
  /// Reads a section that's defined as:
  ///   Size (4) | Body (n) | Hash (32)
  async fn read<R: futures::io::AsyncRead + Unpin>(
    mut reader: R,
    options: Options,
  ) -> Result<Section, ParseError> {
    let len = read_u32(&mut reader).await? as usize;
    Section::read_with_size(reader, options, len).await
  }

  /// Reads a section that's defined as:
  ///   Body (n) | Hash (32)
  /// Where the `n` size is provided.
  async fn read_with_size<R: futures::io::AsyncRead + Unpin>(
    mut reader: R,
    options: Options,
    len: usize,
  ) -> Result<Section, ParseError> {
    let checksum_size = options
      .checksum_size()
      .expect("Checksum size must be known") as usize;
    let mut body_and_checksum = vec![0u8; len + checksum_size];
    reader.read_exact(&mut body_and_checksum).await?;

    Ok(Section(body_and_checksum, options))
  }

  fn content(&self) -> &[u8] {
    &self.0[..self.content_len()]
  }

  fn into_content(mut self) -> Vec<u8> {
    self.0.truncate(self.content_len());
    self.0
  }

  fn content_len(&self) -> usize {
    self.total_len()
      - self.1.checksum_size().expect("Checksum size must be known") as usize
  }

  fn total_len(&self) -> usize {
    self.0.len()
  }

  fn checksum_hash(&self) -> &[u8] {
    &self.0[self.content_len()..]
  }

  fn is_checksum_valid(&self) -> bool {
    let Some(checksum) = self.1.checksum else {
      // degrade to not checksuming
      return true;
    };
    let actual_hash = checksum.hash(self.content());
    let expected_hash = self.checksum_hash();
    &*actual_hash == expected_hash
  }
}

async fn read_u32<R: futures::io::AsyncRead + Unpin>(
  mut reader: R,
) -> Result<u32, ParseError> {
  let mut buf = [0u8; 4];
  reader.read_exact(&mut buf).await?;
  Ok(u32::from_be_bytes(buf))
}

#[cfg(test)]
mod tests {
  use std::io::Cursor;
  use std::path::Path;
  use std::sync::Arc;

  use deno_ast::EmitOptions;
  use deno_ast::TranspileOptions;
  use deno_error::JsErrorBox;
  use deno_graph::BuildOptions;
  use deno_graph::GraphKind;
  use deno_graph::ModuleGraph;
  use deno_graph::ModuleSpecifier;
  use deno_graph::ast::CapturingModuleAnalyzer;
  use deno_graph::source::CacheSetting;
  use deno_graph::source::LoadOptions;
  use deno_graph::source::LoadResponse;
  use deno_graph::source::MemoryLoader;
  use deno_graph::source::ResolveError;
  use deno_graph::source::Source;
  use futures::io::AllowStdIo;
  use futures::io::BufReader;
  use import_map::ImportMap;
  use pretty_assertions::assert_eq;
  use url::Url;

  use super::Checksum;
  use super::ESZIP_V2_2_MAGIC;
  use super::EszipV2;
  use crate::ModuleKind;

  struct FileLoader {
    base_dir: String,
  }

  macro_rules! assert_matches_file {
    ($source:ident, $file:literal) => {
      assert_eq!(
        String::from_utf8($source.to_vec()).unwrap(),
        include_str!($file)
      );
    };
  }

  macro_rules! assert_matches_file_bytes {
    ($source:ident, $file:literal) => {
      assert_eq!($source.to_vec(), include_bytes!($file));
    };
  }

  macro_rules! assert_content_order {
      ($bytes:expr, $expected_content:expr) => {
      let mut bytes: &[u8] = &$bytes;
      let expected_content: &[&[u8]] = $expected_content;
      for &expected_content in expected_content {
        let mut byte_windows = bytes.windows(expected_content.len());
        let Some(expected_content_pos) =
          byte_windows.position(|window| window == expected_content)
        else {
          panic!(
            "expected content not found.\nExpected: {:?} ({})\nRemaining: {:?} ({})",
            &expected_content, std::str::from_utf8(&expected_content).unwrap(),
            bytes,
            String::from_utf8_lossy(bytes)
          );
        };

        bytes = &bytes[expected_content_pos + expected_content.len()..];
      }
    }
  }

  impl deno_graph::source::Loader for FileLoader {
    #[allow(clippy::disallowed_methods, reason = "test code")]
    fn load(
      &self,
      specifier: &ModuleSpecifier,
      _options: LoadOptions,
    ) -> deno_graph::source::LoadFuture {
      match specifier.scheme() {
        "file" => {
          let path = format!("{}{}", self.base_dir, specifier.path());
          Box::pin(async move {
            let path = Path::new(&path);
            let Ok(resolved) = path.canonicalize() else {
              return Ok(None);
            };
            let source = std::fs::read(&resolved).unwrap();
            let specifier =
              resolved.file_name().unwrap().to_string_lossy().to_string();
            let specifier =
              Url::parse(&format!("file:///{specifier}")).unwrap();
            Ok(Some(LoadResponse::Module {
              content: source.into(),
              maybe_headers: None,
              mtime: None,
              specifier,
            }))
          })
        }
        "data" => {
          let result =
            deno_graph::source::load_data_url(specifier).map_err(|err| {
              deno_graph::source::LoadError::Other(Arc::new(
                JsErrorBox::from_err(err),
              ))
            });
          Box::pin(async move { result })
        }
        "npm" => Box::pin(async { Ok(None) }),
        _ => unreachable!(),
      }
    }
  }

  #[derive(Debug)]
  struct ImportMapResolver(ImportMap);

  impl deno_graph::source::Resolver for ImportMapResolver {
    fn resolve(
      &self,
      specifier: &str,
      referrer_range: &deno_graph::Range,
      _kind: deno_graph::source::ResolutionKind,
    ) -> Result<ModuleSpecifier, ResolveError> {
      self
        .0
        .resolve(specifier, &referrer_range.specifier)
        .map_err(ResolveError::from_err)
    }
  }

  #[tokio::test]
  async fn test_graph_external() {
    let roots = vec![ModuleSpecifier::parse("file:///external.ts").unwrap()];

    struct ExternalLoader;

    impl deno_graph::source::Loader for ExternalLoader {
      #[allow(clippy::disallowed_methods, reason = "test code")]
      fn load(
        &self,
        specifier: &ModuleSpecifier,
        options: LoadOptions,
      ) -> deno_graph::source::LoadFuture {
        if options.in_dynamic_branch {
          unreachable!();
        }
        let scheme = specifier.scheme();
        if scheme == "extern" {
          let specifier = specifier.clone();
          return Box::pin(async move {
            Ok(Some(LoadResponse::External { specifier }))
          });
        }
        assert_eq!(scheme, "file");
        let path = format!("./testdata/source{}", specifier.path());
        Box::pin(async move {
          let path = Path::new(&path);
          let resolved = path.canonicalize().unwrap();
          let source = std::fs::read(&resolved).unwrap();
          let specifier =
            resolved.file_name().unwrap().to_string_lossy().to_string();
          let specifier = Url::parse(&format!("file:///{specifier}")).unwrap();
          Ok(Some(LoadResponse::Module {
            content: source.into(),
            maybe_headers: None,
            mtime: None,
            specifier,
          }))
        })
      }
    }

    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    graph
      .build(
        roots,
        Vec::new(),
        &ExternalLoader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    let module = eszip.get_module("file:///external.ts").unwrap();
    assert_eq!(module.specifier, "file:///external.ts");
    assert!(eszip.get_module("external:fs").is_none());
  }

  #[tokio::test]
  async fn from_graph_redirect() {
    let roots = vec![ModuleSpecifier::parse("file:///main.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    let module = eszip.get_module("file:///main.ts").unwrap();
    assert_eq!(module.specifier, "file:///main.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/emit/main.ts");
    let source_map = module.source_map().await.unwrap();
    assert_matches_file!(source_map, "./testdata/emit/main.ts.map");
    assert_eq!(module.kind, ModuleKind::JavaScript);
    let module = eszip.get_module("file:///a.ts").unwrap();
    assert_eq!(module.specifier, "file:///b.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/emit/b.ts");
    let source_map = module.source_map().await.unwrap();
    assert_matches_file!(source_map, "./testdata/emit/b.ts.map");
    assert_eq!(module.kind, ModuleKind::JavaScript);
  }

  #[tokio::test]
  async fn from_graph_json() {
    let roots = vec![ModuleSpecifier::parse("file:///json.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    let module = eszip.get_module("file:///json.ts").unwrap();
    assert_eq!(module.specifier, "file:///json.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/emit/json.ts");
    let _source_map = module.source_map().await.unwrap();
    assert_eq!(module.kind, ModuleKind::JavaScript);
    let module = eszip.get_module("file:///data.json").unwrap();
    assert_eq!(module.specifier, "file:///data.json");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/source/data.json");
    let source_map = module.source_map().await.unwrap();
    assert_eq!(&*source_map, &[0; 0]);
    assert_eq!(module.kind, ModuleKind::Json);
  }

  #[tokio::test]
  async fn from_graph_wasm() {
    let roots = vec![ModuleSpecifier::parse("file:///wasm.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    let module = eszip.get_module("file:///wasm.ts").unwrap();
    assert_eq!(module.specifier, "file:///wasm.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/source/wasm.ts");
    let module = eszip.get_module("file:///math.wasm").unwrap();
    assert_eq!(module.specifier, "file:///math.wasm");
    let source = module.source().await.unwrap();
    assert_matches_file_bytes!(source, "./testdata/source/math.wasm");
    let source_map = module.source_map().await.unwrap();
    assert_eq!(&*source_map, &[0; 0]);
    assert_eq!(module.kind, ModuleKind::Wasm);
  }

  #[tokio::test]
  async fn loads_eszip_with_wasm() {
    let file = std::fs::File::open("./testdata/wasm.eszip2_3").unwrap();
    let (eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(file)))
        .await
        .unwrap();
    fut.await.unwrap();
    let module = eszip.get_module("file:///wasm.ts").unwrap();
    assert_eq!(module.specifier, "file:///wasm.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/source/wasm.ts");
    let module = eszip.get_module("file:///math.wasm").unwrap();
    assert_eq!(module.specifier, "file:///math.wasm");
    let source = module.source().await.unwrap();
    assert_matches_file_bytes!(source, "./testdata/source/math.wasm");
    let source_map = module.source_map().await.unwrap();
    assert_eq!(&*source_map, &[0; 0]);
    assert_eq!(module.kind, ModuleKind::Wasm);
  }

  #[tokio::test]
  async fn from_graph_dynamic() {
    let roots = vec![ModuleSpecifier::parse("file:///dynamic.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    let module = eszip.get_module("file:///dynamic.ts").unwrap();
    assert_eq!(module.specifier, "file:///dynamic.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/emit/dynamic.ts");
    let _source_map = module.source_map().await.unwrap();
    assert_eq!(module.kind, ModuleKind::JavaScript);
    let module = eszip.get_module("file:///data.json");
    assert!(module.is_some()); // we include statically analyzable dynamic imports
    let mut specifiers = eszip.specifiers();
    specifiers.sort();
    assert_eq!(specifiers, vec!["file:///data.json", "file:///dynamic.ts"]);
  }

  #[tokio::test]
  async fn from_graph_dynamic_data() {
    let roots =
      vec![ModuleSpecifier::parse("file:///dynamic_data.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    let module = eszip.get_module("file:///dynamic_data.ts").unwrap();
    assert_eq!(module.specifier, "file:///dynamic_data.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/emit/dynamic_data.ts");
  }

  #[tokio::test]
  async fn from_graph_relative_base() {
    let base = ModuleSpecifier::parse("file:///dir/").unwrap();
    let roots = vec![ModuleSpecifier::parse("file:///dir/main.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = MemoryLoader::new(
      vec![
        (
          "file:///dir/main.ts".to_string(),
          Source::Module {
            specifier: "file:///dir/main.ts".to_string(),
            maybe_headers: None,
            content: "import './sub_dir/mod.ts';".to_string(),
          },
        ),
        (
          "file:///dir/sub_dir/mod.ts".to_string(),
          Source::Module {
            specifier: "file:///dir/sub_dir/mod.ts".to_string(),
            maybe_headers: None,
            content: "console.log(1);".to_string(),
          },
        ),
      ],
      vec![],
    );
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: Some((&base).into()),
    })
    .unwrap();
    let module = eszip.get_module("main.ts").unwrap();
    assert_eq!(module.specifier, "main.ts");
    let source = module.source().await.unwrap();
    assert_eq!(
      String::from_utf8_lossy(&source),
      "import './sub_dir/mod.ts';\n"
    );
    let module = eszip.get_module("sub_dir/mod.ts").unwrap();
    assert_eq!(module.specifier, "sub_dir/mod.ts");
    let source_map = module.source_map().await.unwrap();
    let value: serde_json::Value =
      serde_json::from_str(&String::from_utf8_lossy(&source_map)).unwrap();
    assert_eq!(
      value,
      serde_json::json!({
        "version": 3,
        "sources": [
          // should be relative
          "sub_dir/mod.ts"
        ],
        "sourcesContent": [
          "console.log(1);"
        ],
        "names": [],
        "mappings": "AAAA,QAAQ,GAAG,CAAC"
      })
    );
  }

  #[cfg(windows)]
  #[tokio::test]
  async fn from_graph_relative_base_windows_different_drives() {
    let base = ModuleSpecifier::parse("file:///V:/dir/").unwrap();
    let roots = vec![ModuleSpecifier::parse("file:///V:/dir/main.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = MemoryLoader::new(
      vec![
        (
          "file:///V:/dir/main.ts".to_string(),
          Source::Module {
            specifier: "file:///V:/dir/main.ts".to_string(),
            maybe_headers: None,
            // obviously this wouldn't work if someone put a V: specifier
            // here, but nobody should be writing code like this so we
            // just do our best effort to keep things working
            content: "import 'file:///C:/other_drive/main.ts';".to_string(),
          },
        ),
        (
          "file:///C:/other_drive/main.ts".to_string(),
          Source::Module {
            specifier: "file:///C:/other_drive/main.ts".to_string(),
            maybe_headers: None,
            content: "console.log(1);".to_string(),
          },
        ),
      ],
      vec![],
    );
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: Some((&base).into()),
    })
    .unwrap();
    let module = eszip.get_module("main.ts").unwrap();
    assert_eq!(module.specifier, "main.ts");
    let source = module.source().await.unwrap();
    assert_eq!(
      String::from_utf8_lossy(&source),
      "import 'file:///C:/other_drive/main.ts';\n"
    );
    let module = eszip.get_module("file:///C:/other_drive/main.ts").unwrap();
    assert_eq!(module.specifier, "file:///C:/other_drive/main.ts");
  }

  #[cfg(feature = "sha256")]
  #[tokio::test]
  async fn file_format_parse_redirect() {
    let file = std::fs::File::open("./testdata/redirect.eszip2").unwrap();
    let (eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(file)))
        .await
        .unwrap();

    let test = async move {
      let module = eszip.get_module("file:///main.ts").unwrap();
      assert_eq!(module.specifier, "file:///main.ts");
      let source = module.source().await.unwrap();
      assert_matches_file!(source, "./testdata/redirect_data/main.ts");
      let source_map = module.source_map().await.unwrap();
      assert_matches_file!(source_map, "./testdata/redirect_data/main.ts.map");
      assert_eq!(module.kind, ModuleKind::JavaScript);
      let module = eszip.get_module("file:///a.ts").unwrap();
      assert_eq!(module.specifier, "file:///b.ts");
      let source = module.source().await.unwrap();
      assert_matches_file!(source, "./testdata/redirect_data/b.ts");
      let source_map = module.source_map().await.unwrap();
      assert_matches_file!(source_map, "./testdata/redirect_data/b.ts.map");
      assert_eq!(module.kind, ModuleKind::JavaScript);

      Ok(())
    };

    tokio::try_join!(fut, test).unwrap();
  }

  #[cfg(feature = "sha256")]
  #[tokio::test]
  async fn file_format_parse_json() {
    let file = std::fs::File::open("./testdata/json.eszip2").unwrap();
    let (eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(file)))
        .await
        .unwrap();

    let test = async move {
      let module = eszip.get_module("file:///json.ts").unwrap();
      assert_eq!(module.specifier, "file:///json.ts");
      let source = module.source().await.unwrap();
      assert_matches_file!(source, "./testdata/source/json.ts");
      let _source_map = module.source_map().await.unwrap();
      assert_eq!(module.kind, ModuleKind::JavaScript);
      let module = eszip.get_module("file:///data.json").unwrap();
      assert_eq!(module.specifier, "file:///data.json");
      let source = module.source().await.unwrap();
      assert_matches_file!(source, "./testdata/emit/data.json");
      let source_map = module.source_map().await.unwrap();
      assert_eq!(&*source_map, &[0; 0]);
      assert_eq!(module.kind, ModuleKind::Json);

      Ok(())
    };

    tokio::try_join!(fut, test).unwrap();
  }

  #[cfg(feature = "sha256")]
  #[tokio::test]
  async fn file_format_roundtrippable() {
    let file = std::fs::File::open("./testdata/redirect.eszip2").unwrap();
    let (eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(file)))
        .await
        .unwrap();
    fut.await.unwrap();
    let bytes = eszip.into_bytes();
    insta::assert_debug_snapshot!(bytes);
    let cursor = Cursor::new(bytes);
    let (eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(cursor)))
        .await
        .unwrap();
    fut.await.unwrap();
    let module = eszip.get_module("file:///main.ts").unwrap();
    assert_eq!(module.specifier, "file:///main.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/redirect_data/main.ts");
    let source_map = module.source_map().await.unwrap();
    assert_matches_file!(source_map, "./testdata/redirect_data/main.ts.map");
    assert_eq!(module.kind, ModuleKind::JavaScript);
    let module = eszip.get_module("file:///a.ts").unwrap();
    assert_eq!(module.specifier, "file:///b.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/redirect_data/b.ts");
    let source_map = module.source_map().await.unwrap();
    assert_matches_file!(source_map, "./testdata/redirect_data/b.ts.map");
    assert_eq!(module.kind, ModuleKind::JavaScript);
  }

  #[tokio::test]
  async fn import_map() {
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };
    let resp = deno_graph::source::Loader::load(
      &loader,
      &Url::parse("file:///import_map.json").unwrap(),
      LoadOptions {
        in_dynamic_branch: false,
        was_dynamic_root: false,
        cache_setting: CacheSetting::Use,
        maybe_checksum: None,
      },
    )
    .await
    .unwrap()
    .unwrap();
    let (specifier, content) = match resp {
      deno_graph::source::LoadResponse::Module {
        specifier, content, ..
      } => (specifier, content),
      _ => unimplemented!(),
    };
    let import_map = import_map::parse_from_json(
      specifier.clone(),
      core::str::from_utf8(&content).unwrap(),
    )
    .unwrap();
    let roots = vec![ModuleSpecifier::parse("file:///mapped.js").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          resolver: Some(&ImportMapResolver(import_map.import_map)),
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let mut eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    eszip.add_import_map(ModuleKind::Json, specifier.to_string(), content);

    let module = eszip.get_module("file:///import_map.json").unwrap();
    assert_eq!(module.specifier, "file:///import_map.json");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/source/import_map.json");
    let source_map = module.source_map().await.unwrap();
    assert_eq!(&*source_map, &[0; 0]);
    assert_eq!(module.kind, ModuleKind::Json);

    let module = eszip.get_module("file:///mapped.js").unwrap();
    assert_eq!(module.specifier, "file:///mapped.js");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/source/mapped.js");
    let source_map = module.source_map().await.unwrap();
    assert_eq!(&*source_map, &[0; 0]);
    assert_eq!(module.kind, ModuleKind::JavaScript);

    let module = eszip.get_module("file:///a.ts").unwrap();
    assert_eq!(module.specifier, "file:///b.ts");
    let source = module.source().await.unwrap();
    assert_matches_file!(source, "./testdata/emit/b.ts");
    let source_map = module.source_map().await.unwrap();
    assert_matches_file!(source_map, "./testdata/emit/b.ts.map");
    assert_eq!(module.kind, ModuleKind::JavaScript);
  }

  // https://github.com/denoland/eszip/issues/110
  #[tokio::test]
  async fn import_map_imported_from_program() {
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };
    let resp = deno_graph::source::Loader::load(
      &loader,
      &Url::parse("file:///import_map.json").unwrap(),
      LoadOptions {
        in_dynamic_branch: false,
        was_dynamic_root: false,
        cache_setting: CacheSetting::Use,
        maybe_checksum: None,
      },
    )
    .await
    .unwrap()
    .unwrap();
    let (specifier, content) = match resp {
      deno_graph::source::LoadResponse::Module {
        specifier, content, ..
      } => (specifier, content),
      _ => unimplemented!(),
    };
    let import_map = import_map::parse_from_json(
      specifier.clone(),
      core::str::from_utf8(&content).unwrap(),
    )
    .unwrap();
    let roots =
      // This file imports `import_map.json` as a module.
      vec![ModuleSpecifier::parse("file:///import_import_map.js").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          resolver: Some(&ImportMapResolver(import_map.import_map)),
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let mut eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    eszip.add_import_map(ModuleKind::Json, specifier.to_string(), content);

    // Verify that the resulting eszip consists of two unique modules even
    // though `import_map.json` is referenced twice:
    // 1. imported from JS
    // 2. specified as the import map
    assert_eq!(
      eszip.specifiers(),
      vec![
        "file:///import_map.json".to_string(),
        "file:///import_import_map.js".to_string(),
      ]
    );
  }

  #[tokio::test]
  async fn ende_jsonc_as_import_map() {
    let loader = FileLoader {
      base_dir: "./testdata/deno_jsonc_as_import_map".to_string(),
    };
    let resp = deno_graph::source::Loader::load(
      &loader,
      &Url::parse("file:///deno.jsonc").unwrap(),
      LoadOptions {
        in_dynamic_branch: false,
        was_dynamic_root: false,
        cache_setting: CacheSetting::Use,
        maybe_checksum: None,
      },
    )
    .await
    .unwrap()
    .unwrap();
    let (specifier, content) = match resp {
      deno_graph::source::LoadResponse::Module {
        specifier, content, ..
      } => (specifier, content),
      _ => unimplemented!(),
    };
    let import_map = import_map::parse_from_value(
      specifier.clone(),
      jsonc_parser::parse_to_serde_value(
        core::str::from_utf8(&content).unwrap(),
        &Default::default(),
      )
      .unwrap(),
    )
    .unwrap();
    let roots = vec![ModuleSpecifier::parse("file:///main.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          resolver: Some(&ImportMapResolver(import_map.import_map)),
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let mut eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    eszip.add_import_map(ModuleKind::Jsonc, specifier.to_string(), content);

    assert_eq!(
      eszip.specifiers(),
      vec![
        "file:///deno.jsonc".to_string(),
        "file:///main.ts".to_string(),
        "file:///a.ts".to_string(),
      ],
    );

    // JSONC can be obtained by calling `get_import_map`
    let ende_jsonc = eszip.get_import_map("file:///deno.jsonc").unwrap();
    let source = ende_jsonc.source().await.unwrap();
    assert_matches_file!(
      source,
      "./testdata/deno_jsonc_as_import_map/deno.jsonc"
    );

    // JSONC can NOT be obtained as a module
    assert!(eszip.get_module("file:///deno.jsonc").is_none());
  }

  #[tokio::test]
  async fn eszipv2_iterator_yields_all_modules() {
    let loader = FileLoader {
      base_dir: "./testdata/deno_jsonc_as_import_map".to_string(),
    };
    let resp = deno_graph::source::Loader::load(
      &loader,
      &Url::parse("file:///deno.jsonc").unwrap(),
      LoadOptions {
        in_dynamic_branch: false,
        was_dynamic_root: false,
        cache_setting: CacheSetting::Use,
        maybe_checksum: None,
      },
    )
    .await
    .unwrap()
    .unwrap();
    let (specifier, content) = match resp {
      deno_graph::source::LoadResponse::Module {
        specifier, content, ..
      } => (specifier, content),
      _ => unimplemented!(),
    };
    let import_map = import_map::parse_from_value(
      specifier.clone(),
      jsonc_parser::parse_to_serde_value(
        core::str::from_utf8(&content).unwrap(),
        &Default::default(),
      )
      .unwrap(),
    )
    .unwrap();
    let roots = vec![ModuleSpecifier::parse("file:///main.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          resolver: Some(&ImportMapResolver(import_map.import_map)),
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    let mut eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();
    eszip.add_import_map(ModuleKind::Jsonc, specifier.to_string(), content);

    struct Expected {
      specifier: String,
      source: &'static str,
      kind: ModuleKind,
    }

    let expected = vec![
      Expected {
        specifier: "file:///deno.jsonc".to_string(),
        source: include_str!("testdata/deno_jsonc_as_import_map/deno.jsonc"),
        kind: ModuleKind::Jsonc,
      },
      Expected {
        specifier: "file:///main.ts".to_string(),
        source: include_str!("testdata/deno_jsonc_as_import_map/main.ts"),
        kind: ModuleKind::JavaScript,
      },
      Expected {
        specifier: "file:///a.ts".to_string(),
        source: include_str!("testdata/deno_jsonc_as_import_map/a.ts"),
        kind: ModuleKind::JavaScript,
      },
    ];

    for (got, expected) in eszip.into_iter().zip(expected) {
      let (got_specifier, got_module) = got;

      assert_eq!(got_specifier, expected.specifier);
      assert_eq!(got_module.kind, expected.kind);
      assert_eq!(
        String::from_utf8_lossy(&got_module.source().await.unwrap()),
        expected.source
      );
    }
  }

  #[cfg(feature = "sha256")]
  #[tokio::test]
  async fn npm_section_of_older_eszips_is_skipped() {
    // eszips written by older versions with npm packages still load
    let file = std::fs::File::open("./testdata/npm_packages.eszip2_1").unwrap();
    let (eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(file)))
        .await
        .unwrap();
    fut.await.unwrap();
    assert!(eszip.get_module("file:///main.ts").is_some());

    let file =
      std::fs::File::open("./testdata/no_npm_packages.eszip2_1").unwrap();
    let (_eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(file)))
        .await
        .unwrap();
    fut.await.unwrap();

    // invalid file with one byte changed in the npm snapshot
    let file =
      std::fs::File::open("./testdata/npm_packages_invalid_1.eszip2_1")
        .unwrap();
    let err = super::EszipV2::parse(BufReader::new(AllowStdIo::new(file)))
      .await
      .err()
      .unwrap();
    assert_eq!(err.to_string(), "invalid eszip v2.1 npm snapshot hash");
  }

  #[tokio::test]
  async fn into_bytes_sequences_modules_depth_first() {
    let roots = vec![ModuleSpecifier::parse("file:///parent.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };

    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();

    let eszip = super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap();

    let eszip_bytes = eszip.into_bytes();
    let expected_content: &[&[u8]] = &[
      b"import \"./child1.ts\";\nimport \"./child2.ts\";",
      b"import \"./grandchild1.ts\";",
      b"export const grandchild1 = \"grandchild1\";",
      b"import \"./grandchild2.ts\";",
      b"export const grandchild2 = \"grandchild2\";",
    ];
    assert_content_order!(eszip_bytes, expected_content);
  }

  #[tokio::test]
  async fn add_to_front_adds_module_to_the_front_instead_of_the_back() {
    let mut eszip = super::EszipV2::default();

    eszip.add_opaque_data(
      String::from("third"),
      Arc::from(*b"third source added with add_opaque_data"),
    );
    eszip.add_to_front(
      ModuleKind::OpaqueData,
      String::from("second"),
      *b"second source added with add_to_front",
      *b"second source map added with add_to_front",
    );
    eszip.add_to_front(
      ModuleKind::OpaqueData,
      String::from("first"),
      *b"first source added with add_to_front",
      *b"first source map added with add_to_front",
    );

    let eszip_bytes = eszip.into_bytes();
    let expected_content: &[&[u8]] = &[
      b"first source added with add_to_front",
      b"second source added with add_to_front",
      b"third source added with add_opaque_data",
      b"first source map added with add_to_front",
      b"second source map added with add_to_front",
    ];
    assert_content_order!(eszip_bytes, expected_content);
  }

  #[tokio::test]
  async fn opaque_data() {
    let mut eszip = super::EszipV2::default();
    let opaque_data: Arc<[u8]> = Arc::new([1, 2, 3]);
    eszip.add_opaque_data("+s/foobar".to_string(), opaque_data.clone());
    let bytes = eszip.into_bytes();
    insta::assert_debug_snapshot!(bytes);
    let cursor = Cursor::new(bytes);
    let (eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(cursor)))
        .await
        .unwrap();
    fut.await.unwrap();
    let opaque_data = eszip.get_module("+s/foobar").unwrap();
    assert_eq!(opaque_data.specifier, "+s/foobar");
    let source = opaque_data.source().await.unwrap();
    assert_eq!(&*source, &[1, 2, 3]);
    assert_eq!(opaque_data.kind, ModuleKind::OpaqueData);
  }

  #[tokio::test]
  async fn v2_2_defaults_to_no_checksum() {
    let eszip = main_eszip().await;
    let bytes = eszip.into_bytes();
    let (eszip, fut) = super::EszipV2::parse(BufReader::new(bytes.as_slice()))
      .await
      .unwrap();
    fut.await.unwrap();
    assert_eq!(eszip.options.checksum, Some(super::Checksum::NoChecksum));
    assert!(!eszip.is_checksumed());
    assert!(!eszip.should_be_checksumed());
  }

  #[cfg(feature = "sha256")]
  #[tokio::test]
  async fn v2_1_and_older_default_to_sha256_checksum() {
    let file = std::fs::File::open("./testdata/json.eszip2").unwrap();
    let (eszip, fut) =
      super::EszipV2::parse(BufReader::new(AllowStdIo::new(file)))
        .await
        .unwrap();
    fut.await.unwrap();
    assert_eq!(eszip.options.checksum, Some(super::Checksum::Sha256));
    assert_eq!(eszip.options.checksum_size(), Some(32));
    assert!(eszip.is_checksumed());
  }

  #[cfg(feature = "xxhash3")]
  #[tokio::test]
  async fn v2_2_set_xxhash3_checksum() {
    let mut eszip = main_eszip().await;
    eszip.set_checksum(super::Checksum::XxHash3);
    let main_source = eszip
      .get_module("file:///main.ts")
      .unwrap()
      .source()
      .await
      .unwrap();
    let bytes = eszip.into_bytes();
    let main_xxhash = xxhash_rust::xxh3::xxh3_64(&main_source).to_be_bytes();
    let xxhash_in_bytes = bytes
      .windows(main_xxhash.len())
      .any(|window| window == main_xxhash);
    assert!(xxhash_in_bytes);
    let (parsed_eszip, fut) = EszipV2::parse(BufReader::new(bytes.as_slice()))
      .await
      .unwrap();
    fut.await.unwrap();
    assert_eq!(
      parsed_eszip.options.checksum,
      Some(super::Checksum::XxHash3)
    );
    assert!(parsed_eszip.is_checksumed());
  }

  #[tokio::test]
  async fn v2_2_options_in_header_are_optional() {
    let empty_options = 0_u32.to_be_bytes();
    let bytes = main_eszip().await.into_bytes();
    let existing_options_size =
      std::mem::size_of::<u32>() + std::mem::size_of::<u8>() * 4;
    let options_start = ESZIP_V2_2_MAGIC.len();
    // Replace the default options set by the library with an empty options header
    let new_bytes = [
      &bytes[..options_start],
      empty_options.as_slice(),
      &bytes[options_start + existing_options_size..],
    ]
    .concat();
    let (new_eszip, fut) = EszipV2::parse(BufReader::new(new_bytes.as_slice()))
      .await
      .unwrap();
    fut.await.unwrap();

    assert_eq!(new_eszip.options.checksum, Some(Checksum::NoChecksum));
    assert!(!new_eszip.is_checksumed());
    assert!(!new_eszip.should_be_checksumed());
  }

  #[cfg(feature = "sha256")]
  #[tokio::test]
  #[should_panic]
  async fn v2_2_unknown_checksum_function_degrades_to_no_checksum() {
    // checksum 255; checksum_size 32
    let option_bytes = &[0, 255, 1, 32];
    let futuristic_options = [
      4_u32.to_be_bytes().as_slice(),
      option_bytes,
      <sha2::Sha256 as sha2::Digest>::digest(option_bytes).as_slice(),
    ]
    .concat();
    let mut eszip = main_eszip().await;
    // Using sha256/32Bytes as mock hash.
    eszip.set_checksum(Checksum::Sha256);
    let bytes = eszip.into_bytes();
    let existing_options_size = std::mem::size_of::<u32>()
      + std::mem::size_of::<u8>() * 4
      + <sha2::Sha256 as sha2::Digest>::output_size();
    let options_start = ESZIP_V2_2_MAGIC.len();
    let new_bytes = [
      &bytes[..options_start],
      futuristic_options.as_slice(),
      &bytes[options_start + existing_options_size..],
    ]
    .concat();
    let (new_eszip, fut) = EszipV2::parse(BufReader::new(new_bytes.as_slice()))
      .await
      .unwrap();
    fut.await.unwrap();

    assert_eq!(new_eszip.options.checksum, None);
    assert_eq!(new_eszip.options.checksum_size(), Some(32));
    assert!(!new_eszip.is_checksumed());
    assert!(new_eszip.should_be_checksumed());

    // This should panic, as cannot re-encode without setting an explicit checksum configuration
    new_eszip.into_bytes();
  }

  #[cfg(feature = "sha256")]
  #[tokio::test]
  async fn wrong_checksum() {
    let mut eszip = main_eszip().await;
    eszip.set_checksum(Checksum::Sha256);
    let main_source = eszip
      .get_module("file:///main.ts")
      .unwrap()
      .source()
      .await
      .unwrap();
    let bytes = eszip.into_bytes();
    let mut main_sha256 = <sha2::Sha256 as sha2::Digest>::digest(&main_source);
    let sha256_in_bytes_start = bytes
      .windows(main_sha256.len())
      .position(|window| window == &*main_sha256)
      .unwrap();
    main_sha256.reverse();
    let bytes = [
      &bytes[..sha256_in_bytes_start],
      main_sha256.as_slice(),
      &bytes[sha256_in_bytes_start + main_sha256.len()..],
    ]
    .concat();
    let (_eszip, fut) = EszipV2::parse(BufReader::new(bytes.as_slice()))
      .await
      .unwrap();
    let result = fut.await;
    assert!(result.is_err());
    assert!(matches!(
      result,
      Err(crate::error::ParseError::InvalidV2SourceHash(_))
    ));
  }

  #[tokio::test]
  async fn v2_2_options_forward_compatibility() {
    let option_bytes = &[255; 98];
    let futuristic_options =
      [98_u32.to_be_bytes().as_slice(), option_bytes].concat();
    let bytes = main_eszip().await.into_bytes();
    let existing_options_size =
      std::mem::size_of::<u32>() + std::mem::size_of::<u8>() * 4;
    let options_start = ESZIP_V2_2_MAGIC.len();
    let new_bytes = [
      &bytes[..options_start],
      futuristic_options.as_slice(),
      &bytes[options_start + existing_options_size..],
    ]
    .concat();
    // Assert that unknown options are ignored just fine
    let (_new_eszip, fut) =
      EszipV2::parse(BufReader::new(new_bytes.as_slice()))
        .await
        .unwrap();
    fut.await.unwrap();
  }

  async fn main_eszip() -> EszipV2 {
    let roots = vec![ModuleSpecifier::parse("file:///main.ts").unwrap()];
    let analyzer = CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(GraphKind::CodeOnly);
    let loader = FileLoader {
      base_dir: "./testdata/source".to_string(),
    };
    graph
      .build(
        roots,
        Vec::new(),
        &loader,
        BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph.valid().unwrap();
    super::EszipV2::from_graph(super::FromGraphOptions {
      graph,
      module_kind_resolver: Default::default(),
      parser: analyzer.as_capturing_parser(),
      transpile_options: TranspileOptions::default(),
      emit_options: EmitOptions::default(),
      relative_file_base: None,
    })
    .unwrap()
  }
}
