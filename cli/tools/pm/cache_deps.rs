// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use deno_error::JsErrorBox;
use deno_error::JsErrorClass;
use deno_graph::JsrPackageReqNotFoundError;
use deno_graph::ModuleGraphError;
use deno_graph::ResolutionError;
use deno_graph::packages::JsrPackageVersionInfo;
use deno_graph::source::ResolveError;
use deno_semver::jsr::JsrPackageReqReference;
use deno_semver::package::PackageNv;
use ende_core::error::AnyError;
use ende_core::futures::StreamExt;
use ende_core::futures::stream::FuturesUnordered;
use ende_core::url::Url;
use ende_resolver::EndeResolveErrorKind;
use ende_resolver::UnsupportedSpecifierError;

use crate::factory::CliFactory;
use crate::graph_container::ModuleGraphContainer;
use crate::graph_container::ModuleGraphUpdatePermit;
use crate::graph_util::BuildGraphOptions;

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
    // Roots added only because they're an export of a JSR package in the
    // config, with the name to show for them.
    let mut export_roots = Vec::new();

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
                Option<(Url, PackageNv, Arc<JsrPackageVersionInfo>)>,
                JsrPackageReqNotFoundError,
              >::Ok(None);
            };
            if let Some(info) = jsr_resolver.package_version_info(&nv).await {
              return Ok(Some((specifier.clone(), nv.into_owned(), info)));
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
      if let Some((specifier, nv, info)) = info_future? {
        let exports = info.exports();
        for (k, _) in exports {
          if let Ok(spec) = specifier.join(k) {
            let name =
              format!("jsr:{}{}", nv, k.strip_prefix('.').unwrap_or(k));
            export_roots.push((spec, name));
          }
        }
      }
    }
    drop(info_futures);

    // Packages requested via `--no-save`: install them without declaring them.
    roots.extend(options.additional_roots.iter().cloned());

    let graph_builder = factory.module_graph_builder().await?;
    graph_builder
      .build_graph_roots(
        graph,
        roots
          .iter()
          .chain(export_roots.iter().map(|(spec, _)| spec))
          .cloned()
          .collect(),
        BuildGraphOptions {
          loader: None,
          is_dynamic: false,
        },
      )
      .await?;
    maybe_graph_error = graph_builder
      .graph_roots_valid(
        graph,
        &roots,
        crate::graph_util::GraphRootsValidOptions {
          allow_unknown_media_types: true,
          allow_unknown_jsr_exports: true,
          allow_sloppy_imports_hints_for_unreferenced_roots: true,
        },
      )
      .and_then(|()| {
        // An export the app may never import shouldn't fail the whole
        // command just because it uses something Ende doesn't support.
        // Importing it still fails at run time.
        for (spec, name) in &export_roots {
          let mut unsupported_reason = None;
          for error in graph_builder.graph_roots_errors(
            graph,
            std::slice::from_ref(spec),
            true,
            true,
          ) {
            match as_unsupported_specifier_error(error.original()) {
              Some(err) => {
                unsupported_reason.get_or_insert_with(|| err.to_string());
              }
              None => return Err(JsErrorBox::from_err(error)),
            }
          }
          if let Some(reason) = unsupported_reason {
            log::warn!(
              "{} {} can't be used in Ende: {}",
              deno_terminal::colors::yellow("Warning"),
              name,
              reason,
            );
          }
        }
        Ok(())
      });
  }

  maybe_graph_error?;

  Ok(())
}

/// The Ende unsupported-specifier error (a Node built-in, npm package or
/// CommonJS module) behind `error`, if that's what it is.
fn as_unsupported_specifier_error(
  error: &ModuleGraphError,
) -> Option<&UnsupportedSpecifierError> {
  let (ModuleGraphError::ResolutionError(ResolutionError::ResolverError {
    error,
    ..
  })
  | ModuleGraphError::TypesResolutionError(
    ResolutionError::ResolverError { error, .. },
  )) = error
  else {
    return None;
  };
  let ResolveError::Other(error) = error.as_ref() else {
    return None;
  };
  match error.get_ref().downcast_ref::<EndeResolveErrorKind>()? {
    EndeResolveErrorKind::UnsupportedSpecifier(err) => Some(err),
    _ => None,
  }
}
