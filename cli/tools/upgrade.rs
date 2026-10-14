// Copyright 2018-2026 the Deno authors. MIT license.

//! This module provides feature to upgrade ende executable

use std::borrow::Cow;
use std::env;
use std::fs;
use std::io::IsTerminal;
use std::ops::Sub;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use deno_semver::Version;
use ende_core::anyhow::Context;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::unsync::spawn;
use ende_core::url::Url;
use ende_lib::version;
use once_cell::sync::Lazy;
use sha2::Digest;

use crate::args::Flags;
use crate::args::UPGRADE_USAGE;
use crate::args::UpgradeFlags;
use crate::colors;
use crate::factory::CliFactory;
use crate::http_util::HttpClient;
use crate::http_util::HttpClientProvider;
use crate::util::archive;
use crate::util::progress_bar::ProgressBar;
use crate::util::progress_bar::ProgressBarStyle;

static RELEASE_URL: &str = "https://github.com/theende/ende/releases";
static DL_RELEASE_URL: &str = "https://dl.ende.sh/release";

pub static ARCHIVE_NAME: Lazy<String> =
  Lazy::new(|| format!("ende-{}.zip", env!("TARGET")));

static DELTA_TARGET_NAME: Lazy<String> =
  Lazy::new(|| format!("ende-{}", env!("TARGET")));

// How often query server for new version. In hours.
const UPGRADE_CHECK_INTERVAL: i64 = 24;

const UPGRADE_CHECK_FETCH_DELAY: Duration = Duration::from_millis(500);

/// Environment necessary for doing the update checker.
/// An alternate trait implementation can be provided for testing purposes.
trait UpdateCheckerEnvironment: Clone {
  fn read_check_file(&self) -> String;
  fn write_check_file(&self, text: &str);
  fn current_time(&self) -> chrono::DateTime<chrono::Utc>;
}

#[derive(Clone)]
struct RealUpdateCheckerEnvironment {
  cache_file_path: PathBuf,
  current_time: chrono::DateTime<chrono::Utc>,
}

impl RealUpdateCheckerEnvironment {
  pub fn new(cache_file_path: PathBuf) -> Self {
    Self {
      cache_file_path,
      // cache the current time
      current_time: chrono::Utc::now(),
    }
  }
}

impl UpdateCheckerEnvironment for RealUpdateCheckerEnvironment {
  fn read_check_file(&self) -> String {
    std::fs::read_to_string(&self.cache_file_path).unwrap_or_default()
  }

  fn write_check_file(&self, text: &str) {
    let _ = std::fs::write(&self.cache_file_path, text);
  }

  fn current_time(&self) -> chrono::DateTime<chrono::Utc> {
    self.current_time
  }
}

#[derive(Debug, Copy, Clone)]
enum UpgradeCheckKind {
  Execution,
  Lsp,
}

#[async_trait(?Send)]
trait VersionProvider: Clone {
  /// Fetch the latest available version.
  async fn latest_version(&self) -> Result<String, AnyError>;

  /// The version of the current executable.
  fn current_version(&self) -> Cow<'_, str>;
}

#[derive(Clone)]
struct RealVersionProvider {
  http_client_provider: Arc<HttpClientProvider>,
  check_kind: UpgradeCheckKind,
}

impl RealVersionProvider {
  pub fn new(
    http_client_provider: Arc<HttpClientProvider>,
    check_kind: UpgradeCheckKind,
  ) -> Self {
    Self {
      http_client_provider,
      check_kind,
    }
  }
}

#[async_trait(?Send)]
impl VersionProvider for RealVersionProvider {
  async fn latest_version(&self) -> Result<String, AnyError> {
    fetch_latest_version(
      &self.http_client_provider.get_or_create()?,
      self.check_kind,
    )
    .await
  }

  fn current_version(&self) -> Cow<'_, str> {
    Cow::Borrowed(version::ENDE_VERSION_INFO.ende)
  }
}

struct UpdateChecker<
  TEnvironment: UpdateCheckerEnvironment,
  TVersionProvider: VersionProvider,
> {
  env: TEnvironment,
  version_provider: TVersionProvider,
  maybe_file: Option<CheckVersionFile>,
}

impl<TEnvironment: UpdateCheckerEnvironment, TVersionProvider: VersionProvider>
  UpdateChecker<TEnvironment, TVersionProvider>
{
  pub fn new(env: TEnvironment, version_provider: TVersionProvider) -> Self {
    let maybe_file = CheckVersionFile::parse(env.read_check_file());
    Self {
      env,
      version_provider,
      maybe_file,
    }
  }

  pub fn should_check_for_new_version(&self) -> bool {
    let Some(file) = &self.maybe_file else {
      return true;
    };

    let last_check_age = self
      .env
      .current_time()
      .signed_duration_since(file.last_checked);
    last_check_age > chrono::Duration::hours(UPGRADE_CHECK_INTERVAL)
  }

  /// Returns a version if a new one is available and it should be prompted about.
  pub fn should_prompt(&self) -> Option<String> {
    let file = self.maybe_file.as_ref()?;
    // If the current version saved is not the actually current version of the binary
    // It means
    // - We already check for a new version today
    // - The user have probably upgraded today
    // So we should not prompt and wait for tomorrow for the latest version to be updated again
    let current_version = self.version_provider.current_version();
    if file.current_version != current_version {
      return None;
    }
    if file.latest_version == current_version {
      return None;
    }

    if let Ok(current) = Version::parse_standard(&current_version)
      && let Ok(latest) = Version::parse_standard(&file.latest_version)
      && current >= latest
    {
      return None;
    }

    let last_prompt_age = self
      .env
      .current_time()
      .signed_duration_since(file.last_prompt);
    if last_prompt_age > chrono::Duration::hours(UPGRADE_CHECK_INTERVAL) {
      Some(file.latest_version.clone())
    } else {
      None
    }
  }

  /// Store that we showed the update message to the user.
  pub fn store_prompted(self) {
    if let Some(file) = self.maybe_file {
      self.env.write_check_file(
        &file.with_last_prompt(self.env.current_time()).serialize(),
      );
    }
  }
}

fn get_release_notes_url(version: &str) -> String {
  format!("{RELEASE_URL}/tag/v{version}")
}

fn get_minor_version_blog_post_url(semver: &Version) -> String {
  format!("https://ende.sh/blog/v{}.{}", semver.major, semver.minor)
}

fn print_release_notes(current_version: &str, new_version: &str) {
  let Ok(current_semver) = Version::parse_standard(current_version) else {
    return;
  };
  let Ok(new_semver) = Version::parse_standard(new_version) else {
    return;
  };

  let is_switching_from_deno1_to_deno2 =
    new_semver.major == 2 && current_semver.major == 1;

  if is_switching_from_deno1_to_deno2 {
    log::info!(
      "{}\n\n  {}\n",
      colors::gray("Migration guide:"),
      colors::bold(
        "https://docs.ende.sh/runtime/manual/advanced/migrate_deprecations"
      )
    );
  }

  let should_print = current_semver.major != new_semver.major
    || current_semver.minor != new_semver.minor;

  if !should_print {
    return;
  }

  log::info!(
    "{}\n\n  {}\n",
    colors::gray("Release notes:"),
    colors::bold(get_release_notes_url(new_version))
  );
  log::info!(
    "{}\n\n  {}\n",
    colors::gray("Blog post:"),
    colors::bold(get_minor_version_blog_post_url(&new_semver))
  );
}

pub fn upgrade_check_enabled() -> bool {
  matches!(
    ende_env::var("ENDE_NO_UPDATE_CHECK"),
    Err(env::VarError::NotPresent)
  )
}

pub fn check_for_upgrades(
  http_client_provider: Arc<HttpClientProvider>,
  cache_file_path: PathBuf,
) {
  if !upgrade_check_enabled() {
    return;
  }

  let env = RealUpdateCheckerEnvironment::new(cache_file_path);
  let version_provider = RealVersionProvider::new(
    http_client_provider.clone(),
    UpgradeCheckKind::Execution,
  );
  let update_checker = UpdateChecker::new(env, version_provider);

  if update_checker.should_check_for_new_version() {
    let env = update_checker.env.clone();
    let version_provider = update_checker.version_provider.clone();
    // do this asynchronously on a separate task
    spawn(async move {
      // Sleep for a small amount of time to not unnecessarily impact startup
      // time.
      tokio::time::sleep(UPGRADE_CHECK_FETCH_DELAY).await;

      fetch_and_store_latest_version(&env, &version_provider).await;

      // text is used by the test suite
      log::debug!("Finished upgrade checker.")
    });
  }

  // Don't bother doing any more computation if we're not in TTY environment.
  let should_prompt =
    log::log_enabled!(log::Level::Info) && std::io::stderr().is_terminal();

  if !should_prompt {
    return;
  }

  // Print a message if an update is available
  if let Some(upgrade_version) = update_checker.should_prompt() {
    log::info!(
      "{} {} → {} {}",
      colors::green("A new release of Ende is available:"),
      colors::cyan(version::ENDE_VERSION_INFO.ende),
      colors::cyan(&upgrade_version),
      colors::italic_gray("Run `ende upgrade` to install it.")
    );

    update_checker.store_prompted();
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspVersionUpgradeInfo {
  pub latest_version: String,
}

pub async fn check_for_upgrades_for_lsp(
  http_client_provider: Arc<HttpClientProvider>,
) -> Result<Option<LspVersionUpgradeInfo>, AnyError> {
  if !upgrade_check_enabled() {
    return Ok(None);
  }

  let version_provider =
    RealVersionProvider::new(http_client_provider, UpgradeCheckKind::Lsp);
  check_for_upgrades_for_lsp_with_provider(&version_provider).await
}

async fn check_for_upgrades_for_lsp_with_provider(
  version_provider: &impl VersionProvider,
) -> Result<Option<LspVersionUpgradeInfo>, AnyError> {
  let latest_version = version_provider.latest_version().await?;
  let current_version = version_provider.current_version();

  // Nothing to upgrade
  if current_version == latest_version {
    return Ok(None);
  }

  if let Ok(current) = Version::parse_standard(&current_version)
    && let Ok(latest) = Version::parse_standard(&latest_version)
    && current >= latest
  {
    return Ok(None); // nothing to upgrade
  }
  Ok(Some(LspVersionUpgradeInfo { latest_version }))
}

async fn fetch_and_store_latest_version<
  TEnvironment: UpdateCheckerEnvironment,
  TVersionProvider: VersionProvider,
>(
  env: &TEnvironment,
  version_provider: &TVersionProvider,
) {
  let Ok(latest_version) = version_provider.latest_version().await else {
    return;
  };

  let version_file = CheckVersionFile {
    // put a date in the past here so that prompt can be shown on next run
    last_prompt: env
      .current_time()
      .sub(chrono::Duration::hours(UPGRADE_CHECK_INTERVAL + 1)),
    last_checked: env.current_time(),
    current_version: version_provider.current_version().to_string(),
    latest_version,
  };

  env.write_check_file(&version_file.serialize());
}

fn get_binary_cache_path(dl_dir: &Path, version: &str) -> PathBuf {
  dl_dir.join(format!("release/v{}/{}", version, *ARCHIVE_NAME))
}

fn try_read_cached_binary(
  sys: &impl sys_traits::FsRead,
  cache_path: &Path,
) -> Option<Vec<u8>> {
  match sys.fs_read(cache_path) {
    Ok(data) => Some(data.into_owned()),
    Err(_) => None,
  }
}

fn store_cached_binary(
  sys: &impl deno_path_util::fs::AtomicWriteFileSys,
  cache_path: &Path,
  data: &[u8],
) {
  if let Err(err) =
    deno_path_util::fs::atomic_write_file(sys, cache_path, data, 0o644)
  {
    log::debug!("Failed to cache binary: {}", err);
  }
}

/// Get the artifact name for the current platform.
/// CI artifacts are named like `{profile}-{os}-{arch}-ende`.
fn get_pr_artifact_name() -> Result<String, AnyError> {
  let target = env!("TARGET");

  let (os, arch) = if target.contains("linux") && target.contains("x86_64") {
    ("linux", "x86_64")
  } else if target.contains("linux") && target.contains("aarch64") {
    ("linux", "aarch64")
  } else if target.contains("apple") && target.contains("x86_64") {
    ("macos", "x86_64")
  } else if target.contains("apple") && target.contains("aarch64") {
    ("macos", "aarch64")
  } else if target.contains("windows") && target.contains("x86_64") {
    ("windows", "x86_64")
  } else if target.contains("windows") && target.contains("aarch64") {
    ("windows", "aarch64")
  } else {
    bail!("Unsupported platform for PR builds: {}", target)
  };

  // Prefer release builds, fall back to debug
  Ok(format!("release-{os}-{arch}-ende"))
}

fn get_pr_debug_artifact_name() -> Result<String, AnyError> {
  let release_name = get_pr_artifact_name()?;
  Ok(release_name.replacen("release-", "debug-", 1))
}

fn upgrade_from_pr(
  pr_number: u64,
  upgrade_flags: &UpgradeFlags,
) -> Result<(), AnyError> {
  // Check that `gh` CLI is available
  let gh_version = Command::new("gh").arg("--version").output();
  if gh_version.is_err() {
    bail!(
      "The `gh` CLI is required for installing from a PR.\n\
       Install it from https://cli.github.com/ and run `gh auth login`."
    );
  }

  log::info!("{}", colors::gray(format!("Looking up PR #{pr_number}...")));

  // Verify the PR exists and get its title/state/branch
  let pr_info = Command::new("gh")
    .args([
      "pr",
      "view",
      &pr_number.to_string(),
      "--repo",
      "theende/ende",
      "--json",
      "title,state,headRefName,headRefOid",
      "-q",
      r#"[.title, .state, .headRefName, .headRefOid] | @tsv"#,
    ])
    .output()
    .context("failed to run `gh pr view`")?;

  if !pr_info.status.success() {
    let stderr = String::from_utf8_lossy(&pr_info.stderr);
    bail!("Failed to find PR #{pr_number}: {stderr}");
  }

  let pr_info_str = String::from_utf8_lossy(&pr_info.stdout);
  let pr_fields: Vec<&str> = pr_info_str.trim().splitn(4, '\t').collect();
  let pr_title = pr_fields.first().unwrap_or(&"unknown");
  let pr_state = pr_fields.get(1).unwrap_or(&"unknown");
  let pr_branch = pr_fields.get(2).unwrap_or(&"");
  let pr_head_sha = pr_fields.get(3).unwrap_or(&"");

  log::info!(
    "PR #{}: {} ({})",
    pr_number,
    colors::bold(pr_title),
    pr_state
  );

  let artifact_name = get_pr_artifact_name()?;
  let debug_artifact_name = get_pr_debug_artifact_name()?;

  // Find CI runs for this PR by branch name
  log::info!("{}", colors::gray("Finding CI artifacts..."));

  let mut all_run_ids = Vec::new();

  if !pr_branch.is_empty() {
    // Filter by headSha to ensure we only get runs for the PR's current commit
    let jq_filter = if pr_head_sha.is_empty() {
      ".[].databaseId".to_string()
    } else {
      format!(
        r#"[.[] | select(.headSha == "{}")] | .[].databaseId"#,
        pr_head_sha
      )
    };
    let branch_runs = Command::new("gh")
      .args([
        "run",
        "list",
        "--repo",
        "theende/ende",
        "--branch",
        pr_branch,
        "--workflow",
        "ci",
        "--limit",
        "5",
        "--json",
        "databaseId,headSha",
        "-q",
        &jq_filter,
      ])
      .output()
      .context("failed to query CI runs by branch")?;

    if branch_runs.status.success() {
      let ids = String::from_utf8_lossy(&branch_runs.stdout);
      for id in ids.trim().lines() {
        if !id.is_empty() {
          all_run_ids.push(id.to_string());
        }
      }
    }
  }

  if all_run_ids.is_empty() {
    bail!(
      "No CI runs found for PR #{pr_number}. \
       The PR may not have been pushed yet, CI hasn't started, \
       or CI hasn't run on the latest commit yet."
    );
  }

  // Try each run to find one with our artifact
  let temp_dir =
    tempfile::TempDir::new().context("failed to create temporary directory")?;
  let download_dir = temp_dir.path();

  let mut downloaded = false;
  for run_id in &all_run_ids {
    // Try release build first, then debug
    for name in [&artifact_name, &debug_artifact_name] {
      log::info!(
        "{}",
        colors::gray(format!("Trying run {run_id}, artifact \"{name}\"..."))
      );

      let dl_result = Command::new("gh")
        .args([
          "run",
          "download",
          run_id,
          "--repo",
          "theende/ende",
          "--name",
          name,
          "--dir",
          &download_dir.to_string_lossy(),
        ])
        .output()
        .context("failed to run `gh run download`")?;

      if dl_result.status.success() {
        log::info!(
          "Downloaded artifact \"{}\" from run {}",
          colors::green(name),
          run_id
        );
        downloaded = true;
        break;
      }
    }
    if downloaded {
      break;
    }
  }

  if !downloaded {
    bail!(
      "Could not find a \"{}\" artifact for PR #{pr_number}.\n\
       Available artifacts may have expired or CI may not have completed.\n\
       Only release builds on linux-x86_64 and debug builds are typically available for PRs.",
      artifact_name
    );
  }

  // Find the downloaded binary
  let exe_name = if cfg!(windows) { "ende.exe" } else { "ende" };
  let new_exe_path = download_dir.join(exe_name);

  if !new_exe_path.exists() {
    bail!(
      "Downloaded artifact does not contain '{}'. Contents: {:?}",
      exe_name,
      fs::read_dir(download_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>()
    );
  }

  // Set executable permissions
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&new_exe_path, std::fs::Permissions::from_mode(0o755))?;
  }

  // Verify the binary works
  check_exe(&new_exe_path)?;

  if upgrade_flags.dry_run {
    log::info!("Upgraded successfully (dry run)");
    drop(temp_dir);
    return Ok(());
  }

  let current_exe_path = std::env::current_exe()
    .context("failed to get the path of the current executable")?;
  let output_exe_path = if let Some(output) = &upgrade_flags.output {
    Cow::Owned(PathBuf::from(output))
  } else {
    Cow::Borrowed(&current_exe_path)
  };

  #[cfg(windows)]
  kill_running_ende_lsp_processes();

  let output_result = if *output_exe_path == current_exe_path {
    replace_exe(&new_exe_path, &output_exe_path)
  } else {
    fs::rename(&new_exe_path, &*output_exe_path)
      .or_else(|_| fs::copy(&new_exe_path, &*output_exe_path).map(|_| ()))
  };
  check_windows_access_denied_error(output_result, &output_exe_path)?;

  log::info!(
    "\nUpgraded successfully from PR #{} {}\n",
    colors::green(&pr_number.to_string()),
    colors::gray(&format!("({})", pr_title))
  );

  drop(temp_dir);
  Ok(())
}

fn upgrade_from_branch(
  branch: &str,
  upgrade_flags: &UpgradeFlags,
) -> Result<(), AnyError> {
  let gh_version = Command::new("gh").arg("--version").output();
  if gh_version.is_err() {
    bail!(
      "The `gh` CLI is required for installing from a branch.\n\
       Install it from https://cli.github.com/ and run `gh auth login`."
    );
  }

  log::info!(
    "{}",
    colors::gray(format!("Finding CI artifacts for branch '{branch}'..."))
  );

  let artifact_name = get_pr_artifact_name()?;
  let debug_artifact_name = get_pr_debug_artifact_name()?;

  let runs_output = Command::new("gh")
    .args([
      "run",
      "list",
      "--repo",
      "theende/ende",
      "--branch",
      branch,
      "--workflow",
      "ci",
      "--limit",
      "5",
      "--json",
      "databaseId",
      "-q",
      ".[].databaseId",
    ])
    .output()
    .context("failed to query CI runs")?;

  if !runs_output.status.success() {
    let stderr = String::from_utf8_lossy(&runs_output.stderr);
    bail!("Failed to find CI runs for branch '{branch}': {stderr}");
  }

  let run_ids: Vec<String> = String::from_utf8_lossy(&runs_output.stdout)
    .trim()
    .lines()
    .filter(|l| !l.is_empty())
    .map(|s| s.to_string())
    .collect();

  if run_ids.is_empty() {
    bail!(
      "No CI runs found for branch '{branch}'. \
       CI may not have run yet."
    );
  }

  let temp_dir =
    tempfile::TempDir::new().context("failed to create temporary directory")?;
  let download_dir = temp_dir.path();

  let mut downloaded = false;
  for run_id in &run_ids {
    for name in [&artifact_name, &debug_artifact_name] {
      log::info!(
        "{}",
        colors::gray(format!("Trying run {run_id}, artifact \"{name}\"..."))
      );

      let dl_result = Command::new("gh")
        .args([
          "run",
          "download",
          run_id,
          "--repo",
          "theende/ende",
          "--name",
          name,
          "--dir",
          &download_dir.to_string_lossy(),
        ])
        .output()
        .context("failed to run `gh run download`")?;

      if dl_result.status.success() {
        log::info!(
          "Downloaded artifact \"{}\" from run {}",
          colors::green(name),
          run_id
        );
        downloaded = true;
        break;
      }
    }
    if downloaded {
      break;
    }
  }

  if !downloaded {
    bail!(
      "Could not find a \"{artifact_name}\" artifact for branch '{branch}'.\n\
       Artifacts may have expired or CI may not have completed."
    );
  }

  let exe_name = if cfg!(windows) { "ende.exe" } else { "ende" };
  let new_exe_path = download_dir.join(exe_name);

  if !new_exe_path.exists() {
    bail!("Downloaded artifact does not contain '{exe_name}'.");
  }

  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&new_exe_path, std::fs::Permissions::from_mode(0o755))?;
  }

  check_exe(&new_exe_path)?;

  if upgrade_flags.dry_run {
    log::info!("Upgraded successfully (dry run)");
    drop(temp_dir);
    return Ok(());
  }

  let current_exe_path = std::env::current_exe()
    .context("failed to get the path of the current executable")?;
  let output_exe_path = if let Some(output) = &upgrade_flags.output {
    Cow::Owned(PathBuf::from(output))
  } else {
    Cow::Borrowed(&current_exe_path)
  };

  #[cfg(windows)]
  kill_running_ende_lsp_processes();

  let output_result = if *output_exe_path == current_exe_path {
    replace_exe(&new_exe_path, &output_exe_path)
  } else {
    fs::rename(&new_exe_path, &*output_exe_path)
      .or_else(|_| fs::copy(&new_exe_path, &*output_exe_path).map(|_| ()))
  };
  check_windows_access_denied_error(output_result, &output_exe_path)?;

  log::info!(
    "\nUpgraded successfully from branch '{}'\n",
    colors::green(branch),
  );

  drop(temp_dir);
  Ok(())
}

pub async fn upgrade(
  flags: Arc<Flags>,
  upgrade_flags: UpgradeFlags,
) -> Result<(), AnyError> {
  if let Some(pr_number) = upgrade_flags.pr {
    return upgrade_from_pr(pr_number, &upgrade_flags);
  }
  if let Some(ref branch) = upgrade_flags.branch {
    return upgrade_from_branch(branch, &upgrade_flags);
  }

  let factory = CliFactory::from_flags(flags);
  let cli_options = factory.cli_options()?;
  let http_client_provider = factory.http_client_provider();
  let client = http_client_provider.get_or_create()?;
  let current_exe_path = std::env::current_exe()
    .context("failed to get the path of the current executable")?;
  let full_path_output_flag = upgrade_flags
    .output
    .as_ref()
    .map(|output| cli_options.initial_cwd().join(output));
  let output_exe_path =
    full_path_output_flag.as_ref().unwrap_or(&current_exe_path);

  let permissions = set_exe_permissions(&current_exe_path, output_exe_path)?;

  let force_selection_of_new_version =
    upgrade_flags.force || full_path_output_flag.is_some();

  let requested_version = requested_version(&upgrade_flags)?;

  log::info!("Current Ende version: v{}", version::ENDE_VERSION_INFO.ende);

  let maybe_selected_version_to_upgrade = match requested_version {
    None => {
      find_latest_version_to_upgrade(
        http_client_provider.clone(),
        force_selection_of_new_version,
      )
      .await?
    }
    Some(version) => select_specific_version_for_upgrade(
      version,
      force_selection_of_new_version,
    ),
  };

  let Some(selected_version_to_upgrade) = maybe_selected_version_to_upgrade
  else {
    return Ok(());
  };

  let banner_handle = spawn_banner_task(
    &selected_version_to_upgrade,
    http_client_provider.get_or_create()?,
  );

  let dl_dir = factory.ende_dir()?.dl_folder_path();
  let cache_path = get_binary_cache_path(&dl_dir, &selected_version_to_upgrade);

  let sys = &sys_traits::impls::RealSys;

  // Try delta upgrade when not disabled
  let maybe_delta_binary = if !upgrade_flags.no_delta
    && try_read_cached_binary(sys, &cache_path).is_none()
  {
    try_delta_upgrade(
      &client,
      &current_exe_path,
      version::ENDE_VERSION_INFO.ende,
      &selected_version_to_upgrade,
    )
    .await
  } else {
    None
  };

  let temp_dir =
    tempfile::TempDir::new().context("failed to create temporary directory")?;

  let new_exe_path = if let Some(delta_binary) = maybe_delta_binary {
    // Delta upgrade succeeded -- write the patched binary directly
    log::info!(
      "{}",
      colors::gray(format!(
        "Ende is upgrading to version {}",
        &selected_version_to_upgrade
      ))
    );

    let exe_name = if cfg!(windows) { "ende.exe" } else { "ende" };
    let new_exe_path = temp_dir.path().join(exe_name);
    std::fs::write(&new_exe_path, &delta_binary).with_context(|| {
      format!(
        "failed to write patched binary to '{}'",
        new_exe_path.display()
      )
    })?;
    new_exe_path
  } else {
    // Full download path (existing behavior)
    let archive_data = if let Some(data) =
      try_read_cached_binary(sys, &cache_path)
    {
      log::info!(
        "{}",
        colors::gray(format!(
          "Using cached binary from {}",
          cache_path.display()
        ))
      );
      data
    } else {
      let download_url = get_download_url(&selected_version_to_upgrade)?;
      log::info!("{}", colors::gray(format!("Downloading {}", &download_url)));
      let Some(data) = download_package(&client, download_url).await? else {
        log::error!("Download could not be found, aborting");
        ende_runtime::exit(1)
      };

      store_cached_binary(sys, &cache_path, &data);

      data
    };

    // verify checksum if provided
    if let Some(expected_checksum) = &upgrade_flags.checksum {
      verify_checksum(&archive_data, expected_checksum)?;
    }

    log::info!(
      "{}",
      colors::gray(format!(
        "Ende is upgrading to version {}",
        &selected_version_to_upgrade
      ))
    );

    archive::unpack_into_dir(archive::UnpackArgs {
      exe_name: if cfg!(windows) { "ende.exe" } else { "ende" },
      archive_name: &ARCHIVE_NAME,
      archive_data: &archive_data,
      dest_path: temp_dir.path(),
    })
    .context("failed to extract archive")?
  };
  fs::set_permissions(&new_exe_path, permissions).with_context(|| {
    format!("failed to set permissions on '{}'", new_exe_path.display())
  })?;
  check_exe(&new_exe_path)?;

  if upgrade_flags.dry_run {
    fs::remove_file(&new_exe_path).with_context(|| {
      format!("failed to remove '{}'", new_exe_path.display())
    })?;
    log::info!("Upgraded successfully (dry run)");
    print_release_notes(
      version::ENDE_VERSION_INFO.ende,
      &selected_version_to_upgrade,
    );
    drop(temp_dir);
    return Ok(());
  }

  let output_exe_path =
    full_path_output_flag.as_ref().unwrap_or(&current_exe_path);

  #[cfg(windows)]
  kill_running_ende_lsp_processes();

  let output_result = if *output_exe_path == current_exe_path {
    replace_exe(&new_exe_path, output_exe_path)
  } else {
    fs::rename(&new_exe_path, output_exe_path)
      .or_else(|_| fs::copy(&new_exe_path, output_exe_path).map(|_| ()))
  };
  check_windows_access_denied_error(output_result, output_exe_path)?;

  log::info!(
    "\nUpgraded successfully to Ende {}\n",
    colors::green(format!("v{selected_version_to_upgrade}")),
  );
  print_release_notes(
    version::ENDE_VERSION_INFO.ende,
    &selected_version_to_upgrade,
  );

  if let Ok(Some(text)) = banner_handle.await {
    log::info!("\n{}\n", text);
  }

  drop(temp_dir); // delete the temp dir

  Ok(())
}

/// The version passed to `ende upgrade`, without a leading `v`, or `None`
/// to upgrade to the latest release.
fn requested_version(
  upgrade_flags: &UpgradeFlags,
) -> Result<Option<String>, AnyError> {
  let Some(passed_version) = &upgrade_flags.version else {
    return Ok(None);
  };
  let passed_version =
    passed_version.strip_prefix('v').unwrap_or(passed_version);
  if Version::parse_standard(passed_version).is_err() {
    bail!(
      "Invalid version passed ({})\n\nPass a release version, like 1.45.0.\n\nUsage:\n{}",
      colors::gray(passed_version),
      UPGRADE_USAGE
    );
  }
  Ok(Some(passed_version.to_string()))
}

fn select_specific_version_for_upgrade(
  version: String,
  force: bool,
) -> Option<String> {
  if !force && version::ENDE_VERSION_INFO.ende == version {
    log::info!(
      "Version {} is already installed",
      version::ENDE_VERSION_INFO.ende
    );
    return None;
  }

  Some(version)
}

async fn find_latest_version_to_upgrade(
  http_client_provider: Arc<HttpClientProvider>,
  force: bool,
) -> Result<Option<String>, AnyError> {
  log::info!("{}", colors::gray("Looking up latest version"));

  let client = http_client_provider.get_or_create()?;

  let latest_version_found =
    match fetch_latest_version(&client, UpgradeCheckKind::Execution).await {
      Ok(v) => v,
      Err(err) => {
        if err.to_string().contains("Not found") {
          bail!("No release available at the moment.");
        } else {
          return Err(err);
        }
      }
    };

  let current_version = version::ENDE_VERSION_INFO.ende;
  let should_upgrade = force || current_version != latest_version_found;

  log::info!("");
  if should_upgrade {
    log::info!(
      "Found latest version {}",
      color_print::cformat!("<g>v{}</>", latest_version_found)
    );
  } else {
    log::info!(
      "Local ende version {} is the most recent release",
      color_print::cformat!("<g>{}</>", current_version)
    );
  }
  log::info!("");

  Ok(should_upgrade.then_some(latest_version_found))
}

async fn fetch_latest_version(
  client: &HttpClient,
  check_kind: UpgradeCheckKind,
) -> Result<String, AnyError> {
  let url = get_latest_version_url(check_kind);
  let text = client.download_text(url.parse()?).await?;
  Ok(normalize_version_from_server(&text))
}

fn normalize_version_from_server(text: &str) -> String {
  text.trim().trim_start_matches('v').to_string()
}

fn get_latest_version_url(check_kind: UpgradeCheckKind) -> String {
  let query_param = match check_kind {
    UpgradeCheckKind::Execution => "",
    UpgradeCheckKind::Lsp => "?lsp",
  };
  format!("{}/release-latest.txt{}", base_upgrade_url(), query_param)
}

fn base_upgrade_url() -> Cow<'static, str> {
  // this is used by the test suite
  if let Ok(url) = env::var("ENDE_DONT_USE_INTERNAL_BASE_UPGRADE_URL") {
    Cow::Owned(url)
  } else {
    Cow::Borrowed("https://dl.ende.sh")
  }
}

fn get_download_url(version: &str) -> Result<Url, AnyError> {
  let release_url = if std::env::var_os("ENDE_TESTING_UPGRADE").is_some() {
    "http://localhost:4545/ende-upgrade"
  } else {
    RELEASE_URL
  };
  let download_url =
    format!("{}/download/v{}/{}", release_url, version, *ARCHIVE_NAME);

  Url::parse(&download_url).with_context(|| {
    format!(
      "Failed to parse URL to download new release: {}",
      download_url
    )
  })
}

/// Build the chain of consecutive versions from current to target.
/// Each release only has a delta from its immediate predecessor, so
/// multi-step upgrades (e.g. 2.7.10 -> 2.7.12) require chaining:
///   download 2.7.11's delta from 2.7.10, apply,
///   download 2.7.12's delta from 2.7.11, apply.
///
/// Supports:
/// - Same-minor patch upgrades (e.g. 2.7.10 -> 2.7.12)
/// - Cross-minor upgrades (e.g. 2.7.13 -> 2.8.0 -> 2.8.1): the first
///   step crosses the minor boundary, remaining steps are same-minor.
///
/// Total chain length is capped at 3 steps. Returns empty if the
/// versions are not eligible for delta upgrade.
fn build_delta_chain(
  current_version: &str,
  target_version: &str,
) -> Vec<(String, String)> {
  let Ok(current) = Version::parse_standard(current_version) else {
    return vec![];
  };
  let Ok(target) = Version::parse_standard(target_version) else {
    return vec![];
  };
  if !current.pre.is_empty()
    || !target.pre.is_empty()
    || current.major != target.major
  {
    return vec![];
  }

  let mut chain = Vec::new();

  if current.minor == target.minor {
    // Same minor: chain through consecutive patches
    let diff = target.patch.saturating_sub(current.patch);
    if diff == 0 || diff > 3 {
      return vec![];
    }
    for i in 0..diff {
      chain.push((
        format!("{}.{}.{}", current.major, current.minor, current.patch + i),
        format!(
          "{}.{}.{}",
          current.major,
          current.minor,
          current.patch + i + 1
        ),
      ));
    }
  } else if target.minor == current.minor + 1 {
    // Cross-minor: first step jumps to target_minor.0, then chain patches
    let minor_zero = format!("{}.{}.0", target.major, target.minor);
    chain.push((current_version.to_string(), minor_zero));
    for i in 0..target.patch {
      chain.push((
        format!("{}.{}.{}", target.major, target.minor, i),
        format!("{}.{}.{}", target.major, target.minor, i + 1),
      ));
    }
    if chain.len() > 3 {
      return vec![];
    }
  } else {
    // Too many minor versions apart
    return vec![];
  }

  chain
}

/// Construct the delta patch download URL for a stable release.
fn get_delta_download_url(
  current_version: &str,
  target_version: &str,
) -> Result<Url, AnyError> {
  let release_url = if std::env::var_os("ENDE_TESTING_UPGRADE").is_some() {
    "http://localhost:4545/ende-upgrade"
  } else {
    RELEASE_URL
  };
  let url = format!(
    "{}/download/v{}/{}.from-{}.bsdiff",
    release_url, target_version, *DELTA_TARGET_NAME, current_version
  );
  Url::parse(&url)
    .with_context(|| format!("Failed to parse delta download URL: {}", url))
}

/// Construct the URL for the delta patch's SHA-256 checksum file.
fn get_delta_checksum_url(
  current_version: &str,
  target_version: &str,
) -> Result<Url, AnyError> {
  let release_url = if std::env::var_os("ENDE_TESTING_UPGRADE").is_some() {
    "http://localhost:4545/ende-upgrade"
  } else {
    RELEASE_URL
  };
  let url = format!(
    "{}/download/v{}/{}.from-{}.bsdiff.sha256sum",
    release_url, target_version, *DELTA_TARGET_NAME, current_version
  );
  Url::parse(&url)
    .with_context(|| format!("Failed to parse delta checksum URL: {}", url))
}

/// Construct the URL for the uncompressed binary's SHA-256 checksum file.
fn get_binary_checksum_url(target_version: &str) -> Result<Url, AnyError> {
  let release_url = if std::env::var_os("ENDE_TESTING_UPGRADE").is_some() {
    "http://localhost:4545/ende-upgrade"
  } else {
    RELEASE_URL
  };
  let url = format!(
    "{}/download/v{}/{}.sha256sum",
    release_url, target_version, *DELTA_TARGET_NAME
  );
  Url::parse(&url)
    .with_context(|| format!("Failed to parse binary checksum URL: {}", url))
}

/// Fetch a SHA-256 hash from a .sha256sum file hosted as a release asset.
/// The file format is expected to be: `<hex_hash>  <filename>\n`
async fn fetch_sha256_from_url(
  client: &HttpClient,
  url: Url,
) -> Option<String> {
  let text = client.download_text(url).await.ok()?;
  // sha256sum files are formatted as "<hash>  <filename>" or just "<hash>"
  let hash = text.split_whitespace().next()?;
  if hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
    Some(hash.to_lowercase())
  } else {
    None
  }
}

/// Compute the SHA-256 hash of the given data, returning it as a lowercase
/// hex string.
fn compute_sha256(data: &[u8]) -> String {
  let digest = sha2::Sha256::digest(data);
  faster_hex::hex_string(&digest)
}

/// Attempt a delta upgrade by chaining consecutive bsdiff patches.
/// For a single-step upgrade (e.g. 2.7.11 -> 2.7.12), downloads and
/// applies one patch. For multi-step (e.g. 2.7.10 -> 2.7.12), chains
/// patches: downloads the 2.7.10->2.7.11 patch from the 2.7.11 release,
/// applies it, then downloads the 2.7.11->2.7.12 patch from the 2.7.12
/// release, applies that. Returns `Some(final_binary)` on success, or
/// `None` on any failure (triggering full download fallback).
async fn try_delta_upgrade(
  client: &HttpClient,
  current_exe: &Path,
  current_version: &str,
  target_version: &str,
) -> Option<Vec<u8>> {
  let chain = build_delta_chain(current_version, target_version);
  if chain.is_empty() {
    log::debug!(
      "No delta chain from {} to {}, skipping delta upgrade",
      current_version,
      target_version
    );
    return None;
  }

  let steps = chain.len();
  log::info!(
    "{}",
    colors::gray(format!(
      "Attempting delta upgrade ({} step{})",
      steps,
      if steps > 1 { "s" } else { "" }
    ))
  );

  // Read the current binary
  let mut binary = match std::fs::read(current_exe) {
    Ok(data) => data,
    Err(e) => {
      log::debug!("Failed to read current binary: {}", e);
      return None;
    }
  };

  // Verify the current binary's integrity
  let current_checksum_url = get_binary_checksum_url(current_version).ok()?;
  if let Some(expected_hash) =
    fetch_sha256_from_url(client, current_checksum_url).await
  {
    let actual_hash = compute_sha256(&binary);
    if actual_hash != expected_hash {
      log::debug!("Current binary checksum mismatch, skipping delta");
      return None;
    }
  } else {
    log::debug!(
      "Could not fetch checksum for current binary v{}, skipping delta",
      current_version,
    );
    return None;
  }

  // Apply each step in the chain
  for (i, (src, dst)) in chain.iter().enumerate() {
    if steps > 1 {
      log::info!(
        "{}",
        colors::gray(format!("Step {}/{}: {} -> {}", i + 1, steps, src, dst))
      );
    }

    // Download the delta patch (from the DST version's release)
    let delta_url = get_delta_download_url(src, dst).ok()?;
    log::info!(
      "{}",
      colors::gray(format!("Downloading delta patch {}", &delta_url))
    );
    let delta_data = match download_package(client, delta_url).await {
      Ok(Some(data)) => data,
      Ok(None) => {
        log::debug!("Delta patch not found for {} -> {}", src, dst);
        return None;
      }
      Err(e) => {
        log::debug!("Failed to download delta patch: {}", e);
        return None;
      }
    };

    // Verify the delta patch's own checksum
    let delta_checksum_url = get_delta_checksum_url(src, dst).ok()?;
    if let Some(expected_delta_hash) =
      fetch_sha256_from_url(client, delta_checksum_url).await
    {
      let actual_delta_hash = compute_sha256(&delta_data);
      if actual_delta_hash != expected_delta_hash {
        log::warn!("Delta patch checksum mismatch for {} -> {}", src, dst);
        return None;
      }
    } else {
      log::debug!(
        "Could not fetch delta patch checksum for {} -> {}",
        src,
        dst
      );
      return None;
    }

    // Apply the bsdiff patch
    binary = match apply_bsdiff_patch(&binary, &delta_data) {
      Ok(data) => data,
      Err(e) => {
        log::warn!("Failed to apply delta patch {} -> {}: {}", src, dst, e);
        return None;
      }
    };

    // Verify the intermediate/final binary
    let checksum_url = get_binary_checksum_url(dst).ok()?;
    if let Some(expected_hash) =
      fetch_sha256_from_url(client, checksum_url).await
    {
      let actual_hash = compute_sha256(&binary);
      if actual_hash != expected_hash {
        log::warn!(
          "Patched binary checksum mismatch after step {} -> {}",
          src,
          dst
        );
        return None;
      }
    } else {
      log::debug!(
        "Could not fetch binary checksum for v{}, skipping delta",
        dst
      );
      return None;
    }
  }

  log::info!("{}", colors::gray("Delta upgrade checksum verified"));

  log::info!(
    "{}",
    colors::gray(format!(
      "Delta upgrade applied successfully ({:.1} MB binary)",
      binary.len() as f64 / 1_000_000.0
    ))
  );

  Some(binary)
}

/// zstd magic number — published patches generated by `tools/bsdiff_helper`
/// are zstd-wrapped. Detected here so we can fall through to legacy raw
/// bsdiff for patches uploaded before that change (e.g. v2.8.0's
/// `from-2.7.14.bsdiff`).
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// Apply a bsdiff patch to produce a new binary.
///
/// Accepts both zstd-wrapped patches (current format) and raw bsdiff patches
/// (legacy format, pre-zstd-wrapping).
fn apply_bsdiff_patch(
  old: &[u8],
  patch_data: &[u8],
) -> Result<Vec<u8>, AnyError> {
  let decoded;
  let raw_patch = if patch_data.starts_with(&ZSTD_MAGIC) {
    // Stream the decompression so the output buffer grows incrementally
    // instead of pre-allocating MAX_PATCH_SIZE up front (which OOMs on
    // low-memory devices). `.take(MAX_PATCH_SIZE)` keeps the anti-DoS cap.
    use std::io::Read;
    let decoder = zstd::stream::Decoder::new(patch_data)
      .context("failed to zstd-decompress delta patch")?;
    let mut buf = Vec::new();
    decoder
      .take(MAX_PATCH_SIZE as u64)
      .read_to_end(&mut buf)
      .context("failed to zstd-decompress delta patch")?;
    decoded = buf;
    decoded.as_slice()
  } else {
    patch_data
  };
  let mut cursor = std::io::Cursor::new(raw_patch);
  let mut new = Vec::new();
  bsdiff::patch(old, &mut cursor, &mut new)
    .context("bspatch failed to apply delta patch")?;
  Ok(new)
}

/// Upper bound on the decompressed bsdiff patch size. Generous (2 GiB) since
/// the raw bsdiff stream can exceed the new binary's size; we only need to
/// reject pathological/malicious inputs, not enforce a tight budget.
const MAX_PATCH_SIZE: usize = 2 * 1024 * 1024 * 1024;

fn spawn_banner_task(
  version: &str,
  client: HttpClient,
) -> ende_core::unsync::JoinHandle<Option<String>> {
  let banner_url = get_banner_url(version);
  ende_core::unsync::spawn(async move {
    let banner_url = banner_url?;
    tokio::select! {
      result = client.download_text(banner_url) => {
        result.ok()
      }
      _ = tokio::time::sleep(Duration::from_secs(5)) => {
        None
      }
    }
  })
}

fn get_banner_url(version: &str) -> Option<Url> {
  Url::parse(&format!("{}/v{}/banner.txt", DL_RELEASE_URL, version)).ok()
}

async fn download_package(
  client: &HttpClient,
  download_url: Url,
) -> Result<Option<Vec<u8>>, AnyError> {
  let progress_bar = ProgressBar::new(ProgressBarStyle::DownloadBars);
  // provide an empty string here in order to prefer the downloading
  // text above which will stay alive after the progress bars are complete
  let progress = progress_bar.update("");
  let response = client
    .download_with_progress_and_retries(download_url.clone(), &Default::default(), &progress)
    .await
    .with_context(|| format!("Failed downloading {download_url}. The version you requested may not have been built for the current architecture."))?;
  Ok(response.into_maybe_bytes()?)
}

fn verify_checksum(
  data: &[u8],
  expected_checksum: &str,
) -> Result<(), AnyError> {
  let computed = sha2::Sha256::digest(data);
  let computed_hex = faster_hex::hex_string(&computed);

  let expected_checksum = expected_checksum.trim().to_lowercase();
  if computed_hex != expected_checksum {
    bail!(
      "Checksum verification failed.\n  Actual:   {}\n  Expected: {}",
      expected_checksum,
      computed_hex
    );
  }

  log::info!("{}", colors::gray("Checksum verified"));
  Ok(())
}

fn replace_exe(from: &Path, to: &Path) -> Result<(), std::io::Error> {
  if cfg!(windows) {
    // On windows you cannot replace the currently running executable.
    // so first we rename it to deno.old.exe
    fs::rename(to, to.with_extension("old.exe"))?;
  } else {
    fs::remove_file(to)?;
  }
  // Windows cannot rename files across device boundaries, so if rename fails,
  // we try again with copy.
  fs::rename(from, to).or_else(|_| fs::copy(from, to).map(|_| ()))?;
  Ok(())
}

fn check_windows_access_denied_error(
  output_result: Result<(), std::io::Error>,
  output_exe_path: &Path,
) -> Result<(), AnyError> {
  let Err(err) = output_result else {
    return Ok(());
  };

  if !cfg!(windows) {
    return Err(err).with_context(|| {
      format!(
        "failed to replace the executable at '{}'",
        output_exe_path.display()
      )
    });
  }

  const WIN_ERROR_ACCESS_DENIED: i32 = 5;
  if err.raw_os_error() != Some(WIN_ERROR_ACCESS_DENIED) {
    return Err(err.into());
  };

  Err(err).with_context(|| {
    format!(
      concat!(
        "Could not replace the ende executable. This may be because an ",
        "existing ende process is running. Please ensure there are no ",
        "running ende processes (ex. Stop-Process -Name ende ; ende {}), ",
        "close any editors before upgrading, and ensure you have ",
        "sufficient permission to '{}'."
      ),
      // skip the first argument, which is the executable path
      std::env::args().skip(1).collect::<Vec<_>>().join(" "),
      output_exe_path.display(),
    )
  })
}

#[cfg(windows)]
fn kill_running_ende_lsp_processes() {
  // limit this to `ende lsp` invocations to avoid killing important programs someone might be running
  let is_debug = log::log_enabled!(log::Level::Debug);
  let get_pipe = || {
    if is_debug {
      std::process::Stdio::inherit()
    } else {
      std::process::Stdio::null()
    }
  };
  let _ = Command::new("powershell.exe")
    .args([
      "-Command",
      r#"Get-WmiObject Win32_Process | Where-Object {
    $_.Name -eq 'ende.exe' -and
    $_.CommandLine -match '^(?:\"[^\"]+\"|\S+)\s+lsp\b'
} | ForEach-Object {
  if ($_.Terminate()) {
    Write-Host 'Terminated:' $_.ProcessId
  }
}"#,
    ])
    .stdout(get_pipe())
    .stderr(get_pipe())
    .output();
}

fn set_exe_permissions(
  current_exe_path: &Path,
  output_exe_path: &Path,
) -> Result<std::fs::Permissions, AnyError> {
  let Ok(metadata) = fs::metadata(output_exe_path) else {
    let metadata = fs::metadata(current_exe_path).with_context(|| {
      format!(
        "failed to get metadata of the current executable at '{}'",
        current_exe_path.display()
      )
    })?;
    return Ok(metadata.permissions());
  };

  let permissions = metadata.permissions();
  if permissions.readonly() {
    bail!(
      "You do not have write permission to {}",
      output_exe_path.display()
    );
  }
  #[cfg(unix)]
  if std::os::unix::fs::MetadataExt::uid(&metadata) == 0
    && !nix::unistd::Uid::effective().is_root()
  {
    bail!(
      concat!(
        "You don't have write permission to {} because it's owned by root.\n",
        "Consider updating ende through your package manager if its installed from it.\n",
        "Otherwise run `ende upgrade` as root.",
      ),
      output_exe_path.display()
    );
  }
  Ok(permissions)
}

fn check_exe(exe_path: &Path) -> Result<(), AnyError> {
  let output = Command::new(exe_path)
    .arg("-V")
    .stderr(std::process::Stdio::inherit())
    .output()
    .with_context(|| format!("failed to run '{}'", exe_path.display()))?;
  if !output.status.success() {
    bail!(
      "Failed to validate Ende executable. This may be because your OS is unsupported or the executable is corrupted"
    )
  } else {
    Ok(())
  }
}

#[derive(Debug)]
struct CheckVersionFile {
  pub last_prompt: chrono::DateTime<chrono::Utc>,
  pub last_checked: chrono::DateTime<chrono::Utc>,
  pub current_version: String,
  pub latest_version: String,
}

impl CheckVersionFile {
  pub fn parse(content: String) -> Option<Self> {
    let split_content = content.split('!').collect::<Vec<_>>();

    // Files written before Ende had a single release channel have a fifth
    // field holding the channel of the binary that wrote them. It is ignored.
    if !(4..=5).contains(&split_content.len()) {
      return None;
    }

    let latest_version = split_content[2].trim().to_owned();
    if latest_version.is_empty() {
      return None;
    }
    let current_version = split_content[3].trim().to_owned();
    if current_version.is_empty() {
      return None;
    }
    let last_prompt = chrono::DateTime::parse_from_rfc3339(split_content[0])
      .map(|dt| dt.with_timezone(&chrono::Utc))
      .ok()?;
    let last_checked = chrono::DateTime::parse_from_rfc3339(split_content[1])
      .map(|dt| dt.with_timezone(&chrono::Utc))
      .ok()?;

    Some(CheckVersionFile {
      last_prompt,
      last_checked,
      current_version,
      latest_version,
    })
  }

  fn serialize(&self) -> String {
    format!(
      "{}!{}!{}!{}",
      self.last_prompt.to_rfc3339(),
      self.last_checked.to_rfc3339(),
      self.latest_version,
      self.current_version,
    )
  }

  fn with_last_prompt(self, dt: chrono::DateTime<chrono::Utc>) -> Self {
    Self {
      last_prompt: dt,
      ..self
    }
  }
}

#[cfg(test)]
mod test {
  use std::cell::RefCell;
  use std::rc::Rc;

  use test_util::assert_contains;

  use super::*;

  #[test]
  fn test_get_pr_artifact_name() {
    let name = get_pr_artifact_name().unwrap();
    // Should match the pattern "release-{os}-{arch}-ende"
    assert!(
      name.starts_with("release-"),
      "artifact name should start with 'release-': {name}"
    );
    assert!(
      name.ends_with("-ende"),
      "artifact name should end with '-ende': {name}"
    );
    // Should contain a valid os
    assert!(
      name.contains("linux")
        || name.contains("macos")
        || name.contains("windows"),
      "artifact name should contain os: {name}"
    );
    // Should contain a valid arch
    assert!(
      name.contains("x86_64") || name.contains("aarch64"),
      "artifact name should contain arch: {name}"
    );
  }

  #[test]
  fn test_get_pr_debug_artifact_name() {
    let release_name = get_pr_artifact_name().unwrap();
    let debug_name = get_pr_debug_artifact_name().unwrap();
    assert!(
      debug_name.starts_with("debug-"),
      "debug artifact name should start with 'debug-': {debug_name}"
    );
    // The rest should match
    assert_eq!(
      release_name.strip_prefix("release-"),
      debug_name.strip_prefix("debug-"),
    );
  }

  #[test]
  fn test_requested_version() {
    let mut upgrade_flags = UpgradeFlags {
      dry_run: false,
      force: false,
      no_delta: false,
      version: None,
      output: None,
      checksum: None,
      pr: None,
      branch: None,
    };

    assert_eq!(requested_version(&upgrade_flags).unwrap(), None);

    upgrade_flags.version = Some("1.46.0".to_string());
    assert_eq!(
      requested_version(&upgrade_flags).unwrap(),
      Some("1.46.0".to_string())
    );

    upgrade_flags.version = Some("v1.46.5".to_string());
    assert_eq!(
      requested_version(&upgrade_flags).unwrap(),
      Some("1.46.5".to_string())
    );

    upgrade_flags.version = Some("5c69b4861b52a".to_string());
    let err = requested_version(&upgrade_flags).unwrap_err().to_string();
    assert_contains!(err, "Invalid version passed");
    assert_contains!(err, "Pass a release version, like 1.45.0.");

    upgrade_flags.version = Some("11.asd.1324".to_string());
    let err = requested_version(&upgrade_flags).unwrap_err().to_string();
    assert_contains!(err, "Invalid version passed");
    assert_contains!(err, "Pass a release version, like 1.45.0.");
  }

  #[test]
  fn test_parse_upgrade_check_file() {
    let file = CheckVersionFile::parse(
      "2020-01-01T00:00:00+00:00!2020-01-01T00:00:00+00:00!1.2.3!1.2.2"
        .to_string(),
    )
    .unwrap();
    assert_eq!(
      file.last_prompt.to_rfc3339(),
      "2020-01-01T00:00:00+00:00".to_string()
    );
    assert_eq!(
      file.last_checked.to_rfc3339(),
      "2020-01-01T00:00:00+00:00".to_string()
    );
    assert_eq!(file.latest_version, "1.2.3".to_string());
    assert_eq!(file.current_version, "1.2.2".to_string());

    let result =
      CheckVersionFile::parse("2020-01-01T00:00:00+00:00!".to_string());
    assert!(result.is_none());

    let result = CheckVersionFile::parse("garbage!test".to_string());
    assert!(result.is_none());

    let result = CheckVersionFile::parse("test".to_string());
    assert!(result.is_none());

    let result = CheckVersionFile::parse(
      "2020-01-01T00:00:00+00:00!2020-01-01T00:00:00+00:00!1.2.3!1.2.2!stable!x"
        .to_string(),
    );
    assert!(result.is_none());
  }

  #[test]
  fn test_parse_upgrade_check_file_with_release_channel() {
    // Files written before Ende had a single release channel end with the
    // channel of the binary that wrote them. They still load, and the
    // channel is ignored, whatever it is.
    for channel in ["stable", "canary", "rc", "lts", "alpha", "beta", "x"] {
      let file = CheckVersionFile::parse(format!(
        "2020-01-01T00:00:00+00:00!2020-01-01T00:00:00+00:00!1.2.3!1.2.2!{channel}"
      ))
      .unwrap();
      assert_eq!(file.latest_version, "1.2.3".to_string());
      assert_eq!(file.current_version, "1.2.2".to_string());
    }
  }

  #[test]
  fn test_serialize_upgrade_check_file() {
    let file = CheckVersionFile {
      last_prompt: chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc),
      last_checked: chrono::DateTime::parse_from_rfc3339(
        "2020-01-01T00:00:00Z",
      )
      .unwrap()
      .with_timezone(&chrono::Utc),
      latest_version: "1.2.3".to_string(),
      current_version: "1.2.2".to_string(),
    };
    assert_eq!(
      file.serialize(),
      "2020-01-01T00:00:00+00:00!2020-01-01T00:00:00+00:00!1.2.3!1.2.2"
    );
  }

  #[derive(Clone)]
  struct TestUpdateCheckerEnvironment {
    file_text: Rc<RefCell<String>>,
    current_version: Rc<RefCell<String>>,
    latest_version: Rc<RefCell<Result<String, String>>>,
    time: Rc<RefCell<chrono::DateTime<chrono::Utc>>>,
  }

  impl TestUpdateCheckerEnvironment {
    pub fn new() -> Self {
      Self {
        file_text: Default::default(),
        current_version: Default::default(),
        latest_version: Rc::new(RefCell::new(Ok(String::new()))),
        time: Rc::new(RefCell::new(chrono::Utc::now())),
      }
    }

    pub fn add_hours(&self, hours: i64) {
      let mut time = self.time.borrow_mut();
      *time = time
        .checked_add_signed(chrono::Duration::hours(hours))
        .unwrap();
    }

    pub fn set_file_text(&self, text: &str) {
      *self.file_text.borrow_mut() = text.to_string();
    }

    pub fn set_current_version(&self, version: &str) {
      *self.current_version.borrow_mut() = version.to_string();
    }

    pub fn set_latest_version(&self, version: &str) {
      *self.latest_version.borrow_mut() = Ok(version.to_string());
    }

    pub fn set_latest_version_err(&self, err: &str) {
      *self.latest_version.borrow_mut() = Err(err.to_string());
    }
  }

  #[async_trait(?Send)]
  impl VersionProvider for TestUpdateCheckerEnvironment {
    async fn latest_version(&self) -> Result<String, AnyError> {
      match self.latest_version.borrow().clone() {
        Ok(result) => Ok(result),
        Err(err) => bail!("{}", err),
      }
    }

    fn current_version(&self) -> Cow<'_, str> {
      Cow::Owned(self.current_version.borrow().clone())
    }
  }

  impl UpdateCheckerEnvironment for TestUpdateCheckerEnvironment {
    fn read_check_file(&self) -> String {
      self.file_text.borrow().clone()
    }

    fn write_check_file(&self, text: &str) {
      self.set_file_text(text);
    }

    fn current_time(&self) -> chrono::DateTime<chrono::Utc> {
      *self.time.borrow()
    }
  }

  #[tokio::test]
  async fn test_update_checker() {
    let env = TestUpdateCheckerEnvironment::new();
    env.set_current_version("1.0.0");
    env.set_latest_version("1.1.0");
    let checker = UpdateChecker::new(env.clone(), env.clone());

    // no version, so we should check, but not prompt
    assert!(checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), None);

    // store the latest version
    fetch_and_store_latest_version(&env, &env).await;

    // reload
    let checker = UpdateChecker::new(env.clone(), env.clone());

    // should not check for latest version because we just did
    assert!(!checker.should_check_for_new_version());
    // but should prompt
    assert_eq!(checker.should_prompt(), Some("1.1.0".to_string()));

    // fast forward an hour and bump the latest version
    env.add_hours(1);
    env.set_latest_version("1.2.0");
    assert!(!checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), Some("1.1.0".to_string()));

    // fast forward again and it should check for a newer version
    env.add_hours(UPGRADE_CHECK_INTERVAL);
    assert!(checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), Some("1.1.0".to_string()));

    fetch_and_store_latest_version(&env, &env).await;

    // reload and store that we prompted
    let checker = UpdateChecker::new(env.clone(), env.clone());
    assert!(!checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), Some("1.2.0".to_string()));
    checker.store_prompted();

    // reload and it should now say not to prompt
    let checker = UpdateChecker::new(env.clone(), env.clone());
    assert!(!checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), None);

    // but if we fast forward past the upgrade interval it should prompt again
    env.add_hours(UPGRADE_CHECK_INTERVAL + 1);
    assert!(checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), Some("1.2.0".to_string()));

    // upgrade the version and it should stop prompting
    env.set_current_version("1.2.0");
    assert!(checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), None);

    // now try failing when fetching the latest version
    env.add_hours(UPGRADE_CHECK_INTERVAL + 1);
    env.set_latest_version_err("Failed");
    env.set_latest_version("1.3.0");

    // this will silently fail
    fetch_and_store_latest_version(&env, &env).await;
    assert!(checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), None);
  }

  #[tokio::test]
  async fn test_update_checker_reads_check_file_with_release_channel() {
    let env = TestUpdateCheckerEnvironment::new();
    let last_prompt = env
      .current_time()
      .sub(chrono::Duration::hours(UPGRADE_CHECK_INTERVAL + 1));
    env.write_check_file(&format!(
      "{}!{}!1.1.0!1.0.0!stable",
      last_prompt.to_rfc3339(),
      env.current_time().to_rfc3339(),
    ));
    env.set_current_version("1.0.0");

    let checker = UpdateChecker::new(env.clone(), env.clone());
    assert!(!checker.should_check_for_new_version());
    assert_eq!(checker.should_prompt(), Some("1.1.0".to_string()));

    // storing the prompt rewrites the file without the channel
    checker.store_prompted();
    assert_eq!(env.read_check_file().split('!').count(), 4);
    let checker = UpdateChecker::new(env.clone(), env);
    assert_eq!(checker.should_prompt(), None);
  }

  #[tokio::test]
  async fn test_update_checker_current_newer_than_latest() {
    let env = TestUpdateCheckerEnvironment::new();
    let file_content = CheckVersionFile {
      last_prompt: env
        .current_time()
        .sub(chrono::Duration::hours(UPGRADE_CHECK_INTERVAL + 1)),
      last_checked: env.current_time(),
      latest_version: "1.26.2".to_string(),
      current_version: "1.27.0".to_string(),
    }
    .serialize();
    env.write_check_file(&file_content);
    env.set_current_version("1.27.0");
    env.set_latest_version("1.26.2");
    let checker = UpdateChecker::new(env.clone(), env);

    // since currently running version is newer than latest available (eg. CDN
    // propagation might be delated) we should not prompt
    assert_eq!(checker.should_prompt(), None);
  }

  #[tokio::test]
  async fn test_should_not_prompt_if_current_cli_version_has_changed() {
    let env = TestUpdateCheckerEnvironment::new();
    let file_content = CheckVersionFile {
      last_prompt: env
        .current_time()
        .sub(chrono::Duration::hours(UPGRADE_CHECK_INTERVAL + 1)),
      last_checked: env.current_time(),
      latest_version: "1.26.2".to_string(),
      current_version: "1.25.0".to_string(),
    }
    .serialize();
    env.write_check_file(&file_content);
    // simulate an upgrade done since the file was written
    env.set_current_version("1.26.0");
    let checker = UpdateChecker::new(env.clone(), env);
    assert_eq!(checker.should_prompt(), None);
  }

  #[test]
  fn test_get_latest_version_url() {
    assert_eq!(
      get_latest_version_url(UpgradeCheckKind::Execution),
      "https://dl.ende.sh/release-latest.txt"
    );
    assert_eq!(
      get_latest_version_url(UpgradeCheckKind::Lsp),
      "https://dl.ende.sh/release-latest.txt?lsp"
    );
  }

  #[test]
  fn test_normalize_version_server() {
    // should strip v
    assert_eq!(normalize_version_from_server("v1.0.0"), "1.0.0");
    // should not replace v after start
    assert_eq!(
      normalize_version_from_server("  v1.0.0-test-v\n\n  "),
      "1.0.0-test-v"
    );
  }

  #[tokio::test]
  async fn test_upgrades_lsp() {
    let env = TestUpdateCheckerEnvironment::new();
    env.set_current_version("1.0.0");
    env.set_latest_version("2.0.0");

    // greater
    {
      let maybe_info = check_for_upgrades_for_lsp_with_provider(&env)
        .await
        .unwrap();
      assert_eq!(
        maybe_info,
        Some(LspVersionUpgradeInfo {
          latest_version: "2.0.0".to_string(),
        })
      );
    }
    // equal
    {
      env.set_latest_version("1.0.0");
      let maybe_info = check_for_upgrades_for_lsp_with_provider(&env)
        .await
        .unwrap();
      assert_eq!(maybe_info, None);
    }
    // less
    {
      env.set_latest_version("0.9.0");
      let maybe_info = check_for_upgrades_for_lsp_with_provider(&env)
        .await
        .unwrap();
      assert_eq!(maybe_info, None);
    }
  }

  #[test]
  fn blog_post_links() {
    let version = Version::parse_standard("1.46.0").unwrap();
    assert_eq!(
      get_minor_version_blog_post_url(&version),
      "https://ende.sh/blog/v1.46"
    );

    let version = Version::parse_standard("2.1.1").unwrap();
    assert_eq!(
      get_minor_version_blog_post_url(&version),
      "https://ende.sh/blog/v2.1"
    );
  }

  #[test]
  fn test_get_binary_cache_path() {
    let dl_dir = Path::new("/dl");

    let path = get_binary_cache_path(dl_dir, "1.46.0");
    assert_eq!(
      path,
      dl_dir.join(format!("release/v1.46.0/{}", *ARCHIVE_NAME))
    );
  }

  #[test]
  fn test_try_read_cached_binary() {
    use sys_traits::FsCreateDirAll;
    use sys_traits::FsWrite;
    use sys_traits::impls::InMemorySys;

    let sys = InMemorySys::default();

    // Returns None when file doesn't exist
    let result =
      try_read_cached_binary(&sys, Path::new("/dl/release/v1.0.0/deno.zip"));
    assert!(result.is_none());

    // Returns data when file exists
    let cache_path = Path::new("/dl/release/v1.0.0/deno.zip");
    sys.fs_create_dir_all(cache_path.parent().unwrap()).unwrap();
    sys.fs_write(cache_path, b"archive-data").unwrap();

    let result = try_read_cached_binary(&sys, cache_path);
    assert_eq!(result.unwrap(), b"archive-data");
  }

  #[test]
  fn test_store_cached_binary() {
    use sys_traits::FsRead;
    use sys_traits::impls::InMemorySys;

    let sys = InMemorySys::default();
    sys.set_seed(Some(42));

    let cache_path = Path::new("/dl/release/v1.0.0/deno.zip");

    // Should create parent dirs and write the file
    store_cached_binary(&sys, cache_path, b"archive-data");

    let data = sys.fs_read(cache_path).unwrap();
    assert_eq!(data.as_ref(), b"archive-data");
  }

  #[test]
  fn test_store_and_read_cached_binary_roundtrip() {
    use sys_traits::impls::InMemorySys;

    let sys = InMemorySys::default();
    sys.set_seed(Some(42));

    let cache_path = Path::new("/dl/release/v1.0.0/deno.zip");

    // Initially no cache
    assert!(try_read_cached_binary(&sys, cache_path).is_none());

    // Store it
    let original_data = b"test-archive-contents";
    store_cached_binary(&sys, cache_path, original_data);

    // Read it back
    let cached = try_read_cached_binary(&sys, cache_path).unwrap();
    assert_eq!(cached, original_data);
  }

  #[test]
  fn test_build_delta_chain() {
    // Single step
    assert_eq!(
      build_delta_chain("2.7.11", "2.7.12"),
      vec![("2.7.11".to_string(), "2.7.12".to_string())]
    );

    // Multi-step (2 steps)
    assert_eq!(
      build_delta_chain("2.7.10", "2.7.12"),
      vec![
        ("2.7.10".to_string(), "2.7.11".to_string()),
        ("2.7.11".to_string(), "2.7.12".to_string()),
      ]
    );

    // Multi-step (3 steps, max)
    assert_eq!(
      build_delta_chain("2.7.9", "2.7.12"),
      vec![
        ("2.7.9".to_string(), "2.7.10".to_string()),
        ("2.7.10".to_string(), "2.7.11".to_string()),
        ("2.7.11".to_string(), "2.7.12".to_string()),
      ]
    );

    // Too far apart (> 3 steps, same minor)
    assert!(build_delta_chain("2.7.0", "2.7.5").is_empty());
    assert!(build_delta_chain("2.7.8", "2.7.12").is_empty());

    // Cross-minor: direct jump to .0
    assert_eq!(
      build_delta_chain("2.7.13", "2.8.0"),
      vec![("2.7.13".to_string(), "2.8.0".to_string())]
    );

    // Cross-minor: jump to .0 then one patch
    assert_eq!(
      build_delta_chain("2.7.13", "2.8.1"),
      vec![
        ("2.7.13".to_string(), "2.8.0".to_string()),
        ("2.8.0".to_string(), "2.8.1".to_string()),
      ]
    );

    // Cross-minor: jump to .0 then two patches (3 steps total, max)
    assert_eq!(
      build_delta_chain("2.7.13", "2.8.2"),
      vec![
        ("2.7.13".to_string(), "2.8.0".to_string()),
        ("2.8.0".to_string(), "2.8.1".to_string()),
        ("2.8.1".to_string(), "2.8.2".to_string()),
      ]
    );

    // Cross-minor: too many steps (> 3)
    assert!(build_delta_chain("2.7.13", "2.8.3").is_empty());

    // Cross-minor: too many minors apart
    assert!(build_delta_chain("2.6.10", "2.8.0").is_empty());

    // Different major
    assert!(build_delta_chain("1.46.3", "2.0.0").is_empty());

    // Downgrade
    assert!(build_delta_chain("2.7.12", "2.7.11").is_empty());

    // Same version
    assert!(build_delta_chain("2.7.12", "2.7.12").is_empty());

    // Pre-release
    assert!(build_delta_chain("2.7.11-rc.0", "2.7.12").is_empty());

    // Invalid
    assert!(build_delta_chain("invalid", "2.7.12").is_empty());
  }

  #[test]
  fn test_get_delta_download_url() {
    let url = get_delta_download_url("2.7.11", "2.7.12").unwrap();
    let url_str = url.to_string();
    assert!(
      url_str.contains("/download/v2.7.12/"),
      "URL should contain target version: {url_str}"
    );
    assert!(
      url_str.contains(".from-2.7.11.bsdiff"),
      "URL should contain source version and bsdiff extension: {url_str}"
    );
    assert!(
      url_str.contains(&format!("/ende-{}.from-", env!("TARGET"))),
      "URL should contain target name: {url_str}"
    );
    assert!(
      url_str.starts_with("https://github.com/theende/ende/releases/"),
      "URL should point at the ende releases: {url_str}"
    );
  }

  #[test]
  fn test_archive_names() {
    assert_eq!(*ARCHIVE_NAME, format!("ende-{}.zip", env!("TARGET")));
    assert_eq!(*DELTA_TARGET_NAME, format!("ende-{}", env!("TARGET")));
  }

  #[test]
  fn test_get_download_url() {
    let target = env!("TARGET");
    assert_eq!(
      get_download_url("2.0.0").unwrap().as_str(),
      format!(
        "https://github.com/theende/ende/releases/download/v2.0.0/ende-{target}.zip"
      )
    );
  }

  #[test]
  fn test_get_binary_checksum_url() {
    assert_eq!(
      get_binary_checksum_url("2.0.0").unwrap().as_str(),
      format!(
        "https://github.com/theende/ende/releases/download/v2.0.0/ende-{}.sha256sum",
        env!("TARGET")
      )
    );
  }

  #[test]
  fn test_get_release_notes_url() {
    assert_eq!(
      get_release_notes_url("2.1.0"),
      "https://github.com/theende/ende/releases/tag/v2.1.0"
    );
  }

  #[test]
  fn test_apply_bsdiff_patch_raw() {
    // Legacy format: bsdiff output written straight to disk. Must keep
    // working so clients on the current release can still apply the
    // uncompressed patches already published (e.g. v2.8.0's
    // `from-2.7.14.bsdiff`).
    let old_data = b"Hello, World!";
    let new_data = b"Hello, Deno!";
    let mut patch = Vec::new();
    bsdiff::diff(old_data, new_data, &mut patch).unwrap();
    assert!(
      !patch.starts_with(&ZSTD_MAGIC),
      "raw bsdiff output should not look like a zstd stream"
    );

    let result = apply_bsdiff_patch(old_data, &patch).unwrap();
    assert_eq!(result, new_data);
  }

  #[test]
  fn test_apply_bsdiff_patch_zstd_wrapped() {
    // Current format produced by `tools/bsdiff_helper`: bsdiff output wrapped
    // in zstd. Must round-trip identically to the raw form.
    let old_data = b"Hello, World!";
    let new_data = b"Hello, Deno!";
    let mut raw_patch = Vec::new();
    bsdiff::diff(old_data, new_data, &mut raw_patch).unwrap();
    let wrapped = zstd::bulk::compress(&raw_patch, 19).unwrap();
    assert!(
      wrapped.starts_with(&ZSTD_MAGIC),
      "zstd-compressed patch should start with the zstd magic number"
    );

    let result = apply_bsdiff_patch(old_data, &wrapped).unwrap();
    assert_eq!(result, new_data);
  }

  #[test]
  fn test_apply_bsdiff_patch_zstd_streamed() {
    // Regression for the OOM in #34809: a zstd frame written by a streaming
    // encoder carries no content-size header, so the decoder cannot know the
    // output size ahead of time. The fix decompresses incrementally instead of
    // pre-allocating MAX_PATCH_SIZE, so this must still round-trip.
    use std::io::Write;
    let old_data = b"Hello, World!";
    let new_data = b"Hello, Deno!";
    let mut raw_patch = Vec::new();
    bsdiff::diff(old_data, new_data, &mut raw_patch).unwrap();

    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 19).unwrap();
    encoder.write_all(&raw_patch).unwrap();
    let wrapped = encoder.finish().unwrap();
    assert!(wrapped.starts_with(&ZSTD_MAGIC));

    let result = apply_bsdiff_patch(old_data, &wrapped).unwrap();
    assert_eq!(result, new_data);
  }

  #[test]
  fn test_bsdiff_patch_is_compressed() {
    // Regression test for #34312: the raw `bsdiff` crate emits uncompressed
    // control/diff/extra streams that are routinely larger than the new
    // binary itself. The release helper (`tools/bsdiff_helper`) wraps the
    // output in zstd. This test asserts that wrapping is effective — the
    // compressed patch between two near-identical "binaries" must be much
    // smaller than either side.
    //
    // We use a 256 KiB pseudo-binary that differs by ~64 bytes between
    // versions. The raw bsdiff output for this is dominated by the
    // unchanged "extra" stream and balloons close to the binary size;
    // zstd squashes that runs-of-zeros payload to a few hundred bytes.
    let mut binary_v1 = Vec::with_capacity(256 * 1024);
    for i in 0..(256 * 1024) {
      binary_v1.push((i % 251) as u8);
    }
    let mut binary_v2 = binary_v1.clone();
    for byte in binary_v2.iter_mut().skip(100_000).take(64) {
      *byte = byte.wrapping_add(1);
    }

    let mut raw_patch = Vec::new();
    bsdiff::diff(&binary_v1, &binary_v2, &mut raw_patch).unwrap();
    let wrapped = zstd::bulk::compress(&raw_patch, 19).unwrap();

    // The patch is what users actually download; it must be substantially
    // smaller than the binary, otherwise delta upgrade is a pessimization.
    assert!(
      wrapped.len() < binary_v2.len() / 4,
      "compressed patch ({} bytes) should be far smaller than the new \
       binary ({} bytes); raw bsdiff was {} bytes",
      wrapped.len(),
      binary_v2.len(),
      raw_patch.len(),
    );

    // And it must still apply.
    let result = apply_bsdiff_patch(&binary_v1, &wrapped).unwrap();
    assert_eq!(result, binary_v2);
  }

  #[test]
  fn test_compute_sha256() {
    let data = b"test data";
    let hash = compute_sha256(data);
    assert_eq!(hash.len(), 64);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    // Known SHA-256 of "test data"
    assert_eq!(
      hash,
      "916f0027a575074ce72a331777c3478d6513f786a591bd892da1a577bf2335f9"
    );
  }

  #[test]
  fn test_chained_bsdiff_patches() {
    // Simulate a 3-step upgrade: 2.7.10 -> 2.7.11 -> 2.7.12 -> 2.7.13
    // Use realistic-ish binary content that differs slightly per version
    let base = b"DENO_BINARY_v2.7.10_padding_to_simulate_real_binary_size_";
    let mut binary_v10 = base.to_vec();
    binary_v10.extend_from_slice(&[0u8; 1024]);

    let mut binary_v11 = binary_v10.clone();
    // Simulate code changes: modify some bytes
    binary_v11[20..26].copy_from_slice(b"2.7.11");
    binary_v11[500..510].copy_from_slice(b"new_code_1");

    let mut binary_v12 = binary_v11.clone();
    binary_v12[20..26].copy_from_slice(b"2.7.12");
    binary_v12[700..710].copy_from_slice(b"new_code_2");

    let mut binary_v13 = binary_v12.clone();
    binary_v13[20..26].copy_from_slice(b"2.7.13");
    binary_v13[900..910].copy_from_slice(b"new_code_3");

    // Generate consecutive delta patches
    let mut patch_10_11 = Vec::new();
    bsdiff::diff(&binary_v10, &binary_v11, &mut patch_10_11).unwrap();
    let mut patch_11_12 = Vec::new();
    bsdiff::diff(&binary_v11, &binary_v12, &mut patch_11_12).unwrap();
    let mut patch_12_13 = Vec::new();
    bsdiff::diff(&binary_v12, &binary_v13, &mut patch_12_13).unwrap();

    // Chain: apply patches sequentially starting from v10
    let result_v11 = apply_bsdiff_patch(&binary_v10, &patch_10_11).unwrap();
    assert_eq!(result_v11, binary_v11, "v10 -> v11 patch failed");

    let result_v12 = apply_bsdiff_patch(&result_v11, &patch_11_12).unwrap();
    assert_eq!(result_v12, binary_v12, "v11 -> v12 patch failed");

    let result_v13 = apply_bsdiff_patch(&result_v12, &patch_12_13).unwrap();
    assert_eq!(result_v13, binary_v13, "v12 -> v13 patch failed");

    // Verify patches are reasonable size (bsdiff has overhead for small
    // inputs, so just check they're not wildly larger)
    assert!(
      patch_10_11.len() < binary_v10.len() * 2,
      "patch should not be much larger than binary: {} vs {}",
      patch_10_11.len(),
      binary_v10.len()
    );
  }

  #[test]
  fn test_chained_patches_checksum_verification() {
    // Verify that applying a wrong patch produces a different checksum,
    // which would be caught by the per-step verification in
    // try_delta_upgrade
    let binary_a = b"binary_version_a_content_here_padding";
    let binary_b = b"binary_version_b_content_here_padding";
    let binary_c = b"binary_version_c_content_here_padding";

    let mut patch_ab = Vec::new();
    bsdiff::diff(binary_a, binary_b, &mut patch_ab).unwrap();
    let mut patch_bc = Vec::new();
    bsdiff::diff(binary_b, binary_c, &mut patch_bc).unwrap();

    // Correct chain: A -> B -> C
    let result_b = apply_bsdiff_patch(binary_a, &patch_ab).unwrap();
    let hash_b = compute_sha256(&result_b);
    let expected_hash_b = compute_sha256(binary_b);
    assert_eq!(hash_b, expected_hash_b, "intermediate hash should match");

    let result_c = apply_bsdiff_patch(&result_b, &patch_bc).unwrap();
    let hash_c = compute_sha256(&result_c);
    let expected_hash_c = compute_sha256(binary_c);
    assert_eq!(hash_c, expected_hash_c, "final hash should match");

    // Wrong order: applying B->C patch to A should produce wrong result
    // (bsdiff will produce SOMETHING but it won't match)
    let wrong_result = apply_bsdiff_patch(binary_a, &patch_bc).unwrap();
    let wrong_hash = compute_sha256(&wrong_result);
    assert_ne!(
      wrong_hash, expected_hash_c,
      "wrong patch order should produce different hash"
    );
  }

  #[test]
  fn test_delta_url_uses_target_release() {
    // Verify that the delta URL downloads from the TARGET version's
    // release (not the source), since patches live on the new release
    let url = get_delta_download_url("2.7.10", "2.7.11").unwrap();
    assert!(
      url.to_string().contains("/download/v2.7.11/"),
      "delta should be fetched from target release: {url}"
    );
    assert!(
      url.to_string().contains(".from-2.7.10.bsdiff"),
      "delta filename should reference source version: {url}"
    );

    // For a chain 2.7.10 -> 2.7.12, the intermediate step should
    // fetch from v2.7.11's release
    let chain = build_delta_chain("2.7.10", "2.7.12");
    assert_eq!(chain.len(), 2);
    let url_step1 = get_delta_download_url(&chain[0].0, &chain[0].1).unwrap();
    assert!(
      url_step1.to_string().contains("/download/v2.7.11/"),
      "step 1 should fetch from v2.7.11 release"
    );
    let url_step2 = get_delta_download_url(&chain[1].0, &chain[1].1).unwrap();
    assert!(
      url_step2.to_string().contains("/download/v2.7.12/"),
      "step 2 should fetch from v2.7.12 release"
    );
  }
}
