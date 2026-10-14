// Copyright 2018-2026 the Deno authors. MIT license.

use std::sync::Arc;

use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::url::Url;

use super::print_install_report;
use crate::args::AddFlags;
use crate::args::CiFlags;
use crate::args::Flags;
use crate::args::InstallFlagsLocal;
use crate::args::InstallTopLevelFlags;
use crate::factory::CliFactory;

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
  print_install_report(start_instant.elapsed(), &install_reporter);

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
