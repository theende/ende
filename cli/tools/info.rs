// Copyright 2018-2026 the Deno authors. MIT license.

use std::collections::HashSet;
use std::fmt::Write;
use std::sync::Arc;

use deno_ast::ModuleSpecifier;
use deno_error::JsErrorClass;
use deno_graph::Dependency;
use deno_graph::GraphKind;
use deno_graph::Module;
use deno_graph::ModuleError;
use deno_graph::ModuleErrorKind;
use deno_graph::ModuleGraph;
use deno_graph::Resolution;
use deno_terminal::colors;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::serde_json;
use ende_core::url;
use ende_lib::util::checksum;
use ende_lib::version::ENDE_VERSION_INFO;
use ende_print::drop_println;
use ende_resolver::display::DisplayTreeNode;

use crate::args::Flags;
use crate::args::InfoFlags;
use crate::display;
use crate::factory::CliFactory;
use crate::graph_util::graph_exit_integrity_errors;
use crate::util::path::resolve_url_or_path_normalized;

const JSON_SCHEMA_VERSION: u8 = 1;

/// Removes the `resolutionMode` of each dependency from the JSON output.
fn remove_resolution_modes(json: &mut serde_json::Value) {
  let modules = json.get_mut("modules").and_then(|m| m.as_array_mut());
  for module in modules.into_iter().flatten() {
    let dependencies = module
      .get_mut("dependencies")
      .and_then(|d| d.as_array_mut());
    for dep in dependencies.into_iter().flatten() {
      for key in ["code", "types"] {
        if let Some(resolution) =
          dep.get_mut(key).and_then(|r| r.as_object_mut())
        {
          resolution.remove("resolutionMode");
        }
      }
    }
  }
}

pub async fn info(
  flags: Arc<Flags>,
  info_flags: InfoFlags,
) -> Result<(), AnyError> {
  let factory = CliFactory::from_flags(flags);
  let cli_options = factory.cli_options()?;
  if let Some(specifier) = info_flags.file {
    let module_graph_builder = factory.module_graph_builder().await?;
    let module_graph_creator = factory.module_graph_creator().await?;
    let maybe_lockfile = factory.maybe_lockfile().await?;
    let resolver = factory.workspace_resolver().await?.clone();

    let cwd_url =
      url::Url::from_directory_path(cli_options.initial_cwd()).unwrap();

    let maybe_import_specifier = match resolver.resolve(
      &specifier,
      &cwd_url,
      ende_resolver::workspace::ResolutionKind::Execution,
    ) {
      Ok(resolved) => match resolved {
        ende_resolver::workspace::MappedResolution::Normal {
          specifier,
          ..
        }
        | ende_resolver::workspace::MappedResolution::WorkspaceJsrPackage {
          specifier,
          ..
        } => Some(specifier),
      },
      Err(_) => None,
    };

    let specifier = match maybe_import_specifier {
      Some(specifier) => specifier,
      None => {
        resolve_url_or_path_normalized(&specifier, cli_options.initial_cwd())?
      }
    };

    let mut loader =
      module_graph_builder.create_graph_loader_with_root_permissions();
    loader.enable_loading_cache_info(); // for displaying the cache information
    let graph = module_graph_creator
      .create_graph_with_loader(GraphKind::All, vec![specifier], &mut loader)
      .await?;

    // write out the lockfile if there is one
    if let Some(lockfile) = &maybe_lockfile {
      graph_exit_integrity_errors(&graph);
      lockfile.write_if_changed()?;
    }

    if info_flags.json {
      let mut json_graph = serde_json::json!(graph);
      if let Some(output) = json_graph.as_object_mut() {
        output.shift_insert(
          0,
          "version".to_string(),
          JSON_SCHEMA_VERSION.into(),
        );
      }
      remove_resolution_modes(&mut json_graph);
      display::write_json_to_stdout(&json_graph)?;
    } else {
      let mut output = String::new();
      GraphDisplayContext::write(&graph, &mut output)?;
      ende_print::drop_write_stdout(output.as_bytes());
    }
  } else {
    // If it was just "ende info" print location of caches and exit
    print_cache_info(
      &factory,
      info_flags.json,
      cli_options.location_flag().as_ref(),
    )?;
  }
  Ok(())
}

fn print_cache_info(
  factory: &CliFactory,
  json: bool,
  location: Option<&ende_core::url::Url>,
) -> Result<(), AnyError> {
  let ende_version = ENDE_VERSION_INFO.ende;
  let dir = factory.ende_dir()?;
  let modules_cache = factory.global_http_cache()?.dir_path();
  let typescript_cache = &dir.gen_cache.location;
  let registry_cache = dir.registries_folder_path();
  let mut origin_dir = dir.origin_data_folder_path();
  let ende_dir = dir.root_path_for_display().to_string();
  let web_cache_dir = ende_lib::worker::get_cache_storage_dir(&origin_dir);

  if let Some(location) = &location {
    origin_dir =
      origin_dir.join(checksum::r#gen(&[location.to_string().as_bytes()]));
  }

  let local_storage_dir = origin_dir.join("local_storage");

  if json {
    let mut json_output = serde_json::json!({
      "version": JSON_SCHEMA_VERSION,
      "denoVersion": ende_version,
      "denoDir": ende_dir,
      "modulesCache": modules_cache,
      "typescriptCache": typescript_cache,
      "registryCache": registry_cache,
      "originStorage": origin_dir,
      "webCacheStorage": web_cache_dir,
    });

    if location.is_some() {
      json_output["localStorage"] = serde_json::to_value(local_storage_dir)?;
    }

    display::write_json_to_stdout(&json_output)
  } else {
    drop_println!("{} {}", colors::bold("Ende version:"), ende_version);
    drop_println!("{} {}", colors::bold("ENDE_DIR location:"), ende_dir);
    drop_println!(
      "{} {}",
      colors::bold("Remote modules cache:"),
      modules_cache.display()
    );
    drop_println!(
      "{} {}",
      colors::bold("Emitted modules cache:"),
      typescript_cache.display()
    );
    drop_println!(
      "{} {}",
      colors::bold("Language server registries cache:"),
      registry_cache.display(),
    );
    drop_println!(
      "{} {}",
      colors::bold("Origin storage:"),
      origin_dir.display()
    );
    drop_println!(
      "{} {}",
      colors::bold("Web cache storage:"),
      web_cache_dir.display()
    );
    if location.is_some() {
      drop_println!(
        "{} {}",
        colors::bold("Local Storage:"),
        local_storage_dir.display(),
      );
    }
    Ok(())
  }
}

struct GraphDisplayContext<'a> {
  graph: &'a ModuleGraph,
  seen: HashSet<String>,
}

impl<'a> GraphDisplayContext<'a> {
  pub fn write<TWrite: Write>(
    graph: &'a ModuleGraph,
    writer: &mut TWrite,
  ) -> Result<(), AnyError> {
    Self {
      graph,
      seen: Default::default(),
    }
    .into_writer(writer)
  }

  fn into_writer<TWrite: Write>(
    mut self,
    writer: &mut TWrite,
  ) -> Result<(), AnyError> {
    if self.graph.roots.is_empty() || self.graph.roots.len() > 1 {
      bail!("displaying graphs that have multiple roots is not supported.");
    }

    let root_specifier = self.graph.resolve(&self.graph.roots[0]);
    match self.graph.try_get(root_specifier) {
      Ok(Some(root)) => {
        let maybe_cache_info = match root {
          Module::Js(module) => module.maybe_cache_info.as_ref(),
          Module::Json(module) => module.maybe_cache_info.as_ref(),
          Module::Wasm(module) => module.maybe_cache_info.as_ref(),
          Module::Node(_) | Module::Npm(_) | Module::External(_) => None,
        };
        if let Some(cache_info) = maybe_cache_info
          && let Some(local) = &cache_info.local
        {
          writeln!(
            writer,
            "{} {}",
            colors::bold("local:"),
            local.to_string_lossy()
          )?;
        }
        if let Some(module) = root.js() {
          writeln!(writer, "{} {}", colors::bold("type:"), module.media_type)?;
        }
        let total_modules_size = self
          .graph
          .modules()
          .map(|m| {
            let size = match m {
              Module::Js(module) => module.size(),
              Module::Json(module) => module.size(),
              Module::Wasm(module) => module.size(),
              Module::Node(_) | Module::Npm(_) | Module::External(_) => 0,
            };
            size as f64
          })
          .sum::<f64>();
        let total_size = total_modules_size;
        let dep_count = self.graph.modules().count() - 1; // -1 for the root module
        writeln!(
          writer,
          "{} {} unique",
          colors::bold("dependencies:"),
          dep_count,
        )?;
        writeln!(
          writer,
          "{} {}",
          colors::bold("size:"),
          display::human_size(total_size),
        )?;
        writeln!(writer)?;
        let root_node = self.build_module_info(root, false);
        root_node.print(writer)?;
        Ok(())
      }
      Err(err) => {
        if let ModuleErrorKind::Missing { .. } = err.as_kind() {
          bail!("module could not be found");
        } else {
          bail!("{:#}", err);
        }
      }
      Ok(None) => {
        bail!("an internal error occurred");
      }
    }
  }

  fn build_dep_info(&mut self, dep: &Dependency) -> Vec<DisplayTreeNode> {
    let mut children = Vec::with_capacity(2);
    if !dep.maybe_code.is_none()
      && let Some(child) = self.build_resolved_info(&dep.maybe_code, false)
    {
      children.push(child);
    }
    if !dep.maybe_type.is_none()
      && let Some(child) = self.build_resolved_info(&dep.maybe_type, true)
    {
      children.push(child);
    }
    children
  }

  fn build_module_info(
    &mut self,
    module: &Module,
    type_dep: bool,
  ) -> DisplayTreeNode {
    let specifier = module.specifier();
    let was_seen = !self.seen.insert(specifier.to_string());
    let header_text = specifier.to_string();
    let header_text = if was_seen {
      let specifier_str = if type_dep {
        colors::italic_gray(header_text).to_string()
      } else {
        colors::gray(header_text).to_string()
      };
      format!("{} {}", specifier_str, colors::gray("*"))
    } else {
      let header_text = if type_dep {
        colors::italic(header_text).to_string()
      } else {
        header_text
      };
      let maybe_size = match module {
        Module::Js(module) => Some(module.size() as u64),
        Module::Json(module) => Some(module.size() as u64),
        Module::Wasm(module) => Some(module.size() as u64),
        Module::Node(_) | Module::Npm(_) | Module::External(_) => None,
      };
      format!("{} {}", header_text, maybe_size_to_text(maybe_size))
    };

    let mut tree_node = DisplayTreeNode::from_text(header_text);

    if !was_seen {
      match module {
        Module::Js(module) => {
          if let Some(types_dep) = &module.maybe_types_dependency
            && let Some(child) =
              self.build_resolved_info(&types_dep.dependency, true)
          {
            tree_node.children.push(child);
          }
          for dep in module.dependencies.values() {
            tree_node.children.extend(self.build_dep_info(dep));
          }
        }
        Module::Wasm(module) => {
          for dep in module.dependencies.values() {
            tree_node.children.extend(self.build_dep_info(dep));
          }
        }
        Module::Json(_)
        | Module::Npm(_)
        | Module::Node(_)
        | Module::External(_) => {}
      }
    }
    tree_node
  }

  fn build_error_info(
    &mut self,
    err: &ModuleError,
    specifier: &ModuleSpecifier,
  ) -> DisplayTreeNode {
    self.seen.insert(specifier.to_string());
    match err.as_kind() {
      ModuleErrorKind::InvalidTypeAssertion { .. } => {
        self.build_error_msg(specifier, "(invalid import attribute)")
      }
      ModuleErrorKind::Load { err, .. } => {
        use deno_graph::ModuleLoadError::*;
        let message = match err {
          HttpsChecksumIntegrity(_) => "(checksum integrity error)",
          Decode(_) => "(loading decode error)",
          Loader(err) => {
            if err.get_class() == "NotCapable" {
              "(not capable, requires --allow-import)"
            } else {
              "(loading error)"
            }
          }
          Jsr(_) => "(loading error)",
          Npm(_) => "(npm loading error)",
          TooManyRedirects => "(too many redirects error)",
        };
        self.build_error_msg(specifier, message.as_ref())
      }
      ModuleErrorKind::Parse { .. } | ModuleErrorKind::WasmParse { .. } => {
        self.build_error_msg(specifier, "(parsing error)")
      }
      ModuleErrorKind::UnsupportedImportAttributeType { .. } => {
        self.build_error_msg(specifier, "(unsupported import attribute)")
      }
      ModuleErrorKind::UnsupportedModuleTypeForSourcePhaseImport { .. } => self
        .build_error_msg(
          specifier,
          "(unsupported module type for source phase import)",
        ),
      ModuleErrorKind::UnsupportedMediaType { .. } => {
        self.build_error_msg(specifier, "(unsupported)")
      }
      ModuleErrorKind::Missing { .. }
      | ModuleErrorKind::MissingDynamic { .. } => {
        self.build_error_msg(specifier, "(missing)")
      }
    }
  }

  fn build_error_msg(
    &self,
    specifier: &ModuleSpecifier,
    error_msg: &str,
  ) -> DisplayTreeNode {
    DisplayTreeNode::from_text(format!(
      "{} {}",
      colors::red(specifier),
      colors::red_bold(error_msg)
    ))
  }

  fn build_resolved_info(
    &mut self,
    resolution: &Resolution,
    type_dep: bool,
  ) -> Option<DisplayTreeNode> {
    match resolution {
      Resolution::Ok(resolved) => {
        let specifier = &resolved.specifier;
        let resolved_specifier = self.graph.resolve(specifier);
        Some(match self.graph.try_get(resolved_specifier) {
          Ok(Some(module)) => self.build_module_info(module, type_dep),
          Err(err) => self.build_error_info(err, resolved_specifier),
          Ok(None) => DisplayTreeNode::from_text(format!(
            "{} {}",
            colors::red(specifier),
            colors::red_bold("(missing)")
          )),
        })
      }
      Resolution::Err(err) => Some(DisplayTreeNode::from_text(format!(
        "{} {}",
        colors::italic(err.to_string()),
        colors::red_bold("(resolve error)")
      ))),
      _ => None,
    }
  }
}

fn maybe_size_to_text(maybe_size: Option<u64>) -> String {
  colors::gray(format!(
    "({})",
    match maybe_size {
      Some(size) => display::human_size(size as f64),
      None => "unknown".to_string(),
    }
  ))
  .to_string()
}
