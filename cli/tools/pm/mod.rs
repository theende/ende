// Copyright 2018-2026 the Deno authors. MIT license.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use deno_path_util::url_to_file_path;
use deno_semver::StackString;
use deno_semver::Version;
use deno_semver::jsr::JsrPackageReqReference;
use deno_semver::package::PackageName;
use deno_semver::package::PackageNv;
use deno_semver::package::PackageReq;
use deps::KeyPath;
use ende_cache_dir::GlobalOrLocalHttpCache;
use ende_cache_dir::file_fetcher::CacheSetting;
use ende_core::anyhow::Context;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::futures::FutureExt;
use ende_core::futures::StreamExt;
use ende_core::serde_json;
use jsonc_parser::cst::CstObject;
use jsonc_parser::cst::CstObjectProp;
use jsonc_parser::cst::CstRootNode;
use jsonc_parser::json;

use crate::args::AddFlags;
use crate::args::CliOptions;
use crate::args::Flags;
use crate::args::LinkFlags;
use crate::args::RemoveFlags;
use crate::args::UnlinkFlags;
use crate::factory::CliFactory;
use crate::file_fetcher::CreateCliFileFetcherOptions;
use crate::file_fetcher::create_cli_file_fetcher;
use crate::jsr::JsrFetchResolver;

mod cache_deps;
pub(crate) mod deps;
pub(crate) mod interactive_picker;
mod list;
pub(crate) mod outdated;
mod why;

pub use cache_deps::CacheTopLevelDepsOptions;
pub use cache_deps::cache_top_level_deps;
pub use list::list;
pub use outdated::outdated;
pub use why::why;

struct ConfigUpdater {
  cst: CstRootNode,
  root_object: CstObject,
  path: PathBuf,
  modified: bool,
}

impl ConfigUpdater {
  fn new(config_file_path: PathBuf) -> Result<Self, AnyError> {
    let config_file_contents = std::fs::read_to_string(&config_file_path)
      .with_context(|| {
        format!("Reading config file '{}'", config_file_path.display())
      })?;
    let cst = CstRootNode::parse(&config_file_contents, &Default::default())
      .with_context(|| {
        format!("Parsing config file '{}'", config_file_path.display())
      })?;
    let root_object = cst.object_value_or_set();
    Ok(Self {
      cst,
      root_object,
      path: config_file_path,
      modified: false,
    })
  }

  fn display_path(&self) -> String {
    deno_path_util::url_from_file_path(&self.path)
      .map(|u| u.to_string())
      .unwrap_or_else(|_| self.path.display().to_string())
  }

  fn obj(&self) -> &CstObject {
    &self.root_object
  }

  fn contents(&self) -> String {
    self.cst.to_string()
  }

  fn get_property_for_mutation(
    &mut self,
    key_path: &KeyPath,
  ) -> Option<CstObjectProp> {
    let mut current_node = self.root_object.clone();

    self.modified = true;

    for (i, part) in key_path.parts.iter().enumerate() {
      let s = part.as_str();
      if i < key_path.parts.len().saturating_sub(1) {
        let object = current_node.object_value(s)?;
        current_node = object;
      } else {
        // last part
        return current_node.get(s);
      }
    }

    None
  }

  fn add(&mut self, selected: SelectedPackage) {
    fn insert_index(object: &CstObject, searching_name: &str) -> usize {
      object
        .properties()
        .into_iter()
        .take_while(|prop| {
          let prop_name =
            prop.name().and_then(|name| name.decoded_value().ok());
          match prop_name {
            Some(current_name) => {
              searching_name.cmp(&current_name) == std::cmp::Ordering::Greater
            }
            None => true,
          }
        })
        .count()
    }

    let imports = self.root_object.object_value_or_set("imports");
    let value = format!("{}@{}", selected.package_name, selected.version_req);
    match imports.get(&selected.import_name) {
      Some(prop) => {
        prop.set_value(json!(value));
      }
      _ => {
        let index = insert_index(&imports, &selected.import_name);
        imports.insert(index, &selected.import_name, json!(value));
      }
    }

    self.modified = true;
  }

  fn remove(&mut self, package: &str) -> bool {
    let removed = match self
      .root_object
      .object_value("imports")
      .and_then(|i| i.get(package))
    {
      Some(prop) => {
        remove_prop_and_maybe_parent_prop(prop);
        true
      }
      _ => false,
    };
    if removed {
      self.modified = true;
    }
    removed
  }

  fn commit(&self) -> Result<(), AnyError> {
    if !self.modified {
      return Ok(());
    }

    let new_text = self.contents();
    std::fs::write(&self.path, new_text).with_context(|| {
      format!("failed writing to '{}'", self.path.display())
    })?;
    Ok(())
  }
}

fn remove_prop_and_maybe_parent_prop(prop: CstObjectProp) {
  let parent = prop.parent().unwrap().as_object().unwrap();
  prop.remove();
  if parent.properties().is_empty() {
    let parent_property = parent.parent().unwrap();
    let root_object = parent_property.parent().unwrap().as_object().unwrap();
    // remove the property
    parent_property.remove();
    root_object.ensure_multiline();
  }
}

fn create_ende_json(
  flags: &Arc<Flags>,
  options: &CliOptions,
) -> Result<CliFactory, AnyError> {
  let name = ende_config::ende_json::DEFAULT_CONFIG_FILE_NAME;
  std::fs::write(options.initial_cwd().join(name), "{}\n")
    .with_context(|| format!("Failed to create {name} file"))?;
  log::info!("Created {name} configuration file.");
  let factory = CliFactory::from_flags(flags.clone());
  Ok(factory)
}

#[derive(Clone, Copy)]
/// The name of the subcommand invoking the `add` operation.
pub enum AddCommandName {
  Add,
  Install,
}

impl std::fmt::Display for AddCommandName {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      AddCommandName::Add => write!(f, "add"),
      AddCommandName::Install => write!(f, "install"),
    }
  }
}

/// Loads the ende.json in the current directory, creating one when it's
/// missing.
fn load_config(
  flags: &Arc<Flags>,
) -> Result<(CliFactory, ConfigUpdater), AnyError> {
  let cli_factory = CliFactory::from_flags(flags.clone());
  let options = cli_factory.cli_options()?;
  if let Some(ende_json) = options.start_dir.member_ende_json() {
    let config = ConfigUpdater::new(url_to_file_path(&ende_json.specifier)?)?;
    return Ok((cli_factory, config));
  }
  let factory = create_ende_json(flags, options)?;
  let options = factory.cli_options()?.clone();
  let Some(ende_json) = options.start_dir.member_or_root_ende_json() else {
    bail!(
      "Failed to discover the newly created {} at \"{}\". This can happen when the current directory is inside a node_modules directory.",
      ende_config::ende_json::DEFAULT_CONFIG_FILE_NAME,
      options
        .initial_cwd()
        .join(ende_config::ende_json::DEFAULT_CONFIG_FILE_NAME)
        .display(),
    );
  };
  let config = ConfigUpdater::new(url_to_file_path(&ende_json.specifier)?)?;
  Ok((factory, config))
}

pub async fn add(
  flags: Arc<Flags>,
  add_flags: AddFlags,
  cmd_name: AddCommandName,
) -> Result<(), AnyError> {
  let save_exact = add_flags.save_exact;
  // error before creating an ende.json
  if let Some(npm_package) = add_flags
    .packages
    .iter()
    .find(|p| p.starts_with("npm:") || p.contains("@npm:"))
  {
    return Err(super::npm_not_supported_error(npm_package));
  }
  let (cli_factory, mut ende_json_config) = load_config(&flags)?;

  if ende_json_config.obj().get("importMap").is_some() {
    bail!(
      concat!(
        "`ende {}` is not supported when configuration file contains an \"importMap\" field. ",
        "Inline the import map into the Ende configuration file.\n",
        "    at {}",
      ),
      cmd_name,
      ende_json_config.display_path(),
    );
  }

  let http_client = cli_factory.http_client_provider();
  let deps_http_cache = cli_factory.global_http_cache()?;
  let deps_file_fetcher = create_cli_file_fetcher(
    ende_runtime::ende_web::BlobStore::default_arc(),
    GlobalOrLocalHttpCache::Global(deps_http_cache.clone()),
    http_client.clone(),
    cli_factory.memory_files().clone(),
    cli_factory.sys(),
    CreateCliFileFetcherOptions {
      allow_remote: true,
      cache_setting: CacheSetting::ReloadAll,
      download_log_level: log::Level::Trace,
      progress_bar: None,
    },
  );

  let jsr_resolver = Arc::new(JsrFetchResolver::new(
    Arc::new(deps_file_fetcher),
    cli_factory.jsr_version_resolver()?.clone(),
  ));

  let mut selected_packages = Vec::with_capacity(add_flags.packages.len());
  let mut package_reqs: Vec<AddRmPackageReq> =
    Vec::with_capacity(add_flags.packages.len());

  let initial_cwd = cli_factory.cli_options()?.initial_cwd().to_path_buf();
  for entry_text in add_flags.packages.iter() {
    if let Some(hint) = link_hint_for_spec(&initial_cwd, entry_text) {
      bail!("{hint}");
    }
    let req = AddRmPackageReq::parse(entry_text, true)
      .with_context(|| format!("Failed to parse package: {}", entry_text))?;

    match req {
      Ok(mut add_req) => {
        if add_flags.unscoped {
          add_req.use_unscoped_alias();
          // packages from different scopes can share an unscoped name
          // (ex. `@luca/flag` and `@other/flag`), which would otherwise
          // silently overwrite each other in the config
          if let Some(existing) =
            package_reqs.iter().find(|r| r.alias == add_req.alias)
          {
            bail!(
              "{} and {} would both be added as \"{}\". Provide an explicit alias for one of them (ex. `{}`).",
              existing.package_name(),
              add_req.package_name(),
              add_req.alias,
              crate::colors::yellow(format!(
                "ende {cmd_name} my-alias@{}",
                add_req.package_name()
              )),
            );
          }
        }
        package_reqs.push(add_req)
      }
      // unreachable because unprefixed packages default to jsr
      Err(package_req) => {
        bail!(
          "{entry_text} is missing a prefix. Did you mean `{}`?",
          crate::colors::yellow(format!("ende {cmd_name} jsr:{package_req}"))
        )
      }
    }
  }

  let package_futures = package_reqs
    .into_iter()
    .map({
      let jsr_resolver = jsr_resolver.clone();
      move |package_req| {
        find_package_and_select_version_for_req(
          jsr_resolver.clone(),
          package_req,
          save_exact,
        )
        .boxed_local()
      }
    })
    .collect::<Vec<_>>();

  let stream_of_futures = ende_core::futures::stream::iter(package_futures);
  let mut buffered = stream_of_futures.buffered(10);

  while let Some(package_and_version_result) = buffered.next().await {
    let package_and_version = package_and_version_result?;

    match package_and_version {
      PackageAndVersion::NotFound {
        package: package_name,
        help,
      } => match help {
        Some(NotFoundHelp::PreReleaseVersion(version)) => {
          bail!(
            "{} has only pre-release versions available. Try specifying a version: `{}`",
            crate::colors::red(&package_name),
            crate::colors::yellow(format!(
              "ende {cmd_name} {package_name}@^{version}"
            ))
          )
        }
        None => bail!("{} was not found.", crate::colors::red(package_name)),
      },
      PackageAndVersion::Selected(selected) => {
        selected_packages.push(selected);
      }
    }
  }

  // With `--no-save` the package is resolved and installed directly as an
  // additional graph root (and written to the lockfile) rather than being
  // picked up from the configuration file during the install step.
  let install_directly = add_flags.no_save;
  let mut additional_roots = Vec::new();
  for selected_package in selected_packages {
    log::info!(
      "Add {}{}{}",
      crate::colors::green(&selected_package.package_name),
      crate::colors::gray("@"),
      selected_package.selected_version
    );

    if install_directly {
      let specifier = format!(
        "{}@{}",
        selected_package.package_name, selected_package.version_req
      );
      match ende_core::url::Url::parse(&specifier) {
        Ok(url) => additional_roots.push(url),
        Err(err) => {
          bail!("Failed to parse package specifier '{specifier}': {err}")
        }
      }
    }

    if add_flags.no_save {
      continue;
    }

    ende_json_config.add(selected_package);
  }

  if !add_flags.no_save {
    ende_json_config.commit()?;
  }

  install_after_modification(
    flags,
    Some(jsr_resolver),
    CacheTopLevelDepsOptions {
      lockfile_only: add_flags.lockfile_only,
      additional_roots,
    },
  )
  .await?;

  Ok(())
}

struct SelectedPackage {
  import_name: StackString,
  package_name: String,
  version_req: String,
  selected_version: StackString,
}

enum NotFoundHelp {
  PreReleaseVersion(Version),
}

enum PackageAndVersion {
  NotFound {
    package: String,
    help: Option<NotFoundHelp>,
  },
  Selected(SelectedPackage),
}

fn best_version<'a>(
  versions: impl Iterator<Item = &'a Version>,
) -> Option<&'a Version> {
  let mut maybe_best_version: Option<&Version> = None;
  for version in versions {
    let is_best_version = maybe_best_version
      .as_ref()
      .map(|best_version| (*best_version).cmp(version).is_lt())
      .unwrap_or(true);
    if is_best_version {
      maybe_best_version = Some(version);
    }
  }
  maybe_best_version
}

async fn jsr_latest_version(
  jsr_resolver: &JsrFetchResolver,
  name: &PackageName,
) -> Option<Version> {
  let info = jsr_resolver.package_info(name).await?;
  best_version(
    info
      .versions
      .iter()
      .filter(|(_, version_info)| !version_info.yanked)
      .map(|(version, _)| version),
  )
  .cloned()
}

async fn find_package_and_select_version_for_req(
  jsr_resolver: Arc<JsrFetchResolver>,
  add_package_req: AddRmPackageReq,
  save_exact: bool,
) -> Result<PackageAndVersion, AnyError> {
  let req = &add_package_req.req;
  let prefixed_name = format!("jsr:{}", req.name);
  // JSR has no dist-tags, so a tag can't go through req_to_nv
  // (VersionReq::matches panics on a tag). "@latest" is conventionally
  // understood as "the newest version", so resolve it to the latest
  // published version; reject any other tag rather than silently treating
  // it as latest.
  let maybe_nv = if let Some(tag) = req.version_req.tag() {
    if tag != "latest" {
      bail!(
        "{} does not support the tag '{tag}'. JSR has no dist-tags; use '@latest' or a version requirement instead.",
        prefixed_name,
      );
    }
    jsr_latest_version(&jsr_resolver, &req.name)
      .await
      .map(|version| PackageNv {
        name: req.name.clone(),
        version,
      })
  } else {
    match jsr_resolver.req_to_nv(req).await {
      Ok(maybe_nv) => maybe_nv,
      Err(err) => {
        if req.version_req.version_text() == "*"
          && let Some(pre_release_version) =
            jsr_latest_version(&jsr_resolver, &req.name).await
        {
          return Ok(PackageAndVersion::NotFound {
            package: prefixed_name,
            help: Some(NotFoundHelp::PreReleaseVersion(
              pre_release_version.clone(),
            )),
          });
        }
        return Err(err.into());
      }
    }
  };
  let Some(nv) = maybe_nv else {
    return Ok(PackageAndVersion::NotFound {
      package: prefixed_name,
      help: None,
    });
  };
  let range_symbol =
    if save_exact || req.version_req.version_text() == nv.version.to_string() {
      ""
    } else if !nv.version.pre.is_empty() {
      // Pin pre-release versions exactly, regardless of any range operator the
      // user requested. A caret or tilde range over a pre-release matches every
      // pre-release sharing the same major.minor.patch and resolves to the
      // lexicographically greatest one, which is not necessarily the build the
      // user asked to install. See #35577.
      ""
    } else if req.version_req.version_text().starts_with('~') {
      "~"
    } else {
      "^"
    };
  Ok(PackageAndVersion::Selected(SelectedPackage {
    import_name: add_package_req.alias,
    package_name: prefixed_name,
    version_req: format!("{}{}", range_symbol, &nv.version),
    selected_version: nv.version.to_custom_string::<StackString>(),
  }))
}

#[derive(Debug, PartialEq, Eq)]
pub struct AddRmPackageReq {
  alias: StackString,
  req: PackageReq,
}

impl AddRmPackageReq {
  /// The package name with its registry prefix (ex. `jsr:@std/path`).
  pub fn package_name(&self) -> String {
    format!("jsr:{}", self.req.name)
  }

  /// Drops the scope from the alias (ex. `@david/jsonc-morph` becomes
  /// `jsonc-morph`).
  ///
  /// Does nothing when the name has no scope, or when the user provided an
  /// alias of their own, which is the case whenever the alias isn't the
  /// package name.
  pub fn use_unscoped_alias(&mut self) {
    let name = &self.req.name;
    if self.alias == *name
      && let Some((_scope, unscoped_name)) =
        name.strip_prefix('@').and_then(|name| name.split_once('/'))
    {
      self.alias = StackString::from(unscoped_name);
    }
  }

  /// Parses a package from the command line. Unprefixed packages are JSR
  /// packages when `default_jsr` is set and are otherwise returned as an
  /// unprefixed package requirement (ex. an alias to remove).
  pub fn parse(
    entry_text: &str,
    default_jsr: bool,
  ) -> Result<Result<Self, PackageReq>, AnyError> {
    // parse the following:
    // - jsr:<package_name>
    // - alias@jsr:<package_name>
    // - @alias/other@jsr:<package_name>
    let (maybe_alias, package_text) = match entry_text.find("jsr:") {
      Some(location) => {
        let prefix = &entry_text[..location];
        let maybe_alias = if prefix.is_empty() {
          None
        } else if let Some(alias) = prefix.strip_suffix('@') {
          Some(StackString::from(alias))
        } else {
          return Ok(Err(PackageReq::from_str(entry_text)?));
        };
        (maybe_alias, &entry_text[location + "jsr:".len()..])
      }
      None => {
        if entry_text.starts_with("npm:") || entry_text.contains("@npm:") {
          return Err(super::npm_not_supported_error(entry_text));
        }
        if !default_jsr {
          return Ok(Err(PackageReq::from_str(entry_text)?));
        }
        (None, entry_text)
      }
    };

    // The reference parser uses the strict specifier version grammar, which
    // only accepts `^`, `~`, exact versions and tags. On the command line we
    // also accept the full range grammar (`>=4`, `>=4 <5`, `^4 || 5`,
    // `1 - 2`, etc) by falling back to loose parsing. `ende add` resolves the
    // requirement to a concrete version before writing it, so what ends up in
    // the config (and in `jsr:` specifiers in code) still uses the strict
    // grammar.
    let req = match JsrPackageReqReference::from_str(&format!(
      "jsr:{}",
      package_text
    )) {
      Ok(req_ref) => req_ref.into_inner().req,
      // If loose parsing also fails the input is genuinely malformed, so
      // surface the original strict error, which carries the more helpful
      // diagnostic (e.g. the "did you mean" subpath suggestion).
      Err(err) => PackageReq::from_str_loose(package_text).map_err(|_| err)?,
    };
    Ok(Ok(AddRmPackageReq {
      alias: maybe_alias.unwrap_or_else(|| req.name.clone()),
      req,
    }))
  }
}

pub async fn remove(
  flags: Arc<Flags>,
  remove_flags: RemoveFlags,
) -> Result<(), AnyError> {
  let cli_factory = CliFactory::from_flags(flags.clone());
  let Some(ende_json) = cli_factory.cli_options()?.start_dir.member_ende_json()
  else {
    log::info!("No packages were removed");
    return Ok(());
  };
  let mut config = ConfigUpdater::new(url_to_file_path(&ende_json.specifier)?)?;

  let mut removed_packages = vec![];

  for package in &remove_flags.packages {
    let req = AddRmPackageReq::parse(package, false)
      .with_context(|| format!("Failed to parse package: {}", package))?;
    let name = match &req {
      Ok(rm_pkg) => &rm_pkg.alias,
      // An alias or a package name without registry/version
      // constraints. Try to remove the package anyway.
      Err(pkg) => &pkg.name,
    };
    if config.remove(name) {
      removed_packages.push(name.clone());
    }
  }

  if removed_packages.is_empty() {
    log::info!("No packages were removed");
  } else {
    for package in &removed_packages {
      log::info!("Removed {}", crate::colors::green(package));
    }
    config.commit()?;

    install_after_modification(
      flags,
      None,
      CacheTopLevelDepsOptions {
        lockfile_only: remove_flags.lockfile_only,
        additional_roots: vec![],
      },
    )
    .await?;
  }

  Ok(())
}

async fn install_after_modification(
  flags: Arc<Flags>,
  // explicitly provided to prevent redownloading
  jsr_resolver: Option<Arc<crate::jsr::JsrFetchResolver>>,
  cache_options: CacheTopLevelDepsOptions,
) -> Result<CliFactory, AnyError> {
  // make a new CliFactory to pick up the updated config file
  let cli_factory = CliFactory::from_flags(flags);
  let start = std::time::Instant::now();
  cache_deps::cache_top_level_deps(&cli_factory, jsr_resolver, cache_options)
    .await?;

  if let Some(install_reporter) = cli_factory.install_reporter()? {
    super::installer::print_install_report(start.elapsed(), install_reporter);
  }

  if let Some(lockfile) = cli_factory.maybe_lockfile().await? {
    lockfile.write_if_changed()?;
  }

  Ok(cli_factory)
}

/// If the user-supplied `spec` looks like a path to a local JSR package
/// directory (relative or absolute, currently exists, and contains a
/// `ende.json`(c)), return a message suggesting `ende link` instead.
fn link_hint_for_spec(cwd: &Path, spec: &str) -> Option<String> {
  // Skip anything with a registry prefix.
  if spec.contains(':') {
    return None;
  }
  // Only treat path-shaped inputs as candidates: `.` / `..` / contains a
  // separator. This avoids false positives on bare package names like `foo`.
  let looks_pathy = spec == "."
    || spec == ".."
    || spec.starts_with("./")
    || spec.starts_with("../")
    || spec.starts_with(".\\")
    || spec.starts_with("..\\")
    || spec.contains('/')
    || spec.contains('\\')
    || Path::new(spec).is_absolute();
  if !looks_pathy {
    return None;
  }
  let abs = resolve_link_path(cwd, spec);
  if !abs.is_dir() {
    return None;
  }
  // Only hint when the directory is actually linkable. `ende link` requires
  // the target's ende.json(c) to exist *and* declare a "name", so check the
  // same condition here. Otherwise the hint dead-ends: `ende add ./pkg` ->
  // "Did you mean `ende link ./pkg`?" -> `ende link ./pkg` -> "its ende.json
  // has no name field".
  read_link_target_name(&abs)?;
  Some(format!(
    "'{}' looks like a local package directory. Did you mean `{}`?",
    spec,
    crate::colors::yellow(format!("ende link {spec}"))
  ))
}

/// Convert a `Path` into the string form we store in the `"links"` array.
///
/// Always uses forward slashes (the `"links"` field is treated as a portable
/// relative path), and strips a leading `./`.
fn path_to_link_string(path: &Path) -> String {
  let s = path
    .to_string_lossy()
    .replace(std::path::MAIN_SEPARATOR, "/");
  s.strip_prefix("./").map(|s| s.to_string()).unwrap_or(s)
}

/// Read the `"name"` field from the config file in the given directory.
fn read_link_target_name(dir: &Path) -> Option<String> {
  for filename in ende_config::ende_json::CONFIG_FILE_NAMES {
    let path = dir.join(filename);
    let Ok(text) = std::fs::read_to_string(&path) else {
      continue;
    };
    // the first config file found is the one that's used
    let parsed = jsonc_parser::parse_to_serde_value::<serde_json::Value>(
      &text,
      &Default::default(),
    );
    return parsed
      .ok()
      .and_then(|v| v.get("name")?.as_str().map(|n| n.to_string()));
  }
  None
}

/// Resolve a user-supplied path to an absolute, normalized form, relative to
/// `cwd` when not already absolute.
fn resolve_link_path(cwd: &Path, raw: &str) -> PathBuf {
  let candidate = Path::new(raw);
  let abs = if candidate.is_absolute() {
    candidate.to_path_buf()
  } else {
    cwd.join(candidate)
  };
  deno_path_util::normalize_path(std::borrow::Cow::Owned(abs)).into_owned()
}

fn load_ende_config_for_link(
  flags: &Arc<Flags>,
  create_if_missing: bool,
) -> Result<(CliFactory, ConfigUpdater), AnyError> {
  if create_if_missing {
    // For `link`, force creation of ende.json if missing; the `"links"` field
    // lives there. We achieve this by claiming jsr specifiers are present,
    // which makes `load_configs` materialise an ende.json when one isn't found.
    return load_config(flags);
  }

  // For `unlink`, never create an ende.json: if there isn't one, there's
  // nothing to unlink. Load only an existing config so a failed unlink can't
  // leave an empty ende.json behind.
  let cli_factory = CliFactory::from_flags(flags.clone());
  let options = cli_factory.cli_options()?;
  let ende_json_config = match options.start_dir.member_ende_json() {
    Some(ende_json) => {
      ConfigUpdater::new(url_to_file_path(&ende_json.specifier)?)?
    }
    None => bail!("No ende.json found; there are no linked packages to unlink"),
  };
  Ok((cli_factory, ende_json_config))
}

pub async fn link(
  flags: Arc<Flags>,
  link_flags: LinkFlags,
) -> Result<(), AnyError> {
  let (cli_factory, mut ende_json_config) =
    load_ende_config_for_link(&flags, true)?;
  let cwd = cli_factory.cli_options()?.initial_cwd().to_path_buf();
  let config_dir = ende_json_config
    .path
    .parent()
    .ok_or_else(|| {
      ende_core::anyhow::anyhow!(
        "Could not determine directory of '{}'",
        ende_json_config.path.display()
      )
    })?
    .to_path_buf();

  // Validate every path and compute the relative form we'll store.
  let mut resolved = Vec::with_capacity(link_flags.paths.len());
  for raw_path in &link_flags.paths {
    let abs = resolve_link_path(&cwd, raw_path);
    if !abs.exists() {
      bail!("Cannot link '{}': directory does not exist", raw_path);
    }
    if !abs.is_dir() {
      bail!("Cannot link '{}': not a directory", raw_path);
    }
    let has_config = ende_config::ende_json::CONFIG_FILE_NAMES
      .iter()
      .any(|name| abs.join(name).exists());
    if !has_config {
      bail!(
        "Cannot link '{}': no ende.json found in directory. \
         A linked package must contain an ende.json with a \"name\" field.",
        raw_path
      );
    }
    if read_link_target_name(&abs).is_none() {
      bail!(
        "Cannot link '{}': its ende.json has no \"name\" field. \
         A linked package must declare a JSR-style \"name\" to be importable.",
        raw_path
      );
    }
    let rel = pathdiff::diff_paths(&abs, &config_dir).unwrap_or(abs);
    resolved.push((raw_path.clone(), path_to_link_string(&rel)));
  }

  // Listing the path in "links" is all that's needed: a linked package is
  // importable by its bare "name" just like a workspace member, so there's no
  // `imports` entry to wire up.
  let links_arr = ende_json_config.root_object.array_value_or_set("links");
  let existing: Vec<String> = links_arr
    .elements()
    .iter()
    .filter_map(|el| el.as_string_lit())
    .filter_map(|s| s.decoded_value().ok())
    .collect();

  let mut changed = false;
  for (display, rel_str) in &resolved {
    if existing.contains(rel_str) {
      log::info!("{} is already linked", crate::colors::yellow(display));
      continue;
    }
    links_arr.append(jsonc_parser::cst::CstInputValue::String(rel_str.clone()));
    log::info!(
      "Link {} {}",
      crate::colors::green(display),
      crate::colors::gray(format!("({})", rel_str))
    );
    changed = true;
  }

  if changed {
    ende_json_config.modified = true;
    ende_json_config.commit()?;
    install_after_modification(
      flags,
      None,
      CacheTopLevelDepsOptions {
        lockfile_only: link_flags.lockfile_only,
        additional_roots: vec![],
      },
    )
    .await?;
  }

  Ok(())
}

pub async fn unlink(
  flags: Arc<Flags>,
  unlink_flags: UnlinkFlags,
) -> Result<(), AnyError> {
  let (cli_factory, mut ende_json_config) =
    load_ende_config_for_link(&flags, false)?;
  // Resolve path arguments relative to the cwd, matching how `link()` resolves
  // them, so `ende unlink ../pkg` finds an entry created by `ende link ../pkg`
  // from the same directory regardless of where the ende.json lives.
  let cwd = cli_factory.cli_options()?.initial_cwd().to_path_buf();
  let config_dir = ende_json_config
    .path
    .parent()
    .ok_or_else(|| {
      ende_core::anyhow::anyhow!(
        "Could not determine directory of '{}'",
        ende_json_config.path.display()
      )
    })?
    .to_path_buf();

  let Some(links_arr) = ende_json_config.root_object.array_value("links")
  else {
    bail!(
      "No \"links\" entries found in '{}'",
      ende_json_config.display_path()
    );
  };

  let mut removed = Vec::new();
  for arg in &unlink_flags.names_or_paths {
    let mut matched = false;
    // Snapshot of current elements (each iteration may mutate the array).
    let elements = links_arr.elements();
    for element in elements {
      let Some(lit) = element.as_string_lit() else {
        continue;
      };
      let Ok(value) = lit.decoded_value() else {
        continue;
      };

      let entry_dir = config_dir.join(&value);
      let entry_dir =
        deno_path_util::normalize_path(std::borrow::Cow::Owned(entry_dir))
          .into_owned();
      let target_name = read_link_target_name(&entry_dir);

      // Match by literal entry, by resolved path, or by JSR name.
      let is_match = value == *arg
        || entry_dir == resolve_link_path(&cwd, arg)
        || target_name.as_deref() == Some(arg.as_str());

      if is_match {
        element.remove();
        removed.push(arg.clone());
        matched = true;
        break;
      }
    }
    if !matched {
      log::warn!("No linked package matched '{}'", arg);
    }
  }

  if removed.is_empty() {
    // Nothing matched any argument. Fail (exit 1) rather than reporting
    // success, so a typo'd name or path is detectable from scripts and CI.
    bail!(
      "No linked packages matched the given {}",
      if unlink_flags.names_or_paths.len() == 1 {
        "name or path"
      } else {
        "names or paths"
      }
    );
  }

  // If the "links" array is now empty, remove the property entirely.
  if links_arr.elements().is_empty()
    && let Some(prop) = ende_json_config.root_object.get("links")
  {
    prop.remove();
  }

  for name in &removed {
    log::info!("Unlink {}", crate::colors::green(name));
  }

  ende_json_config.modified = true;
  ende_json_config.commit()?;

  install_after_modification(
    flags,
    None,
    CacheTopLevelDepsOptions {
      lockfile_only: unlink_flags.lockfile_only,
      additional_roots: vec![],
    },
  )
  .await?;

  Ok(())
}

#[cfg(test)]
mod test {
  use super::*;

  fn jsr_pkg_req(alias: &str, req: &str) -> AddRmPackageReq {
    AddRmPackageReq {
      alias: alias.into(),
      req: PackageReq::from_str_loose(req).unwrap(),
    }
  }

  #[test]
  fn test_parse_add_package_req() {
    let cases = [
      (("jsr:foo", false), jsr_pkg_req("foo", "foo")),
      (("alias@jsr:foo", false), jsr_pkg_req("alias", "foo")),
      (
        ("@alias/pkg@jsr:foo", false),
        jsr_pkg_req("@alias/pkg", "foo"),
      ),
      (
        ("alias@jsr:foo@^1.5.0", false),
        jsr_pkg_req("alias", "foo@^1.5.0"),
      ),
      (("foo", true), jsr_pkg_req("foo", "foo")),
      (("jsr:foo", true), jsr_pkg_req("foo", "foo")),
      (("my-alias@jsr:foo", true), jsr_pkg_req("my-alias", "foo")),
      // unprefixed packages are jsr packages
      (
        ("@scope/pkg", true),
        jsr_pkg_req("@scope/pkg", "@scope/pkg"),
      ),
      // range syntax is accepted on the command line (issue #26587)
      (
        ("jsr:@std/path@>=1.0.0", true),
        jsr_pkg_req("@std/path", "@std/path@>=1.0.0"),
      ),
      (
        ("alias@jsr:@std/path@>=1 <2", false),
        jsr_pkg_req("alias", "@std/path@>=1 <2"),
      ),
    ];

    for ((input, default_jsr), expected) in cases {
      let s = format!("on input: {input}, default_jsr: {default_jsr}");
      assert_eq!(
        AddRmPackageReq::parse(input, default_jsr).unwrap().unwrap(),
        expected,
        "{s}",
      );
    }

    assert_eq!(
      AddRmPackageReq::parse("@scope/pkg@tag", false)
        .unwrap()
        .unwrap_err()
        .to_string(),
      "@scope/pkg@tag",
    );
  }

  #[test]
  fn test_parse_npm_package_req_errors() {
    for input in ["npm:chalk", "npm:chalk@^4", "alias@npm:chalk"] {
      for default_jsr in [false, true] {
        assert_eq!(
          AddRmPackageReq::parse(input, default_jsr)
            .unwrap_err()
            .to_string(),
          format!("npm packages are not supported in Ende (found '{input}')."),
        );
      }
    }
  }

  #[test]
  fn test_use_unscoped_alias() {
    let cases = [
      (
        ("jsr:@david/jsonc-morph", false),
        jsr_pkg_req("jsonc-morph", "@david/jsonc-morph"),
      ),
      (
        ("jsr:@std/path@^1.0.0", false),
        jsr_pkg_req("path", "@std/path@^1.0.0"),
      ),
      (("@scope/pkg", true), jsr_pkg_req("pkg", "@scope/pkg")),
      // an explicit alias always wins
      (
        ("my-alias@jsr:@david/jsonc-morph", false),
        jsr_pkg_req("my-alias", "@david/jsonc-morph"),
      ),
      // unscoped names are unaffected
      (("jsr:foo", false), jsr_pkg_req("foo", "foo")),
    ];

    for ((input, default_jsr), expected) in cases {
      let s = format!("on input: {input}, default_jsr: {default_jsr}");
      let mut req =
        AddRmPackageReq::parse(input, default_jsr).unwrap().unwrap();
      req.use_unscoped_alias();
      assert_eq!(req, expected, "{s}");
    }
  }
}
