// Copyright 2018-2026 the Deno authors. MIT license.

mod cache_db;
mod caches;
mod check;
mod code_cache;
mod fast_check;
mod incremental;
mod module_info;

pub type EndeDir = ende_resolver::cache::EndeDir<CliSys>;
pub type EndeDirProvider = ende_resolver::cache::EndeDirProvider<CliSys>;

pub use cache_db::CacheDBHash;
pub use caches::Caches;
pub use check::TypeCheckCache;
pub use code_cache::CodeCache;
/// Permissions used to save a file in the disk caches.
pub use ende_cache_dir::CACHE_PERM;
pub use fast_check::FastCheckCache;
pub use incremental::IncrementalCache;
pub use module_info::ModuleInfoCache;

use crate::sys::CliSys;

pub type GlobalHttpCache = ende_cache_dir::GlobalHttpCache<CliSys>;
pub type LocalLspHttpCache = ende_cache_dir::LocalLspHttpCache<CliSys>;
pub use ende_cache_dir::HttpCache;
