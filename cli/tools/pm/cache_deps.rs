// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use deno_graph::JsrPackageReqNotFoundError;
use deno_graph::packages::JsrPackageVersionInfo;
use deno_semver::jsr::JsrPackageReqReference;
use ende_core::error::AnyError;
use ende_core::futures::StreamExt;
use ende_core::futures::stream::FuturesUnordered;
use ende_core::url::Url;
use ende_npm_installer::PackageCaching;
use ende_npm_installer::graph::NpmCachingStrategy;

use crate::factory::CliFactory;
use crate::graph_container::ModuleGraphContainer;
use crate::graph_container::ModuleGraphUpdatePermit;
use crate::graph_util::BuildGraphWithNpmOptions;

pub struct CacheTopLevelDepsOptions {
  pub lockfile_only: bool,
  /// Extra specifiers to resolve and cache in addition to the project's
  /// declared dependencies. Used by `ende add --no-save` to install packages
  /// without writing them to the configuration file.
  pub additional_roots: Vec<Url>,
}

pub async fn cache_top_level_deps(
  // todo(dsherret): don't pass the factory into this function. Instead use ctor deps
  factory: &CliFactory,
  jsr_resolver: Option<Arc<crate::jsr::JsrFetchResolver>>,
  options: CacheTopLevelDepsOptions,
) -> Result<(), AnyError> {
  let _clear_guard = factory
    .text_only_progress_bar()
    .deferred_keep_initialize_alive();
  let npm_installer = factory.npm_installer().await?;
  if let Some(lockfile) = factory.maybe_lockfile().await? {
    lockfile.error_if_changed()?;
  }
  // cache as many entries in the import map as we can
  let resolver = factory.workspace_resolver().await?;

  let mut maybe_graph_error = Ok(());
  let has_import_map = resolver.maybe_import_map().is_some();
  if has_import_map || !options.additional_roots.is_empty() {
    let jsr_resolver = if let Some(resolver) = jsr_resolver {
      resolver
    } else {
      Arc::new(crate::jsr::JsrFetchResolver::new(
        factory.file_fetcher()?.clone(),
        factory.jsr_version_resolver()?.clone(),
      ))
    };
    let mut graph_permit = factory
      .main_module_graph_container()
      .await?
      .acquire_update_permit()
      .await;
    let graph = graph_permit.graph_mut();
    if let Some(lockfile) = factory.maybe_lockfile().await? {
      lockfile.fill_graph(graph);
    }

    let mut roots = Vec::new();

    let mut info_futures = FuturesUnordered::new();

    let mut seen_reqs = HashSet::new();

    let workspace_jsr_packages = resolver.jsr_packages();

    let import_map_entries = resolver
      .maybe_import_map()
      .map(|import_map| {
        import_map
          .imports()
          .entries()
          .chain(
            import_map
              .scopes()
              .flat_map(|scope| scope.imports.entries()),
          )
          .collect::<Vec<_>>()
      })
      .unwrap_or_default();

    for entry in import_map_entries {
      let Some(specifier) = entry.value else {
        continue;
      };

      match specifier.scheme() {
        "jsr" => {
          let specifier_str = specifier.as_str();
          let Ok(req_ref) = JsrPackageReqReference::from_str(specifier_str)
          else {
            continue;
          };
          if workspace_jsr_packages
            .iter()
            .any(|pkg| pkg.matches_req(req_ref.req()))
          {
            // do not install a workspace jsr package
            continue;
          }
          if let Some(sub_path) = req_ref.sub_path() {
            if sub_path.ends_with('/') {
              continue;
            }
            roots.push(specifier.clone());
            continue;
          }
          if !seen_reqs.insert(req_ref.req().clone()) {
            continue;
          }
          if req_ref.req().version_req.tag().is_some() {
            // JSR has no dist-tags, so this dependency can never resolve.
            // Warn instead of failing so the rest of the project still
            // installs; the graph build reports a proper error when the
            // specifier is actually imported. Skipping here also keeps
            // tagged reqs away from VersionReq::matches below, which
            // panics on tags.
            log::warn!(
              "{} Ignoring \"jsr:{}\". Version tags are not supported in jsr specifiers.",
              deno_terminal::colors::yellow("Warning"),
              req_ref.req(),
            );
            continue;
          }
          let resolved_req = graph.packages.mappings().get(req_ref.req());
          let resolved_req = resolved_req.and_then(|nv| {
            // the version might end up being upgraded to a newer version that's already in
            // the graph (due to a reverted change), in which case our exports could end up
            // being wrong. to avoid that, see if there's a newer version that matches the version
            // req.
            let versions =
              graph.packages.versions_by_name(&req_ref.req().name)?;
            let mut best = nv;
            for version in versions {
              if version.version > best.version
                && req_ref.req().version_req.matches(&version.version)
              {
                best = version;
              }
            }
            Some(best)
          });

          let jsr_resolver = jsr_resolver.clone();
          info_futures.push(async move {
            let nv = if let Some(nv) = resolved_req {
              Cow::Borrowed(nv)
            } else if let Some(nv) =
              jsr_resolver.req_to_nv(req_ref.req()).await?
            {
              Cow::Owned(nv)
            } else {
              return Result::<
                Option<(Url, Arc<JsrPackageVersionInfo>)>,
                JsrPackageReqNotFoundError,
              >::Ok(None);
            };
            if let Some(info) = jsr_resolver.package_version_info(&nv).await {
              return Ok(Some((specifier.clone(), info)));
            }
            Ok(None)
          });
        }
        // npm packages are not supported in Ende
        "npm" => {}
        _ => {
          if entry.key.ends_with('/') && specifier.as_str().ends_with('/') {
            continue;
          }
          if specifier.scheme() == "file"
            && let Ok(path) = specifier.to_file_path()
            && !path.is_file()
          {
            continue;
          }
          roots.push(specifier.clone());
        }
      }
    }

    while let Some(info_future) = info_futures.next().await {
      if let Some((specifier, info)) = info_future? {
        let exports = info.exports();
        for (k, _) in exports {
          if let Ok(spec) = specifier.join(k) {
            roots.push(spec);
          }
        }
      }
    }
    drop(info_futures);

    // Packages requested via `--no-save`: install them without declaring them.
    roots.extend(options.additional_roots.iter().cloned());

    let graph_builder = factory.module_graph_builder().await?;
    graph_builder
      .build_graph_roots_with_npm_resolution(
        graph,
        roots.clone(),
        BuildGraphWithNpmOptions {
          loader: None,
          is_dynamic: false,
          npm_caching: NpmCachingStrategy::Manual,
        },
      )
      .await?;
    maybe_graph_error =
      graph_builder.graph_roots_valid(graph, &roots, true, true);
  }

  if options.lockfile_only {
    // do a resolution install if the npm snapshot is in a
    // pending state due to a config file change
    npm_installer.install_resolution_if_pending().await?;
  } else {
    npm_installer.cache_packages(PackageCaching::All).await?;
  }

  maybe_graph_error?;

  Ok(())
}
