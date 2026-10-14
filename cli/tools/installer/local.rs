// Copyright 2018-2026 the Deno authors. MIT license.

use std::sync::Arc;

use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::url::Url;
use ende_npm::NpmPackageId;

use super::InstallReporter;
use super::print_install_report;
use crate::args::AddFlags;
use crate::args::CiFlags;
use crate::args::Flags;
use crate::args::InstallFlagsLocal;
use crate::args::InstallTopLevelFlags;
use crate::factory::CliFactory;
use crate::npm::CliNpmResolver;

pub async fn install_local(
  flags: Arc<Flags>,
  install_flags: InstallFlagsLocal,
) -> Result<(), AnyError> {
  match install_flags {
    InstallFlagsLocal::Add(add_flags) => {
      crate::tools::pm::add(
        flags,
        add_flags,
        crate::tools::pm::AddCommandName::Install,
      )
      .await
    }
    InstallFlagsLocal::Entrypoints(entrypoints) => {
      super::install_from_entrypoints(flags, entrypoints).await
    }
    InstallFlagsLocal::TopLevel(top_level_flags) => {
      install_top_level(flags, top_level_flags).await
    }
  }
}

/// The top-level npm packages (dependencies of JSR packages) that were
/// installed, sorted by name.
pub fn installed_npm_deps(
  npm_resolver: &CliNpmResolver,
  install_reporter: &InstallReporter,
) -> Vec<NpmPackageId> {
  let Some(managed_resolver) = npm_resolver.as_managed() else {
    return Vec::new();
  };
  let snapshot = managed_resolver.resolution().snapshot();
  let npm_installed_set = if npm_resolver.root_node_modules_path().is_some() {
    &install_reporter.stats.intialized_npm
  } else {
    &install_reporter.stats.downloaded_npm
  };
  let mut installed = snapshot
    .top_level_packages()
    .filter(|pkg| npm_installed_set.contains(&pkg.nv.to_string()))
    .cloned()
    .collect::<Vec<_>>();
  installed.sort_by(|a, b| a.nv.name.cmp(&b.nv.name));
  installed
}

async fn install_top_level(
  flags: Arc<Flags>,
  top_level_flags: InstallTopLevelFlags,
) -> Result<(), AnyError> {
  let start_instant = std::time::Instant::now();
  let factory = CliFactory::from_flags(flags);

  // the actual work
  crate::tools::pm::cache_top_level_deps(
    &factory,
    None,
    crate::tools::pm::CacheTopLevelDepsOptions {
      lockfile_only: top_level_flags.lockfile_only,
      additional_roots: vec![],
    },
  )
  .await?;

  if let Some(lockfile) = factory.maybe_lockfile().await? {
    lockfile.write_if_changed()?;
  }

  let install_reporter = factory.install_reporter()?.unwrap().clone();
  let npm_resolver = factory.npm_resolver().await?;
  print_install_report(
    start_instant.elapsed(),
    &install_reporter,
    npm_resolver,
  );

  Ok(())
}

pub async fn ci_command(
  flags: Arc<Flags>,
  _ci_flags: CiFlags,
) -> Result<(), AnyError> {
  let factory = CliFactory::from_flags(flags.clone());
  if factory.maybe_lockfile().await?.is_none() {
    bail!(
      "ende ci requires a lockfile, but none was found.\n  hint: run `ende install` to create one."
    );
  }
  if let Some(node_modules_dir) = factory.node_modules_dir_path()?
    && node_modules_dir.exists()
  {
    log::info!(
      "{} {}",
      deno_terminal::colors::gray("Removing"),
      node_modules_dir.display()
    );
    std::fs::remove_dir_all(node_modules_dir).map_err(|err| {
      ende_core::anyhow::anyhow!(
        "failed to remove {}: {err}",
        node_modules_dir.display()
      )
    })?;
  }
  drop(factory);
  install_top_level(
    flags,
    InstallTopLevelFlags {
      lockfile_only: false,
    },
  )
  .await
}

pub fn check_if_installs_a_single_package_globally(
  maybe_add_flags: Option<&AddFlags>,
) -> Result<(), AnyError> {
  let Some(add_flags) = maybe_add_flags else {
    return Ok(());
  };
  if add_flags.packages.len() != 1 {
    return Ok(());
  }
  let Ok(url) = Url::parse(&add_flags.packages[0]) else {
    return Ok(());
  };
  if matches!(url.scheme(), "http" | "https") {
    bail!(
      "Failed to install \"{}\" specifier. If you are trying to install {} globally, run again with `-g` flag:\n  ende install -g {}",
      url.scheme(),
      url.as_str(),
      url.as_str()
    );
  }
  Ok(())
}
