// Copyright 2018-2026 the Deno authors. MIT license.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno_semver::jsr::JsrPackageReqReference;
use deno_semver::npm::NpmPackageReqReference;
use deno_semver::package::PackageReq;
use ende_cache_dir::file_fetcher::CacheSetting;
use ende_config::ende_json::MinimumDependencyAgeConfig;
use ende_config::ende_json::NewestDependencyDate;
use ende_core::anyhow;
use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_core::serde_json;
use ende_npm_installer::PackagesAllowedScripts;
use ende_runtime::ende_permissions::PathQueryDescriptor;
use node_resolver::BinValue;

use crate::args::CliOptions;
use crate::args::ConfigFlag;
use crate::args::EndeXShimName;
use crate::args::Flags;
use crate::args::FlagsExt;
use crate::args::XFlags;
use crate::args::XFlagsKind;
use crate::factory::CliFactory;
use crate::node::CliNodeResolver;
use crate::npm::CliManagedNpmResolver;
use crate::npm::CliNpmResolver;
use crate::tools::pm::CacheTopLevelDepsOptions;
use crate::util::console::ConfirmOptions;
use crate::util::console::confirm;
use crate::util::draw_thread::DrawThread;
use crate::util::fs::canonicalize_path;

/// Renders the `--minimum-dependency-age` argument for the re-run `ende run`
/// subprocess, but only when the user passed the flag EXPLICITLY on the CLI.
///
/// `ende x` executes the target as an `ende run <target>` subprocess in the
/// user's cwd, so it re-discovers the cwd `ende.json` `minimumDependencyAge`
/// (including its `exclude`) and `.npmrc` policy exactly as a plain
/// `ende run <target>` would. The only thing that subprocess can't observe is
/// an explicit CLI flag, so we forward just that — rendering the EXPLICIT value
/// (never a computed effective policy). Forwarding a config-derived scalar
/// would override the natively-discovered policy and drop its `exclude`,
/// diverging from `ende run` (issue #35991). An explicit flag carries
/// empty-`exclude` semantics, same as `ende run --minimum-dependency-age=...`.
fn minimum_dependency_age_flag_arg(flags: &Flags) -> Option<String> {
  let value = match flags.minimum_dependency_age? {
    NewestDependencyDate::Disabled => "0".to_string(),
    NewestDependencyDate::Enabled(date) => date.to_rfc3339(),
  };
  Some(format!("--minimum-dependency-age={}", value))
}

/// Renders the user's explicit `--config`/`--no-config` selection for the re-run
/// `ende run <target>` subprocess argv.
///
/// The subprocess re-resolves the target from the user's cwd, so it only
/// auto-discovers a cwd `ende.json`. When the user selected a config that is NOT
/// auto-discoverable (an explicit `--config=<path>` outside cwd), that policy —
/// including any `minimumDependencyAge` — would be dropped by the subprocess,
/// diverging from `ende run --config=<path> <target>` (issue #35991). Forward it:
/// - `Path(p)` -> `--config=<abs>` (made absolute, but WITHOUT resolving
///   symlinks, so it is robust regardless of the child's cwd).
/// - `Disabled` -> `--no-config`.
/// - `Discover` -> nothing; the subprocess auto-discovers the cwd config itself.
fn config_flag_arg(flags: &Flags, initial_cwd: &Path) -> Option<String> {
  match &flags.config_flag {
    ConfigFlag::Discover => None,
    ConfigFlag::Disabled => Some("--no-config".to_string()),
    ConfigFlag::Path(path) => {
      let path = Path::new(path);
      let abs = if path.is_absolute() {
        std::borrow::Cow::Borrowed(path)
      } else {
        std::borrow::Cow::Owned(initial_cwd.join(path))
      };
      // Make the path absolute WITHOUT resolving symlinks (lexical normalization
      // only). Canonicalizing would resolve a symlinked config to its target, so
      // relative config members (permission set paths, `imports`, lock path,
      // ...) would then resolve relative to the target's directory instead of
      // the path the user supplied — diverging from `ende run --config=<path>`.
      let abs = deno_path_util::normalize_path(abs);
      Some(format!("--config={}", abs.display()))
    }
  }
}

/// Like [`config_flag_arg`], but for re-run sites whose child does NOT
/// auto-discover the user's cwd `ende.json`, so an original `ConfigFlag::Discover`
/// must be resolved to the discovered config path and forwarded explicitly. Two
/// such sites (issue #35991):
/// - the npm auto-install re-run, which executes the *installed bin file* (under
///   `ende_dir/ende_x_cache/...`): its config auto-discovery anchors on the
///   installed file's directory, NEVER the user's cwd (it finds the internal
///   temp `ende.json` instead).
/// - the jsr/URL re-run, which executes a `jsr:`/`data:`/`http(s):` main module:
///   `ende run` disables config discovery entirely for a non-`file:`/non-`npm:`
///   entrypoint, so the child would drop the cwd policy (`minimumDependencyAge`,
///   permission sets, ...) that the isolated install already honored.
///
/// Resolving `Discover` to the discovered config keeps install and re-run
/// consistent and matches applying the user's cwd policy. Explicit `Path`/
/// `Disabled` selections are forwarded exactly as [`config_flag_arg`] does.
fn config_flag_arg_carrying_discovered(
  flags: &Flags,
  cli_options: &crate::args::CliOptions,
) -> Option<String> {
  if matches!(flags.config_flag, ConfigFlag::Discover) {
    return cli_options
      .start_dir
      .member_or_root_ende_json()
      .and_then(|config| {
        deno_path_util::url_to_file_path(&config.specifier).ok()
      })
      .map(|path| format!("--config={}", path.display()));
  }
  config_flag_arg(flags, cli_options.initial_cwd())
}

/// Renders the user's explicit `-P`/`--permission-set=<name>` selection for the
/// re-run `ende run <target>` subprocess argv.
///
/// A permission set is resolved from the (discovered or explicit) config file at
/// runtime and is NOT baked into `flags.permissions`, so `to_permission_args()`
/// does not forward it. Without forwarding the selection, the subprocess would
/// run with no permission set and fail with `NotCapable`, diverging from
/// `ende run --permission-set=<name> <target>` (issue #35991). A bare `-P`
/// resolves to `Some("")` (the default set), which round-trips as
/// `--permission-set=`.
fn permission_set_flag_arg(flags: &Flags) -> Option<String> {
  flags
    .permission_set
    .as_ref()
    .map(|name| format!("--permission-set={}", name))
}

/// Computes the extra `ende run` arguments that carry the user's ORIGINAL CLI
/// selections into a re-run subprocess argv: the explicit
/// `--minimum-dependency-age` flag, the `--config`/`--no-config` selection
/// (passed in as `config_arg`, rendered by `config_flag_arg_carrying_discovered`),
/// and the `-P`/`--permission-set` selection.
///
/// These MUST be computed from the user's original `flags`/`cli_options`, never
/// from the internal runner flags used for npm auto-install: those overwrite
/// `config_flag` with a generated temp `ende.json` path (see
/// `autoinstall_package::make_new_flags`), which would forward the internal
/// config instead of the user's explicit `--config` — or forward a config at all
/// when the user relied on auto-discovery (issue #35991).
fn forwarded_run_args(
  flags: &Flags,
  config_arg: Option<String>,
) -> Vec<String> {
  let mut args = Vec::new();
  if let Some(arg) = minimum_dependency_age_flag_arg(flags) {
    args.push(arg);
  }
  if let Some(arg) = config_arg {
    args.push(arg);
  }
  if let Some(arg) = permission_set_flag_arg(flags) {
    args.push(arg);
  }
  args
}

/// Serializes the effective minimum dependency age policy (age + exclude) into
/// the object form understood by `to_minimum_dependency_age_config`, so the
/// full policy can be written into the generated temp `ende.json` used for
/// installing. The temp workspace uses a config override and therefore can't
/// see the user's cwd config, so the policy must travel with it.
fn minimum_dependency_age_config_json(
  config: &MinimumDependencyAgeConfig,
) -> Option<serde_json::Value> {
  let age = match config.age? {
    NewestDependencyDate::Disabled => "0".to_string(),
    NewestDependencyDate::Enabled(date) => date.to_rfc3339(),
  };
  Some(serde_json::json!({
    "age": age,
    "exclude": config.exclude,
  }))
}

async fn resolve_local_bins(
  node_resolver: &CliNodeResolver,
  npm_resolver: &CliNpmResolver,
  factory: &CliFactory,
) -> Result<BTreeMap<String, BinValue>, AnyError> {
  match &npm_resolver {
    ende_resolver::npm::NpmResolver::Byonm(npm_resolver) => {
      let node_modules_dir = npm_resolver.root_node_modules_path().unwrap();
      let bin_dir = node_modules_dir.join(".bin");
      Ok(node_resolver.resolve_npm_commands_from_bin_dir(&bin_dir))
    }
    ende_resolver::npm::NpmResolver::Managed(npm_resolver) => {
      let mut all_bins = BTreeMap::new();
      for id in npm_resolver.resolution().top_level_packages() {
        let package_folder =
          npm_resolver.resolve_pkg_folder_from_pkg_id(&id)?;
        let bins = match node_resolver
          .resolve_npm_binary_commands_for_package(&package_folder)
        {
          Ok(bins) => bins,
          Err(_) => {
            crate::tools::pm::cache_top_level_deps(
              factory,
              None,
              CacheTopLevelDepsOptions {
                lockfile_only: false,
                additional_roots: vec![],
              },
            )
            .await?;
            node_resolver
              .resolve_npm_binary_commands_for_package(&package_folder)?
          }
        };
        for (command, bin_value) in bins {
          all_bins.insert(command.clone(), bin_value.clone());
        }
      }
      Ok(all_bins)
    }
  }
}

fn run_js_file(
  main_module: &ende_core::url::Url,
  ende_args: &[String],
  argv: &[String],
  npm_process_state: Option<String>,
  npm: bool,
) -> Result<i32, AnyError> {
  use ende_runtime::ende_io::FromRawIoHandle;

  let ende_exe = std::env::current_exe()
    .and_then(|p| canonicalize_path(&p))
    .context("Failed to get current executable path")?;

  let mut args: Vec<std::ffi::OsString> = vec!["run".into()];
  args.extend(ende_args.iter().map(|s| s.into()));
  args.push(main_module.as_str().into());
  args.extend(argv.iter().map(|s| s.into()));

  let mut command = std::process::Command::new(ende_exe);
  command.args(&args);

  let _temp_file = if let Some(state) = &npm_process_state {
    let fd =
      ende_runtime::ende_process::npm_process_state_tempfile(state.as_bytes())
        .context("Failed to create npm process state tempfile")?;
    command.env(
      ende_runtime::ende_process::NPM_RESOLUTION_STATE_FD_ENV_VAR_NAME,
      (fd as usize).to_string(),
    );
    // Keep the file alive until the subprocess completes
    // SAFETY: fd is valid
    Some(unsafe { std::fs::File::from_raw_io_handle(fd) })
  } else {
    None
  };

  if npm {
    crate::tools::run::set_npm_user_agent();
  }

  #[cfg(unix)]
  {
    use std::os::unix::process::CommandExt;
    Err(command.exec().into())
  }
  #[cfg(not(unix))]
  {
    let mut child =
      command.spawn().context("Failed to spawn ende subprocess")?;
    let status = child.wait().context("Failed to wait for ende subprocess")?;
    Ok(status.code().unwrap_or(1))
  }
}

pub(crate) fn get_npm_process_state(
  npm_resolver: &CliNpmResolver,
) -> Option<String> {
  match npm_resolver {
    ende_resolver::npm::NpmResolver::Managed(managed) => {
      let linker_mode = match managed.linker_mode() {
        ende_config::ende_json::NodeModulesLinkerMode::Hoisted => {
          ende_npm_installer::process_state::NpmProcessStateLinkerMode::Hoisted
        }
        ende_config::ende_json::NodeModulesLinkerMode::Isolated => {
          ende_npm_installer::process_state::NpmProcessStateLinkerMode::Isolated
        }
      };
      Some(
        ende_npm_installer::process_state::NpmProcessState::new_managed(
          managed.resolution().serialized_valid_snapshot(),
          managed.root_node_modules_path(),
          linker_mode,
        )
        .as_serialized(),
      )
    }
    ende_resolver::npm::NpmResolver::Byonm(_) => None,
  }
}

pub(crate) fn run_bin_value(
  factory: &CliFactory,
  flags: &Flags,
  bin_value: BinValue,
  npm_process_state: Option<String>,
  unstable_args: &[String],
  // Extra `ende run` args carrying the user's ORIGINAL CLI selections (config,
  // minimum-dependency-age, permission-set). Computed by the caller from the
  // original flags/cwd via `forwarded_run_args`, NOT from `flags` here: in the
  // npm auto-install path `flags` is the internal runner flags whose
  // `config_flag` points at a generated temp `ende.json` (issue #35991).
  forwarded_args: &[String],
) -> Result<i32, AnyError> {
  match bin_value {
    BinValue::JsFile(path_buf) => {
      let path = deno_path_util::url_from_file_path(path_buf.as_ref())?;
      let mut ende_args = flags.to_permission_args();
      ende_args.extend(unstable_args.iter().cloned());
      ende_args.extend(forwarded_args.iter().cloned());

      run_js_file(&path, &ende_args, &flags.argv, npm_process_state, true)
    }
    BinValue::Executable(mut path_buf) => {
      if cfg!(windows) && path_buf.extension().is_none() {
        // prefer cmd shim over sh
        path_buf.set_extension("cmd");
        if !path_buf.exists() {
          //  just fall back to original path
          path_buf.set_extension("");
        }
      }
      let permissions = factory.root_permissions_container()?;
      permissions.check_run(
        &ende_runtime::ende_permissions::RunQueryDescriptor::Path(
          PathQueryDescriptor::new(
            &factory.sys(),
            std::borrow::Cow::Borrowed(path_buf.as_ref()),
          )?,
        ),
        "entrypoint",
      )?;
      let mut child = std::process::Command::new(path_buf)
        .args(&flags.argv)
        .spawn()
        .context("Failed to spawn command")?;
      let status = child.wait()?;
      Ok(status.code().unwrap_or(1))
    }
  }
}

/// Try to find a bin value from a map of bins, with fallbacks for scoped package names
/// and single-bin packages.
pub(crate) fn find_bin_value(
  bins: &BTreeMap<String, BinValue>,
  bin_name: &str,
) -> Option<BinValue> {
  bins
    .get(bin_name)
    .or_else(|| {
      // Try the package name without scope as fallback
      if bin_name.starts_with('@') && bin_name.contains('/') {
        bin_name
          .split('/')
          .next_back()
          .and_then(|name| bins.get(name))
      } else {
        None
      }
    })
    .or_else(|| {
      // If there's only one bin, use it
      if bins.len() == 1 {
        bins.values().next()
      } else {
        None
      }
    })
    .cloned()
}

async fn maybe_run_local_npm_bin(
  factory: &CliFactory,
  flags: &Flags,
  node_resolver: &CliNodeResolver,
  npm_resolver: &CliNpmResolver,
  command: &str,
  unstable_args: &[String],
  forwarded_args: &[String],
) -> Result<Option<i32>, AnyError> {
  let mut bins =
    resolve_local_bins(node_resolver, npm_resolver, factory).await?;
  let bin_value = if let Some(bin_value) = bins.remove(command) {
    bin_value
  } else if let Some(bin_value) = {
    let command = if command.starts_with("@") && command.contains("/") {
      command.split("/").last().unwrap()
    } else {
      command
    };
    bins.remove(command)
  } {
    bin_value
  } else {
    return Ok(None);
  };

  let npm_process_state = get_npm_process_state(npm_resolver);
  run_bin_value(
    factory,
    flags,
    bin_value,
    npm_process_state,
    unstable_args,
    forwarded_args,
  )
  .map(Some)
}

enum XTempDir {
  Existing(PathBuf),
  New(PathBuf),
}
impl XTempDir {
  fn path(&self) -> &PathBuf {
    match self {
      XTempDir::Existing(path) => path,
      XTempDir::New(path) => path,
    }
  }
}

fn create_package_temp_dir(
  prefix: Option<&str>,
  package_req: &PackageReq,
  reload: bool,
  ende_dir: &Path,
  min_dep_age_config: &MinimumDependencyAgeConfig,
) -> Result<XTempDir, AnyError> {
  let mut package_req_folder = String::from(prefix.unwrap_or(""));
  package_req_folder.push_str(
    &package_req
      .to_string()
      .replace("/", "__")
      .replace(">", "gt")
      .replace("<", "lt"),
  );
  let temp_dir = ende_dir.join("ende_x_cache").join(package_req_folder);
  if temp_dir.exists() {
    if reload
      || !temp_dir
        .join(ende_config::ende_json::LOCKFILE_NAME)
        .exists()
    {
      std::fs::remove_dir_all(&temp_dir)?;
    } else {
      let temp_dir = canonicalize_path(&temp_dir).unwrap_or(temp_dir);
      return Ok(XTempDir::Existing(temp_dir));
    }
  }
  std::fs::create_dir_all(&temp_dir)?;
  let package_json_path = temp_dir.join("package.json");
  std::fs::write(&package_json_path, "{}")?;
  let ende_json_path =
    temp_dir.join(ende_config::ende_json::DEFAULT_CONFIG_FILE_NAME);
  let mut ende_json_value = serde_json::json!({});
  if let Some(min_dep_age) =
    minimum_dependency_age_config_json(min_dep_age_config)
  {
    ende_json_value
      .as_object_mut()
      .unwrap()
      .insert("minimumDependencyAge".to_string(), min_dep_age);
  }
  std::fs::write(&ende_json_path, serde_json::to_string(&ende_json_value)?)?;

  let temp_dir = canonicalize_path(&temp_dir).unwrap_or(temp_dir);
  Ok(XTempDir::New(temp_dir))
}

fn write_shim(
  out_dir: &Path,
  shim_name: EndeXShimName,
) -> Result<(), AnyError> {
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let out_path = out_dir.join(shim_name.name());
    if let EndeXShimName::Other(_) = shim_name {
      // Remove any existing entry first: writing through a symlink left by
      // an earlier built-in alias would overwrite the `ende` binary itself.
      match std::fs::remove_file(&out_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
      }
      std::fs::write(
        &out_path,
        r##"#!/bin/sh
SCRIPT_DIR="$(dirname -- "$(readlink -f -- "$0")")"
exec "$SCRIPT_DIR/ende" x "$@"
"##
          .as_bytes(),
      )?;
      let mut permissions = std::fs::metadata(&out_path)?.permissions();
      permissions.set_mode(0o755);
      std::fs::set_permissions(&out_path, permissions)?;
    } else {
      match std::os::unix::fs::symlink("./ende", &out_path) {
        Ok(_) => {}
        Err(e) => match e.kind() {
          std::io::ErrorKind::AlreadyExists => {
            std::fs::remove_file(&out_path)?;
            std::os::unix::fs::symlink("./ende", &out_path)?;
          }
          _ => return Err(e.into()),
        },
      }
    }
  }

  #[cfg(windows)]
  {
    let out_path = out_dir.join(format!("{}.cmd", shim_name.name()));
    std::fs::write(
      out_path,
      r##"@echo off
"%~dp0ende.exe" x %*
exit /b %ERRORLEVEL%
"##,
    )?;
  }

  Ok(())
}

pub async fn run(flags: Arc<Flags>, x_flags: XFlags) -> Result<i32, AnyError> {
  let command_flags = match x_flags.kind {
    XFlagsKind::InstallAlias(shim_name) => {
      let exe = std::env::current_exe()?;
      let out_dir = exe.parent().unwrap();
      write_shim(out_dir, shim_name)?;
      return Ok(0);
    }
    XFlagsKind::Command(command) => command,
    XFlagsKind::Print => {
      let factory = CliFactory::from_flags(flags.clone());
      let npm_resolver = factory.npm_resolver().await?;
      let node_resolver = factory.node_resolver().await?;
      let bins =
        resolve_local_bins(node_resolver, npm_resolver, &factory).await?;
      if bins.is_empty() {
        log::info!("No local commands found");
        return Ok(0);
      }
      log::info!("Available (local) commands:");
      for command in bins.keys() {
        log::info!("  {}", command);
      }
      return Ok(0);
    }
  };
  let factory = CliFactory::from_flags(flags.clone());
  let cli_options = factory.cli_options()?;
  let unstable_args = cli_options.unstable_args();
  let npm_resolver = factory.npm_resolver().await?;
  let node_resolver = factory.node_resolver().await?;
  // Extra `ende run` args forwarded to every re-run subprocess, computed ONCE
  // from the user's ORIGINAL flags/cwd (never the internal npm-install runner
  // flags, whose `config_flag` points at a generated temp ende.json). See
  // `forwarded_run_args` (issue #35991). Every `ende x` re-run site executes a
  // target whose child does NOT auto-discover the user's cwd `ende.json` — the
  // installed bin FILE anchors discovery on the install dir, and a jsr:/url main
  // module has config discovery disabled entirely — so an original `Discover` is
  // resolved to the discovered config path and forwarded explicitly, keeping the
  // re-run consistent with the isolated install (see
  // `config_flag_arg_carrying_discovered`).
  let forwarded = forwarded_run_args(
    &flags,
    config_flag_arg_carrying_discovered(&flags, cli_options),
  );
  let result = maybe_run_local_npm_bin(
    &factory,
    &flags,
    node_resolver,
    npm_resolver,
    &command_flags.command,
    &unstable_args,
    &forwarded,
  )
  .await?;
  if let Some(exit_code) = result {
    return Ok(exit_code);
  }

  // When --package is specified, the command is the binary name and
  // the package flag specifies which package to install. Combine them
  // into a single specifier like "npm:package/binary" so the existing
  // resolution flow handles it correctly.
  let effective_command = if let Some(ref package) = command_flags.package {
    format!("{}/{}", package, command_flags.command)
  } else {
    command_flags.command.clone()
  };

  let is_file_like = effective_command.starts_with('.')
    || effective_command.starts_with('/')
    || effective_command.starts_with('~')
    || effective_command.starts_with('\\')
    || Path::new(&effective_command).extension().is_some();
  if is_file_like && Path::new(&effective_command).is_file() {
    return Err(anyhow::anyhow!(
      "Use 'ende run' to run a local file directly, 'ende x' is intended for running commands from packages."
    ));
  }

  let thing_to_run = match ende_core::url::Url::parse(&effective_command) {
    Ok(url) => {
      if url.scheme() == "npm" {
        let req_ref = NpmPackageReqReference::from_specifier(&url)?;
        ReqRefOrUrl::Npm(req_ref)
      } else if url.scheme() == "jsr" {
        let req_ref = JsrPackageReqReference::from_specifier(&url)?;
        ReqRefOrUrl::Jsr(req_ref)
      } else {
        ReqRefOrUrl::Url(url)
      }
    }
    Err(ende_core::url::ParseError::RelativeUrlWithoutBase) => {
      let new_command = format!("npm:{}", effective_command);
      let req_ref = NpmPackageReqReference::from_str(&new_command)?;
      ReqRefOrUrl::Npm(req_ref)
    }
    Err(e) => {
      return Err(e.into());
    }
  };

  let cache_setting = cli_options.cache_setting();
  let reload = matches!(cache_setting, CacheSetting::ReloadAll);
  // Effective minimum dependency age policy (CLI flag > cwd deno.json > .npmrc
  // > default) resolved from the ORIGINAL factory, i.e. the user's cwd config.
  // This full config (age + exclude) is written into the generated temp
  // ende.json used for installing, since that temp workspace can't see the
  // user's cwd config on its own.
  let min_dep_age_config = factory
    .resolver_factory()?
    .minimum_dependency_age_config()?
    .clone();
  match thing_to_run {
    ReqRefOrUrl::Npm(npm_package_req_reference) => {
      // First try to resolve from the local project
      if let Ok(package_folder) = npm_resolver
        .resolve_pkg_folder_from_deno_module_req(
          npm_package_req_reference.req(),
          &deno_path_util::url_from_directory_path(cli_options.initial_cwd())
            .unwrap(),
        )
      {
        let bin_name =
          if let Some(sub_path) = npm_package_req_reference.sub_path() {
            sub_path
          } else {
            npm_package_req_reference.req().name.as_str()
          };

        let bins = node_resolver
          .resolve_npm_binary_commands_for_package(&package_folder)?;
        if let Some(bin_value) = find_bin_value(&bins, bin_name) {
          let npm_process_state = get_npm_process_state(npm_resolver);
          return run_bin_value(
            &factory,
            &flags,
            bin_value,
            npm_process_state,
            &unstable_args,
            &forwarded,
          );
        }
      }

      // Fall back to autoinstall
      let (managed_flags, managed_factory) = autoinstall_package(
        ReqRef::Npm(&npm_package_req_reference),
        &flags,
        reload,
        command_flags.yes,
        &factory.ende_dir()?.root,
        &min_dep_age_config,
        resolve_import_deny_rules(cli_options)?,
      )
      .await?;
      let mut runner_flags = (*managed_flags).clone();
      runner_flags.node_modules_dir =
        Some(ende_config::ende_json::NodeModulesDirMode::Manual);
      let runner_flags = Arc::new(runner_flags);
      let runner_factory = CliFactory::from_flags(runner_flags.clone());
      let runner_node_resolver = runner_factory.node_resolver().await?;
      let runner_npm_resolver = runner_factory.npm_resolver().await?;

      let bin_name =
        if let Some(sub_path) = npm_package_req_reference.sub_path() {
          sub_path
        } else {
          npm_package_req_reference.req().name.as_str()
        };

      let res = maybe_run_local_npm_bin(
        &runner_factory,
        &runner_flags,
        runner_node_resolver,
        runner_npm_resolver,
        bin_name,
        &unstable_args,
        &forwarded,
      )
      .await?;
      if let Some(exit_code) = res {
        Ok(exit_code)
      } else {
        let managed_npm_resolver =
          managed_factory.npm_resolver().await?.as_managed().unwrap();
        let bin_commands = bin_commands_for_package(
          runner_node_resolver,
          managed_npm_resolver,
          npm_package_req_reference.req(),
        )?;
        let fallback_name = if bin_commands.len() == 1 {
          Some(bin_commands.keys().next().unwrap())
        } else {
          None
        };

        if let Some(fallback_name) = fallback_name
          && let Some(exit_code) = maybe_run_local_npm_bin(
            &runner_factory,
            &runner_flags,
            runner_node_resolver,
            runner_npm_resolver,
            fallback_name.as_ref(),
            &unstable_args,
            &forwarded,
          )
          .await?
        {
          return Ok(exit_code);
        }

        Err(anyhow::anyhow!(
          "Unable to choose binary for {}\n  Available bins:\n{}",
          effective_command,
          bin_commands
            .keys()
            .map(|k| format!("    {}", k))
            .collect::<Vec<_>>()
            .join("\n")
        ))
      }
    }
    ReqRefOrUrl::Jsr(jsr_package_req_reference) => {
      let (_new_flags, new_factory) = autoinstall_package(
        ReqRef::Jsr(&jsr_package_req_reference),
        &flags,
        reload,
        command_flags.yes,
        &factory.ende_dir()?.root,
        &min_dep_age_config,
        resolve_import_deny_rules(cli_options)?,
      )
      .await?;

      let npm_resolver = new_factory.npm_resolver().await?;
      let npm_process_state = get_npm_process_state(npm_resolver);

      let mut ende_args = flags.to_permission_args();
      ende_args.extend(unstable_args.iter().cloned());
      ende_args.extend(forwarded.iter().cloned());
      let url =
        ende_core::url::Url::parse(&jsr_package_req_reference.to_string())?;
      run_js_file(&url, &ende_args, &flags.argv, npm_process_state, false)
    }
    ReqRefOrUrl::Url(url) => {
      let mut ende_args = flags.to_permission_args();
      ende_args.extend(unstable_args.iter().cloned());
      ende_args.extend(forwarded.iter().cloned());
      run_js_file(&url, &ende_args, &flags.argv, None, false)
    }
  }
}

fn bin_commands_for_package(
  node_resolver: &CliNodeResolver,
  managed_npm_resolver: &CliManagedNpmResolver,
  package_req: &PackageReq,
) -> Result<BTreeMap<String, BinValue>, AnyError> {
  let pkg_id =
    managed_npm_resolver.resolve_pkg_id_from_deno_module_req(package_req)?;
  let package_folder =
    managed_npm_resolver.resolve_pkg_folder_from_pkg_id(&pkg_id)?;
  node_resolver
    .resolve_npm_binary_commands_for_package(&package_folder)
    .map_err(Into::into)
}

/// The user's `--deny-import` rules, resolved against the config the user
/// actually selected.
///
/// The auto-install below runs against a generated temp `ende.json`, which
/// cannot see a `-P` set defined in the user's config. Resolving the rules
/// against the user's own options and baking them into the internal flags
/// keeps the deny list — which decides what code may be downloaded at all —
/// in force for that install, and lets
/// `import_deny_permissions_options` treat an unresolvable set as the error it
/// is everywhere else.
fn resolve_import_deny_rules(
  cli_options: &CliOptions,
) -> Result<Option<Vec<String>>, AnyError> {
  Ok(cli_options.import_deny_permissions_options()?.deny_import)
}

#[allow(clippy::too_many_arguments, reason = "internal re-run plumbing")]
async fn autoinstall_package(
  req_ref: ReqRef<'_>,
  old_flags: &Flags,
  reload: bool,
  yes: bool,
  ende_dir: &Path,
  min_dep_age_config: &MinimumDependencyAgeConfig,
  import_deny_rules: Option<Vec<String>>,
) -> Result<(Arc<Flags>, CliFactory), AnyError> {
  fn make_new_flags(
    old_flags: &Flags,
    temp_dir: &Path,
    import_deny_rules: Option<Vec<String>>,
  ) -> Arc<Flags> {
    let mut new_flags = (*old_flags).clone();
    // The generated config below defines no permission sets, so the selection
    // is dropped in favour of the rules it resolved to.
    new_flags.permissions.deny_import = import_deny_rules;
    new_flags.permission_set = None;
    new_flags.node_modules_dir =
      Some(ende_config::ende_json::NodeModulesDirMode::Auto);
    let temp_node_modules = temp_dir.join("node_modules");
    new_flags.internal.root_node_modules_dir_override = Some(temp_node_modules);
    new_flags.config_flag = crate::args::ConfigFlag::Path(
      temp_dir
        .join(ende_config::ende_json::DEFAULT_CONFIG_FILE_NAME)
        .to_string_lossy()
        .into_owned(),
    );
    if matches!(old_flags.allow_scripts, PackagesAllowedScripts::None) {
      new_flags.allow_scripts = PackagesAllowedScripts::All;
    }

    log::debug!("new_flags: {:?}", new_flags);

    Arc::new(new_flags)
  }
  let temp_dir = create_package_temp_dir(
    Some(req_ref.prefix()),
    req_ref.req(),
    reload,
    ende_dir,
    min_dep_age_config,
  )?;

  let new_flags = make_new_flags(old_flags, temp_dir.path(), import_deny_rules);
  let new_factory = CliFactory::from_flags(new_flags.clone());

  match temp_dir {
    XTempDir::Existing(_) => Ok((new_flags, new_factory)),

    XTempDir::New(temp_dir) => {
      let confirmed = yes
        || match confirm(ConfirmOptions {
          default: true,
          message: format!("Install {}?", req_ref),
        }) {
          Some(true) => true,
          Some(false) => false,
          None if !DrawThread::is_supported() => {
            log::warn!(
              "Unable to prompt, installing {} without confirmation",
              req_ref.req()
            );
            true
          }
          None => false,
        };
      if !confirmed {
        return Err(anyhow::anyhow!("Installation rejected"));
      }
      match req_ref {
        ReqRef::Npm(req_ref) => {
          let pkg_json = temp_dir.join("package.json");
          std::fs::write(
            &pkg_json,
            format!(
              "{{\"dependencies\": {{\"{}\": \"{}\"}} }}",
              req_ref.req().name,
              req_ref.req().version_req
            ),
          )?;
        }
        ReqRef::Jsr(req_ref) => {
          let ende_json =
            temp_dir.join(ende_config::ende_json::DEFAULT_CONFIG_FILE_NAME);
          let mut imports = serde_json::Map::new();
          imports.insert(
            req_ref.req().name.to_string(),
            serde_json::Value::String(format!("jsr:{}", req_ref.req())),
          );
          let mut ende_json_value = serde_json::json!({
            "imports": imports,
          });
          if let Some(min_dep_age) =
            minimum_dependency_age_config_json(min_dep_age_config)
          {
            ende_json_value
              .as_object_mut()
              .unwrap()
              .insert("minimumDependencyAge".to_string(), min_dep_age);
          }
          std::fs::write(&ende_json, serde_json::to_string(&ende_json_value)?)?;
        }
      }

      crate::tools::pm::cache_top_level_deps(
        &new_factory,
        None,
        CacheTopLevelDepsOptions {
          lockfile_only: false,
          additional_roots: vec![],
        },
      )
      .await?;

      if let Some(lockfile) = new_factory.maybe_lockfile().await? {
        lockfile.write_if_changed()?;
      }
      Ok((new_flags, new_factory))
    }
  }
}

#[derive(Debug, Clone, Copy)]
enum ReqRef<'a> {
  Npm(&'a NpmPackageReqReference),
  Jsr(&'a JsrPackageReqReference),
}
impl<'a> std::fmt::Display for ReqRef<'a> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      ReqRef::Npm(req) => write!(f, "{}", req),
      ReqRef::Jsr(req) => write!(f, "{}", req),
    }
  }
}
impl<'a> ReqRef<'a> {
  fn req(&self) -> &PackageReq {
    match self {
      ReqRef::Npm(req) => req.req(),
      ReqRef::Jsr(req) => req.req(),
    }
  }

  fn prefix(&self) -> &str {
    match self {
      ReqRef::Npm(_) => "npm-",
      ReqRef::Jsr(_) => "jsr-",
    }
  }
}

enum ReqRefOrUrl {
  Npm(NpmPackageReqReference),
  Jsr(JsrPackageReqReference),
  Url(ende_core::url::Url),
}

#[cfg(all(test, unix))]
mod tests {
  use test_util::TempDir;

  use super::*;

  #[test]
  fn write_shim_builtin_name_is_symlink() {
    let temp_dir = TempDir::new();
    let dir = temp_dir.path().as_path();
    write_shim(dir, EndeXShimName::Endex).unwrap();
    let link = dir.join("endex");
    assert!(link.symlink_metadata().unwrap().is_symlink());
    assert_eq!(std::fs::read_link(link).unwrap(), Path::new("./ende"));
  }

  #[test]
  fn write_shim_other_name_is_script() {
    let temp_dir = TempDir::new();
    let dir = temp_dir.path().as_path();
    write_shim(dir, EndeXShimName::Other("denox".to_string())).unwrap();
    let shim = dir.join("denox");
    assert!(shim.symlink_metadata().unwrap().is_file());
    assert!(
      std::fs::read_to_string(shim)
        .unwrap()
        .contains("exec \"$SCRIPT_DIR/ende\" x \"$@\"")
    );
  }

  /// A `denox` symlink installed when `denox` was a built-in name must be
  /// replaced, not written through (which would overwrite the binary).
  #[test]
  fn write_shim_script_replaces_existing_symlink() {
    let temp_dir = TempDir::new();
    temp_dir.write("ende", "binary");
    temp_dir.symlink_file("ende", "denox");
    let dir = temp_dir.path().as_path();
    write_shim(dir, EndeXShimName::Other("denox".to_string())).unwrap();
    assert_eq!(temp_dir.read_to_string("ende"), "binary");
    assert!(dir.join("denox").symlink_metadata().unwrap().is_file());
  }
}
