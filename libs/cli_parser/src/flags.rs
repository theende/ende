// Copyright 2018-2026 the Deno authors. MIT license.
#![allow(dead_code, reason = "types are used by the ende CLI crate")]

// Flag types using real Deno dependencies. These are the canonical type
// definitions shared by both the parser crate and `cli/args/flags.rs`.

use std::net::SocketAddr;
use std::num::NonZeroU8;
use std::num::NonZeroU32;
use std::num::NonZeroUsize;
use std::path::PathBuf;

pub use ende_config::ende_json::NewestDependencyDate;
pub use ende_core::url::Url;
pub use ende_lib::args::CaData;
pub use ende_lib::args::UnstableConfig;
pub use ende_runtime::ende_inspector_server::InspectPublishUid;
pub use log::Level;
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ConfigFlag {
  #[default]
  Discover,
  Path(String),
  Disabled,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileFlags {
  pub ignore: Vec<String>,
  pub include: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddFlags {
  pub packages: Vec<String>,
  pub no_save: bool,
  pub lockfile_only: bool,
  pub save_exact: bool,
  pub unscoped: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WhyFlags {
  pub package: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoveFlags {
  pub packages: Vec<String>,
  pub lockfile_only: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LinkFlags {
  pub paths: Vec<String>,
  pub lockfile_only: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnlinkFlags {
  pub names_or_paths: Vec<String>,
  pub lockfile_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct VersionFlags {
  pub increment: Option<VersionIncrement>,
  /// Bump every package in the workspace. Defaults to true when invoked at the
  /// workspace root and the workspace has more than one member with a version.
  pub workspace: Option<bool>,
  /// When in workspace mode without an explicit increment, derive bumps from
  /// commit messages between `start` and `base`. The default for both is to
  /// fall back to git (latest tag and current branch respectively).
  pub start: Option<String>,
  pub base: Option<String>,
  /// Path to the import map to rewrite jsr: version constraints in. Defaults
  /// to the root ende.json (or whatever its `importMap` field points to).
  pub import_map: Option<String>,
  /// Path to the release notes markdown file to prepend in conventional-commits
  /// mode. Defaults to `Releases.md`.
  pub release_notes: Option<String>,
  /// Don't write any files; just print what would happen.
  pub dry_run: bool,
  /// Explicit path to the configuration file to bump. When set, single-file
  /// mode is forced (workspace auto-detection is bypassed).
  pub config: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VersionIncrement {
  Major,
  Minor,
  Patch,
  Premajor,
  Preminor,
  Prepatch,
  Prerelease,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BenchFlags {
  pub files: FileFlags,
  pub filter: Option<String>,
  pub json: bool,
  pub no_run: bool,
  pub permit_no_files: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheFlags {
  pub files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckFlags {
  pub files: Vec<String>,
  pub doc: bool,
  pub doc_only: bool,
  pub check_js: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JavaScriptEngine {
  #[default]
  V8,
  QuickJs,
}

impl JavaScriptEngine {
  pub const fn artifact_suffix(&self) -> &'static str {
    match self {
      Self::V8 => "",
      Self::QuickJs => "-quickjs",
    }
  }
}

impl std::str::FromStr for JavaScriptEngine {
  type Err = String;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    match value {
      "v8" => Ok(Self::V8),
      "quickjs" => Ok(Self::QuickJs),
      _ => Err(format!("unsupported JavaScript engine: {value}")),
    }
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileFlags {
  pub source_file: String,
  pub output: Option<String>,
  pub args: Vec<String>,
  pub target: Option<String>,
  pub no_terminal: bool,
  pub icon: Option<String>,
  pub include: Vec<String>,
  pub exclude: Vec<String>,
  pub eszip: bool,
  pub self_extracting: bool,
  /// Stable identity for the compiled app. Determines where origin-bound
  /// storage (default `Ende.openKv()`, `localStorage`, `caches`) is persisted.
  /// Defaults to the output file name when not set.
  pub app_name: Option<String>,
  /// JS engine the compiled binary should run on. Named `engine` because
  /// `ende desktop --backend` already means the render backend.
  pub engine: JavaScriptEngine,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IconSetEntry {
  pub path: String,
  pub size: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IconConfig {
  /// A single icon file (`.icns`, `.ico`, or `.png`).
  Single(String),
  /// Multiple PNGs at specific pixel sizes.
  Set(Vec<IconSetEntry>),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DesktopFlags {
  pub source_file: String,
  pub output: Option<String>,
  pub args: Vec<String>,
  pub target: Option<String>,
  pub icon: Option<IconConfig>,
  pub include: Vec<String>,
  pub exclude: Vec<String>,
  pub hmr: bool,
  pub backend: Option<String>,
  /// JS engine the desktop binary runs on. Distinct from `backend`, which
  /// selects the render backend (webview/cef/raw).
  pub engine: JavaScriptEngine,
  pub all_targets: bool,
  /// Reverse-DNS bundle / application identifier (e.g. `com.acme.foo`).
  /// Used for the macOS `CFBundleIdentifier`, the Linux `.desktop` file
  /// identifier, and (eventually) the Windows AppUserModelID. When unset
  /// a synthetic `sh.ende.desktop.<app-slug>` is generated.
  pub identifier: Option<String>,
  /// Custom URL schemes (deep links) to register with the OS, e.g. `["acme"]`
  /// for `acme://...` links. Sourced from `desktop.app.deepLinks` in
  /// `ende.json`. Each entry is a bare scheme with no `://`.
  pub deep_links: Vec<String>,
  /// macOS codesigning identity (e.g. `Developer ID Application: Acme,
  /// Inc. (TEAMID)`, or `-` for ad-hoc). When unset the bundle is left
  /// unsigned; the system will quarantine it on download.
  pub codesign_identity: Option<String>,
  /// Optional override for the CEF renderer debugger port. When unset, a free
  /// port is allocated. The user-visible inspector port (from `--inspect`) is
  /// separate and is carried on `Flags::inspect`.
  pub inspect_renderer: Option<SocketAddr>,
  /// When set, emit a compressed distribution archive of the packaged app
  /// next to it (`<output>.tar.xz` or `<output>.tar.zst`). The installed
  /// `.app`/app dir is left untouched (and still code-signed); the archive is
  /// just a small download artifact. Value is the compressor: `"xz"` (LZMA)
  /// or `"zstd"`.
  pub compress: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionsFlags {
  /// Pre-rendered static completion script.
  Static(Box<[u8]>),
  /// Dynamic completions for the given shell. The actual generation is
  /// performed on the CLI side (it needs the runtime's completion handler),
  /// so this variant only records the target shell.
  Dynamic { shell: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum CoverageType {
  #[default]
  Summary,
  Detailed,
  Lcov,
  Html,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct CoverageFlags {
  pub files: FileFlags,
  pub output: Option<String>,
  pub include: Vec<String>,
  pub exclude: Vec<String>,
  pub r#type: CoverageType,
  /// Minimum coverage percentage (0-100) applied to line, branch, and function
  /// coverage. Overrides per-metric thresholds from `ende.json`.
  pub threshold: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct DeployFlags {
  pub sandbox: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum DocSourceFileFlag {
  #[default]
  Builtin,
  Paths(Vec<String>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocHtmlFlag {
  pub name: Option<String>,
  pub category_docs_path: Option<String>,
  pub symbol_redirect_map_path: Option<String>,
  pub default_symbol_map_path: Option<String>,
  pub strip_trailing_html: bool,
  pub output: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocFlags {
  pub private: bool,
  pub json: bool,
  pub lint: bool,
  pub html: Option<DocHtmlFlag>,
  pub source_files: DocSourceFileFlag,
  pub filter: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct CpuProfFlags {
  pub dir: Option<String>,
  pub name: Option<String>,
  pub interval: Option<u32>,
  pub md: bool,
  pub flamegraph: bool,
}

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct EvalFlags {
  pub print: bool,
  pub code: String,
}

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct FmtFlags {
  pub check: bool,
  pub fail_fast: bool,
  pub files: FileFlags,
  pub permit_no_files: bool,
  pub use_tabs: Option<bool>,
  pub line_width: Option<NonZeroU32>,
  pub indent_width: Option<NonZeroU8>,
  pub single_quote: Option<bool>,
  pub prose_wrap: Option<String>,
  pub no_semicolons: Option<bool>,
  pub no_editorconfig: bool,
  pub unstable_component: bool,
  pub unstable_sql: bool,
}

impl FmtFlags {
  pub fn is_stdin(&self) -> bool {
    let args = &self.files.include;
    args.len() == 1 && args[0] == "-"
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitFlags {
  pub package: Option<String>,
  pub package_args: Vec<String>,
  pub dir: Option<String>,
  pub lib: bool,
  pub serve: bool,
  pub empty: bool,
  pub yes: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InfoFlags {
  pub json: bool,
  pub file: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ListFlags {
  pub recursive: bool,
  pub depth: u16,
  pub filters: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallFlagsGlobal {
  pub module_urls: Vec<String>,
  pub args: Vec<String>,
  pub name: Option<String>,
  pub root: Option<String>,
  pub force: bool,
  pub compile: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallFlags {
  Local(InstallFlagsLocal),
  Global(InstallFlagsGlobal),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstallFlagsLocal {
  Add(AddFlags),
  TopLevel(InstallTopLevelFlags),
  Entrypoints(InstallEntrypointsFlags),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CiFlags {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallTopLevelFlags {
  pub lockfile_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallEntrypointsFlags {
  pub entrypoints: Vec<String>,
  pub lockfile_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JSONReferenceFlags {
  pub json: ende_core::serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JupyterFlags {
  pub install: bool,
  pub name: Option<String>,
  pub display: Option<String>,
  pub kernel: bool,
  pub conn_file: Option<String>,
  pub force: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallFlagsGlobal {
  pub packages: Vec<String>,
  pub root: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UninstallKind {
  Local(RemoveFlags),
  Global(UninstallFlagsGlobal),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallFlags {
  pub kind: UninstallKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LintFlags {
  pub files: FileFlags,
  pub rules: bool,
  pub fix: bool,
  pub maybe_rules_tags: Option<Vec<String>>,
  pub maybe_rules_include: Option<Vec<String>>,
  pub maybe_rules_exclude: Option<Vec<String>>,
  pub permit_no_files: bool,
  pub json: bool,
  pub compact: bool,
}

impl LintFlags {
  pub fn is_stdin(&self) -> bool {
    let args = &self.files.include;
    args.len() == 1 && args[0] == "-"
  }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ReplFlags {
  pub eval_files: Option<Vec<String>>,
  pub eval: Option<String>,
  pub is_default_command: bool,
  pub json: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct RunFlags {
  pub script: String,
  pub bare: bool,
  pub coverage_dir: Option<String>,
  pub print_task_list: bool,
}

impl RunFlags {
  pub fn new_default(script: String) -> Self {
    Self {
      script,
      bare: false,
      coverage_dir: None,
      print_task_list: false,
    }
  }

  pub fn is_stdin(&self) -> bool {
    self.script == "-"
  }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum EndeXShimName {
  #[default]
  Dx,
  Endex,
  Dnx,
  Other(String),
}

impl EndeXShimName {
  pub fn name(&self) -> &str {
    match self {
      Self::Dx => "dx",
      Self::Endex => "endex",
      Self::Dnx => "dnx",
      Self::Other(name) => name,
    }
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum XFlagsKind {
  InstallAlias(EndeXShimName),
  Command(XCommandFlags),
  Print,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XCommandFlags {
  pub yes: bool,
  pub command: String,
  /// When set via `--package`/`-p`, specifies the package to install
  /// while `command` is the binary name to execute from that package.
  pub package: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XFlags {
  pub kind: XFlagsKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServeFlags {
  pub script: String,
  pub port: u16,
  pub host: String,
  pub parallel: bool,
  pub open_site: bool,
}

impl ServeFlags {
  pub fn new_default(script: String, port: u16, host: &str) -> Self {
    Self {
      script,
      port,
      host: host.to_owned(),
      parallel: false,
      open_site: false,
    }
  }
}

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct WatchFlagsWithPaths {
  pub hmr: bool,
  pub paths: Vec<String>,
  pub no_clear_screen: bool,
  pub exclude: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskFlags {
  pub cwd: Option<String>,
  pub task: Option<String>,
  pub is_run: bool,
  pub recursive: bool,
  /// Run the task in all workspace members, but not in the workspace root.
  pub members: bool,
  pub filter: Option<String>,
  pub eval: bool,
  pub no_prefix: bool,
  /// Maximum number of workspace tasks to run concurrently. Overrides the
  /// `ENDE_JOBS` env var and the `available_parallelism()` default. Only
  /// meaningful for multi-task (`-r`/`--filter`) runs.
  pub concurrency: Option<NonZeroUsize>,
  /// Exit with code 0 instead of an error when the named task is not found.
  pub if_present: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TestReporterConfig {
  #[default]
  Pretty,
  Dot,
  Junit,
  Tap,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestFlags {
  pub doc: bool,
  pub no_run: bool,
  pub coverage_dir: Option<String>,
  pub coverage_raw_data_only: bool,
  /// Minimum coverage percentage (0-100) required when `--coverage` is set.
  /// Overrides per-metric thresholds from `ende.json`.
  pub coverage_threshold: Option<u32>,
  pub clean: bool,
  pub fail_fast: Option<NonZeroUsize>,
  pub files: FileFlags,
  pub parallel: bool,
  pub permit_no_files: bool,
  pub filter: Option<String>,
  pub shuffle: Option<u64>,
  pub retry: u32,
  pub repeats: u32,
  /// Run only a subset of test files, as `(index, count)` with a 1-based
  /// index. Used to split a run across machines, e.g. `--shard=2/3`.
  pub shard: Option<(usize, usize)>,
  pub trace_leaks: bool,
  pub sanitize_ops: bool,
  pub sanitize_resources: bool,
  pub reporter: TestReporterConfig,
  pub junit_path: Option<String>,
  pub hide_stacktraces: bool,
  /// Run only test modules affected by files changed in git.
  /// `None` when `--changed` is absent, `Some(None)` for `--changed` with no
  /// value (uncommitted changes), `Some(Some(ref))` for `--changed=<ref>`.
  pub changed: Option<Option<String>>,
  /// Run only test modules that depend on the given source files (`--related`).
  pub related: Vec<String>,
  pub update_snapshots: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub enum SourceMapMode {
  #[default]
  None,
  Inline,
  Separate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranspileFlags {
  pub files: Vec<String>,
  pub output: Option<String>,
  pub output_dir: Option<String>,
  pub declaration: bool,
  pub source_map: SourceMapMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpgradeFlags {
  pub dry_run: bool,
  pub force: bool,
  pub release_candidate: bool,
  pub canary: bool,
  pub no_delta: bool,
  pub version: Option<String>,
  pub output: Option<String>,
  pub version_or_hash_or_channel: Option<String>,
  pub checksum: Option<String>,
  pub pr: Option<u64>,
  pub branch: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishFlags {
  pub token: Option<String>,
  pub dry_run: bool,
  pub allow_slow_types: bool,
  pub allow_dirty: bool,
  pub no_provenance: bool,
  pub set_version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HelpFlags {
  pub help: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanFlags {
  pub except_paths: Vec<String>,
  pub dry_run: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndeSubcommand {
  Add(AddFlags),
  Remove(RemoveFlags),
  Bench(BenchFlags),
  Cache(CacheFlags),
  Check(CheckFlags),
  Ci(CiFlags),
  Clean(CleanFlags),
  Compile(CompileFlags),
  Completions(CompletionsFlags),
  Desktop(DesktopFlags),
  Coverage(CoverageFlags),
  Deploy(DeployFlags),
  Doc(DocFlags),
  Eval(EvalFlags),
  Fmt(FmtFlags),
  Init(InitFlags),
  Info(InfoFlags),
  List(ListFlags),
  Install(InstallFlags),
  JSONReference(JSONReferenceFlags),
  Jupyter(JupyterFlags),
  Uninstall(UninstallFlags),
  Link(LinkFlags),
  Unlink(UnlinkFlags),
  Lsp,
  Lint(LintFlags),
  Repl(ReplFlags),
  Run(RunFlags),
  Serve(ServeFlags),
  Task(TaskFlags),
  Test(TestFlags),
  Transpile(TranspileFlags),
  Outdated(OutdatedFlags),
  Types,
  Upgrade(UpgradeFlags),
  Vendor,
  Why(WhyFlags),
  BumpVersion(VersionFlags),
  Publish(PublishFlags),
  Help(HelpFlags),
  X(XFlags),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutdatedKind {
  Update {
    latest: bool,
    interactive: bool,
    lockfile_only: bool,
  },
  PrintOutdated {
    compatible: bool,
  },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutdatedFlags {
  pub filters: Vec<String>,
  pub recursive: bool,
  pub kind: OutdatedKind,
}

impl EndeSubcommand {
  pub fn is_run(&self) -> bool {
    matches!(self, Self::Run(_))
  }

  // Returns `true` if the subcommand depends on testing infrastructure.
  pub fn needs_test(&self) -> bool {
    matches!(
      self,
      Self::Test(_)
        | Self::Jupyter(_)
        | Self::Repl(_)
        | Self::Bench(_)
        | Self::Lint(_)
        | Self::Lsp
    )
  }
}

impl Default for EndeSubcommand {
  fn default() -> EndeSubcommand {
    EndeSubcommand::Repl(ReplFlags {
      eval_files: None,
      eval: None,
      is_default_command: true,
      json: false,
    })
  }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum TypeCheckMode {
  /// Type-check all modules.
  All,
  /// Skip type-checking of all modules. The default value for "ende run" and
  /// several other subcommands.
  #[default]
  None,
  /// Only type-check local modules. The default value for "ende test" and
  /// several other subcommands.
  Local,
}

impl TypeCheckMode {
  /// Gets if type checking will occur under this mode.
  pub fn is_true(&self) -> bool {
    match self {
      Self::None => false,
      Self::Local | Self::All => true,
    }
  }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct InternalFlags {
  /// Used when the language server is configured with an
  /// explicit cache option.
  pub cache_path: Option<PathBuf>,
  /// Only reads to the lockfile instead of writing to it.
  pub lockfile_skip_write: bool,
  /// Set when running the desktop subcommand to use desktop type libs.
  pub is_desktop: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Flags {
  pub initial_cwd: Option<PathBuf>,
  /// Vector of CLI arguments - these are user script arguments, all Ende
  /// specific flags are removed.
  pub argv: Vec<String>,
  pub subcommand: EndeSubcommand,

  pub frozen_lockfile: Option<bool>,
  pub ca_stores: Option<Vec<String>>,
  pub ca_data: Option<CaData>,
  pub cache_blocklist: Vec<String>,
  pub cached_only: bool,
  pub type_check_mode: TypeCheckMode,
  pub config_flag: ConfigFlag,
  pub vendor: Option<bool>,
  pub enable_testing_features: bool,
  pub ext: Option<String>,
  /// Flags that aren't exposed in the CLI, but are used internally.
  pub internal: InternalFlags,
  pub ignore: Vec<String>,
  pub import_map_path: Option<String>,
  pub env_file: Option<Vec<String>>,
  pub inspect_brk: Option<SocketAddr>,
  pub inspect_wait: Option<SocketAddr>,
  pub inspect: Option<SocketAddr>,
  pub inspect_publish_uid: Option<InspectPublishUid>,
  pub location: Option<Url>,
  pub lock: Option<String>,
  pub log_level: Option<Level>,
  pub minimum_dependency_age: Option<NewestDependencyDate>,
  pub no_remote: bool,
  pub no_lock: bool,
  pub reload: bool,
  pub seed: Option<u64>,
  pub trace_ops: Option<Vec<String>>,
  pub unstable_config: UnstableConfig,
  pub unsafely_ignore_certificate_errors: Option<Vec<String>>,
  pub v8_flags: Vec<String>,
  pub code_cache_enabled: bool,
  pub permissions: PermissionFlags,
  pub permission_set: Option<String>,
  pub eszip: bool,
  pub preload: Vec<String>,
  pub tunnel: bool,
  pub cpu_prof: Option<CpuProfFlags>,
  /// Watch configuration for subcommands that support `--watch`. `paths` is
  /// only populated for subcommands whose `--watch` accepts values.
  pub watch: Option<WatchFlagsWithPaths>,
}

#[derive(Clone, Debug, Eq, PartialEq, Default, Serialize, Deserialize)]
pub struct PermissionFlags {
  pub allow_all: bool,
  pub allow_env: Option<Vec<String>>,
  pub deny_env: Option<Vec<String>>,
  pub ignore_env: Option<Vec<String>>,
  pub allow_ffi: Option<Vec<String>>,
  pub deny_ffi: Option<Vec<String>>,
  pub allow_net: Option<Vec<String>>,
  pub deny_net: Option<Vec<String>>,
  pub allow_read: Option<Vec<String>>,
  pub deny_read: Option<Vec<String>>,
  pub ignore_read: Option<Vec<String>>,
  pub allow_run: Option<Vec<String>>,
  pub deny_run: Option<Vec<String>>,
  pub allow_sys: Option<Vec<String>>,
  pub deny_sys: Option<Vec<String>>,
  pub allow_write: Option<Vec<String>>,
  pub deny_write: Option<Vec<String>>,
  pub no_prompt: bool,
  pub allow_import: Option<Vec<String>>,
  pub deny_import: Option<Vec<String>>,
}

impl PermissionFlags {
  pub fn has_permission(&self) -> bool {
    self.allow_all
      || self.allow_env.is_some()
      || self.deny_env.is_some()
      || self.ignore_env.is_some()
      || self.allow_ffi.is_some()
      || self.deny_ffi.is_some()
      || self.allow_net.is_some()
      || self.deny_net.is_some()
      || self.allow_read.is_some()
      || self.deny_read.is_some()
      || self.ignore_read.is_some()
      || self.allow_run.is_some()
      || self.deny_run.is_some()
      || self.allow_sys.is_some()
      || self.deny_sys.is_some()
      || self.allow_write.is_some()
      || self.deny_write.is_some()
      || self.allow_import.is_some()
      || self.deny_import.is_some()
  }
}
impl Flags {
  pub fn has_permission(&self) -> bool {
    self.permissions.has_permission()
  }

  pub fn has_permission_in_argv(&self) -> bool {
    self.argv.iter().any(|arg| {
      arg == "--allow-all"
        || arg.starts_with("--allow-env")
        || arg.starts_with("--deny-env")
        || arg.starts_with("--allow-ffi")
        || arg.starts_with("--deny-ffi")
        || arg.starts_with("--allow-net")
        || arg.starts_with("--deny-net")
        || arg.starts_with("--allow-read")
        || arg.starts_with("--deny-read")
        || arg.starts_with("--allow-run")
        || arg.starts_with("--deny-run")
        || arg.starts_with("--allow-sys")
        || arg.starts_with("--deny-sys")
        || arg.starts_with("--allow-write")
        || arg.starts_with("--deny-write")
    })
  }
}
