// Copyright 2018-2026 the Deno authors. MIT license.

use std::sync::Arc;

use dashmap::DashSet;
use ende_core::error::AnyError;
use ende_core::url::Url;

pub use self::bin_name_resolver::BinNameResolver;
use crate::args::Flags;
use crate::args::InstallEntrypointsFlags;
use crate::args::InstallFlags;
use crate::args::InstallFlagsLocal;
use crate::factory::CliFactory;
use crate::graph_container::CollectSpecifiersOptions;
use crate::graph_container::ModuleGraphContainer;
use crate::util::display;

mod bin_name_resolver;
mod global;
mod local;

pub use global::uninstall;
pub use local::ci_command;

#[derive(Default)]
pub struct InstallStats {
  pub resolved_jsr: DashSet<String>,
  pub downloaded_jsr: DashSet<String>,
  pub reused_jsr: DashSet<String>,
}

impl std::fmt::Debug for InstallStats {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("InstallStats")
      .field(
        "resolved_jsr",
        &self
          .resolved_jsr
          .iter()
          .map(|s| s.as_str().to_string())
          .collect::<Vec<_>>(),
      )
      .field(
        "downloaded_jsr",
        &self
          .downloaded_jsr
          .iter()
          .map(|s| s.as_str().to_string())
          .collect::<Vec<_>>(),
      )
      .field("resolved_jsr_count", &self.resolved_jsr.len())
      .field("downloaded_jsr_count", &self.downloaded_jsr.len())
      .finish()
  }
}

#[derive(Debug)]
pub struct InstallReporter {
  stats: Arc<InstallStats>,
}

impl InstallReporter {
  pub fn new() -> Self {
    Self {
      stats: Arc::new(InstallStats::default()),
    }
  }
}

fn package_nv_from_url(url: &Url) -> Option<String> {
  if !matches!(url.scheme(), "http" | "https") {
    return None;
  }
  if !url.host_str().is_some_and(|h| h.contains("jsr.io")) {
    return None;
  }
  let mut parts = url.path_segments()?;
  let scope = parts.next()?;
  let name = parts.next()?;
  let version = parts.next()?;
  if version.ends_with(".json") {
    // don't include meta.json urls
    return None;
  }
  Some(format!("{scope}/{name}@{version}"))
}

impl deno_graph::source::Reporter for InstallReporter {
  fn on_resolve(
    &self,
    _req: &deno_semver::package::PackageReq,
    package_nv: &deno_semver::package::PackageNv,
  ) {
    self.stats.resolved_jsr.insert(package_nv.to_string());
  }
}

impl ende_resolver::file_fetcher::GraphLoaderReporter for InstallReporter {
  fn on_load(
    &self,
    specifier: &Url,
    loaded_from: ende_cache_dir::file_fetcher::LoadedFrom,
  ) {
    if let Some(nv) = package_nv_from_url(specifier) {
      match loaded_from {
        ende_cache_dir::file_fetcher::LoadedFrom::Cache => {
          self.stats.reused_jsr.insert(nv);
        }
        ende_cache_dir::file_fetcher::LoadedFrom::Remote => {
          self.stats.downloaded_jsr.insert(nv);
        }
        _ => {}
      }
    } else {
      // it's a local file or http/https specifier
    }
  }
}

pub async fn install_from_entrypoints(
  flags: Arc<Flags>,
  entrypoints_flags: InstallEntrypointsFlags,
) -> Result<(), AnyError> {
  let started = std::time::Instant::now();
  let factory = CliFactory::from_flags(flags.clone());
  let emitter = factory.emitter()?;
  let main_graph_container = factory.main_module_graph_container().await?;
  let specifiers = main_graph_container.collect_specifiers(
    &entrypoints_flags.entrypoints,
    CollectSpecifiersOptions {
      include_ignored_specified: true,
    },
  )?;
  main_graph_container
    .check_specifiers(
      &specifiers,
      crate::graph_container::CheckSpecifiersOptions {
        ext_overwrite: None,
        allow_unknown_media_types: true,
      },
    )
    .await?;
  emitter
    .cache_module_emits(&main_graph_container.graph())
    .await?;

  print_install_report(
    started.elapsed(),
    &factory.install_reporter()?.unwrap().clone(),
  );
  Ok(())
}

pub async fn install_command(
  flags: Arc<Flags>,
  install_flags: InstallFlags,
) -> Result<(), AnyError> {
  match install_flags {
    InstallFlags::Global(global_flags) => {
      Box::pin(global::install_global(flags, global_flags)).await
    }
    InstallFlags::Local(local_flags) => {
      if let InstallFlagsLocal::Add(add_flags) = &local_flags {
        local::check_if_installs_a_single_package_globally(Some(add_flags))?;
      }
      Box::pin(local::install_local(flags, local_flags)).await
    }
  }
}

pub fn print_install_report(
  elapsed: std::time::Duration,
  install_reporter: &InstallReporter,
) {
  fn human_elapsed(elapsed: u128) -> String {
    display::human_elapsed_with_ms_limit(elapsed, 3_000)
  }

  let rep = install_reporter;

  if !rep.stats.downloaded_jsr.is_empty() {
    let total_installed = rep.stats.downloaded_jsr.len();
    log::info!(
      "{} {} {} {} {}",
      deno_terminal::colors::gray("Installed"),
      total_installed,
      deno_terminal::colors::gray(format!(
        "package{}",
        if total_installed > 1 { "s" } else { "" },
      )),
      deno_terminal::colors::gray("in"),
      human_elapsed(elapsed.as_millis())
    );

    let total_reused = rep.stats.reused_jsr.len();
    log::info!(
      "{} {} {}",
      deno_terminal::colors::gray("Reused"),
      total_reused,
      deno_terminal::colors::gray(format!(
        "package{} from cache",
        if total_reused == 1 { "" } else { "s" },
      )),
    );
    if total_reused > 0 {
      log::info!(
        "{}",
        deno_terminal::colors::yellow_bold("+".repeat(total_reused))
      );
    }

    let jsr_downloaded = rep.stats.downloaded_jsr.len();
    log::info!(
      "{} {} {}",
      deno_terminal::colors::gray("Downloaded"),
      jsr_downloaded,
      deno_terminal::colors::gray(format!(
        "package{} from JSR",
        if jsr_downloaded == 1 { "" } else { "s" },
      )),
    );
    if jsr_downloaded > 0 {
      log::info!(
        "{}",
        deno_terminal::colors::green("+".repeat(jsr_downloaded))
      );
    }
  }

  if !rep.stats.downloaded_jsr.is_empty() {
    log::info!("");
    log::info!("{}", deno_terminal::colors::cyan("Dependencies:"));
    let mut jsr_packages = rep
      .stats
      .downloaded_jsr
      .clone()
      .into_iter()
      .collect::<Vec<_>>();
    jsr_packages.sort();
    for pkg in jsr_packages {
      let (name, version) = pkg.rsplit_once("@").unwrap();
      log::info!(
        "{} {}{} {}",
        deno_terminal::colors::green("+"),
        deno_terminal::colors::gray("jsr:"),
        name,
        deno_terminal::colors::gray(version)
      );
    }
    log::info!("");
  }
}
