// Copyright 2018-2026 the Ende authors. MIT license.

//! What a sandbox's packages ask for, and what the lockfile pins about a
//! rule.
//!
//! - **Entries**: the sandbox's JavaScript and TypeScript modules that the
//!   app imports (a module outside every sandbox, statically or with a
//!   literal `import()` the graph holds), by the specifier the import
//!   resolved to; one per module.
//! - **Manifests**: an entry may declare `export const manifest = {
//!   permissions: … }`, as an extension does. A sandbox with several entries
//!   runs with what every manifest allows: grant ∩ each manifest ∩ the app.
//!   Each manifest can only narrow, so counting all of them is safe, and no
//!   entry decides for the others.
//! - **Pins** (the lockfile's `isolate` section, keyed by the rule as
//!   written, normalized lexically; see `SandboxRegistry::rule_key`): the
//!   grant as written, and the approved runs, each as the grant it was
//!   approved under and the manifests it imported. A run may start when
//!   the grant is the recorded one and what it gets is within what one
//!   approved run got; otherwise `ende install --entrypoint` approves it.
//!   Entry names decide nothing (the per-entry manifests are kept for the
//!   review text), so another spelling, version or rename can't widen a
//!   sandbox, and nothing `ende install` can't load gets it stuck.

use std::collections::BTreeMap;

use deno_error::JsErrorBox;
use deno_graph::ModuleGraph;
use ende_core::ModuleSpecifier;
use ende_core::serde_json::Value;
use ende_core::url::Url;
use ende_lockfile::IsolateLockEntry;
use ende_lockfile::IsolateRunPin;
use ende_lockfile::IsolateYield;
use ende_runtime::ende_permissions::PermissionDescriptorParser;
use ende_runtime::ende_permissions::PermissionsOptions;
use ende_runtime::ende_permissions::UngrantedPermission;

use super::SandboxId;
use crate::extensions::PackageManifest;
use crate::extensions::extract_package_manifest;

/// The entries of sandbox `id` in `graph`: what each import from outside
/// every sandbox resolved to, with the module it names. `sandbox_of` puts
/// a specifier under its rule. In a type-checked graph an import from the
/// app goes to a type stub (see `super::types`), which stands for the entry
/// it types.
pub fn sandbox_entries(
  graph: &ModuleGraph,
  id: SandboxId,
  sandbox_of: &dyn Fn(&ModuleSpecifier) -> Option<SandboxId>,
) -> BTreeMap<ModuleSpecifier, ModuleSpecifier> {
  let mut entries = BTreeMap::new();
  for module in graph.modules() {
    let deno_graph::Module::Js(module) = module else {
      continue;
    };
    if super::types::stub_entry(&module.specifier).is_some()
      || sandbox_of(&module.specifier).is_some()
    {
      continue;
    }
    for dependency in module.dependencies.values() {
      let Some(code) = dependency.get_code() else {
        continue;
      };
      let entry =
        super::types::stub_entry(code).unwrap_or_else(|| code.clone());
      if sandbox_of(&entry) != Some(id) {
        continue;
      }
      let target = graph.resolve(&entry).clone();
      if matches!(graph.get(&target), Some(deno_graph::Module::Js(_))) {
        entries.insert(entry, target);
      }
    }
  }
  entries
}

/// How the lockfile names `entry` (an import's resolved specifier, naming
/// the module `target`): a `jsr:` import by its package, the version it
/// resolved to and its export (`jsr:@scope/name@1.2.0/sub`), so that
/// `ende install` reads it again exactly; a URL as is; a local file by its
/// path relative to `root_dir` (the folder of the root ende.json), so the
/// lockfile can be shared. Another version or spelling is another entry.
/// Entries are for review only: runs are held to the approved runs (see
/// [`decide_run`]), and modules to their rule by `super::PinChecks`.
pub fn entry_key(
  entry: &ModuleSpecifier,
  target: &ModuleSpecifier,
  root_dir: &Url,
  jsr_url: &Url,
) -> String {
  if entry.scheme() == "jsr"
    && let Ok(reference) =
      deno_semver::jsr::JsrPackageReqReference::from_specifier(entry)
    && let Some(nv) = deno_graph::source::recommended_registry_package_url_to_nv(
      jsr_url, target,
    )
  {
    return match reference.sub_path() {
      Some(sub_path) => format!("jsr:{nv}/{sub_path}"),
      None => format!("jsr:{nv}"),
    };
  }
  if entry.scheme() != "file" {
    return entry.to_string();
  }
  match root_dir.make_relative(entry) {
    Some(relative) if relative.starts_with("../") => relative,
    Some(relative) => format!("./{relative}"),
    None => entry.to_string(),
  }
}

/// The manifest the module `target` of `graph`, an entry of the sandbox of
/// `isolate[rule]`, declares, if any.
pub fn read_manifest(
  rule: &str,
  graph: &ModuleGraph,
  target: &ModuleSpecifier,
) -> Result<Option<PackageManifest>, JsErrorBox> {
  let Some(deno_graph::Module::Js(module)) = graph.get(target) else {
    return Ok(None);
  };
  extract_package_manifest(
    rule,
    &module.specifier,
    module.media_type,
    &module.source.text,
  )
  .map_err(JsErrorBox::from_err)
}

/// What a sandbox granted `grant` runs with when its entries declare
/// `manifests`: what the grant and every manifest allow (the grant's deny
/// lists kept), before the app's own permissions are applied. Also a
/// warning for each manifest that asks for something the grant doesn't
/// give, by entry.
pub fn effective_options<'a>(
  rule: &str,
  grant: &PermissionsOptions,
  manifests: impl Iterator<Item = (&'a str, &'a PackageManifest)>,
  parser: &dyn PermissionDescriptorParser,
) -> Result<(PermissionsOptions, Vec<String>), JsErrorBox> {
  let to_error =
    |err: ende_runtime::ende_permissions::IntersectPermissionsError| {
      JsErrorBox::type_error(format!("isolate[\"{rule}\"]: {err}"))
    };
  let mut options = grant.clone();
  let mut warnings = Vec::new();
  for (entry, manifest) in manifests {
    let (_, ungranted) = grant
      .intersect_requested(&manifest.permissions, parser)
      .map_err(to_error)?;
    if let Some(warning) = ungranted_warning(rule, entry, &ungranted) {
      warnings.push(warning);
    }
    options = options
      .intersect_requested(&manifest.permissions, parser)
      .map_err(to_error)?
      .0;
  }
  options.prompt = false;
  Ok((options, warnings))
}

/// One warning naming every permission the manifest of `entry` asks for
/// that ende.json doesn't grant `isolate[rule]`, and where to grant it.
fn ungranted_warning(
  rule: &str,
  entry: &str,
  ungranted: &[UngrantedPermission],
) -> Option<String> {
  if ungranted.is_empty() {
    return None;
  }
  let mut warning = format!(
    "The manifest of {entry} (in the sandbox of isolate[\"{rule}\"]) asks for permissions that ende.json doesn't grant, so using them will be denied:"
  );
  for permission in ungranted {
    match &permission.entry {
      Some(entry) => warning.push_str(&format!(
        "\n  {} \"{entry}\": add it to isolate[\"{rule}\"].permissions.{}",
        permission.name, permission.name
      )),
      None => warning.push_str(&format!(
        "\n  {}: add \"{}\" to isolate[\"{rule}\"].permissions",
        permission.name, permission.name
      )),
    }
  }
  Some(warning)
}

/// What the lockfile would record about an `isolate` rule, and what of
/// it needs approval first (see [`decide_run`], [`decide_install`] and
/// [`crate::pins`]).
pub struct RulePin {
  /// Its pin's key (see `SandboxRegistry::rule_key`).
  pub key: String,
  pub matcher: String,
  pub record: IsolateLockEntry,
  /// What needs approval first; empty when nothing does.
  pub needs_approval: Vec<String>,
  /// The change, for `ende install` to show.
  pub description: String,
  /// The command that approves what needs approval.
  pub approve_command: String,
  /// What it runs with, for the notice.
  pub options: PermissionsOptions,
  /// Whether recording it prints the notice.
  pub notice: bool,
  /// Whether recording keeps this pin: false for a rule ende.json no longer
  /// declares, whose removal is what gets approved.
  pub keep: bool,
  /// The pins (keys) this rule takes modules from: recording it makes each
  /// of them yield to this rule.
  pub takes_from: Vec<String>,
}

impl RulePin {
  /// The removal of the pin keyed `key`, of a rule ende.json no longer
  /// declares. It needs approval: what the rule sandboxed would run in the
  /// app.
  pub fn removal(key: &str, pin: &IsolateLockEntry) -> Self {
    Self {
      key: key.to_string(),
      matcher: key.to_string(),
      record: pin.clone(),
      needs_approval: vec![
        "it is no longer declared in ende.json, so what it sandboxed would run in the app".to_string(),
      ],
      description: format!(
        "isolate[\"{key}\"] changed:\n  removed from ende.json: what it sandboxed now runs in the app, or under another rule that covers it ({} approved runs dropped)",
        pin.runs.len()
      ),
      approve_command: "ende install".to_string(),
      options: Default::default(),
      notice: false,
      keep: false,
      takes_from: Vec::new(),
    }
  }
}

impl crate::pins::Pinned for RulePin {
  type Entry = IsolateLockEntry;
  const KIND: &'static str = "isolate rule";
  const PLURAL: &'static str = "sandboxes";
  const PINNED_PARTS: &'static str = "its grant or its packages' manifests";
  const PROTECTS: &'static str = "its grant and its packages' manifests";

  fn name(&self) -> String {
    format!("isolate[\"{}\"]", self.matcher)
  }

  fn title(&self) -> String {
    self.name()
  }

  fn options(&self) -> &PermissionsOptions {
    &self.options
  }

  fn approved(
    &self,
    lockfile: &ende_lockfile::Lockfile,
  ) -> Option<Self::Entry> {
    lockfile.isolate_rule(&self.key).cloned()
  }

  fn record(
    &self,
    lockfile: &mut ende_lockfile::Lockfile,
    mut entry: Self::Entry,
  ) {
    if !self.keep {
      lockfile.retain_isolate_rules(|key| key != self.key);
      // What other pins yield to it goes with it: a rule declared again
      // under this key takes nothing they cover unapproved.
      let yielding = lockfile
        .content
        .isolate
        .iter()
        .filter(|(_, pin)| {
          pin.yields_to.iter().any(|yielded| yielded.rule == self.key)
        })
        .map(|(key, pin)| (key.clone(), pin.clone()))
        .collect::<Vec<_>>();
      for (key, mut pin) in yielding {
        pin.yields_to.retain(|yielded| yielded.rule != self.key);
        lockfile.insert_isolate_rule(key, pin);
      }
      return;
    }
    // What another item recorded first (this pin yielding to it) stays.
    if let Some(current) = lockfile.isolate_rule(&self.key) {
      for yielded in &current.yields_to {
        set_yield(&mut entry.yields_to, yielded.clone());
      }
    }
    let yielded = IsolateYield {
      rule: self.key.clone(),
      grant: entry.grant.clone(),
    };
    lockfile.insert_isolate_rule(self.key.clone(), entry);
    for other in &self.takes_from {
      if let Some(mut pin) = lockfile.isolate_rule(other).cloned() {
        set_yield(&mut pin.yields_to, yielded.clone());
        lockfile.insert_isolate_rule(other.clone(), pin);
      }
    }
  }

  fn pin(&self, _approved: Option<&Self::Entry>) -> Self::Entry {
    self.record.clone()
  }

  fn changed_parts(&self, _approved: Option<&Self::Entry>) -> Vec<String> {
    self.needs_approval.clone()
  }

  fn describe_change(&self, _approved: Option<&Self::Entry>) -> String {
    self.description.clone()
  }

  fn approve_command(&self) -> String {
    self.approve_command.clone()
  }

  fn notice(&self) -> bool {
    self.notice
  }
}

/// Puts `yielded` in `list`, in place of what it held for that rule.
fn set_yield(list: &mut Vec<IsolateYield>, yielded: IsolateYield) {
  match list.iter_mut().find(|other| other.rule == yielded.rule) {
    Some(other) => *other = yielded,
    None => list.push(yielded),
  }
}

/// The run that imports entries with `manifests` (each entry's manifest's
/// `permissions`, `null` for none) under the grant `grant_json`.
pub fn run_of<'a>(
  grant_json: &Value,
  manifests: impl Iterator<Item = &'a Value>,
) -> IsolateRunPin {
  let mut list = Vec::new();
  for manifest in manifests {
    if !manifest.is_null() && !list.contains(manifest) {
      list.push(manifest.clone());
    }
  }
  list.sort_by_key(|manifest| manifest.to_string());
  IsolateRunPin {
    grant: grant_json.clone(),
    manifests: list,
  }
}

/// A manifest's `permissions` as the lockfile records them, read back.
fn manifest_from_json(
  rule: &str,
  permissions: &Value,
) -> Result<PackageManifest, JsErrorBox> {
  let specifier = ModuleSpecifier::parse("file:///ende.lock").unwrap();
  extract_package_manifest(
    rule,
    &specifier,
    deno_ast::MediaType::TypeScript,
    &format!("export const manifest = {{ permissions: {permissions} }};"),
  )
  .map_err(JsErrorBox::from_err)?
  .ok_or_else(|| {
    JsErrorBox::type_error(format!(
      "The lockfile's pin of isolate[\"{rule}\"] holds a manifest that can't be read: {permissions}"
    ))
  })
}

/// What `run` got, under the rule's grant now (`grant`): that ∩ the grant
/// it was approved under ∩ each of its manifests.
fn run_options(
  rule: &str,
  grant: &PermissionsOptions,
  run: &IsolateRunPin,
  root_dir: &Url,
  parser: &dyn PermissionDescriptorParser,
) -> Result<PermissionsOptions, JsErrorBox> {
  let invalid = |err: &dyn std::fmt::Display| {
    JsErrorBox::type_error(format!(
      "The lockfile's pin of isolate[\"{rule}\"] is invalid: {err}"
    ))
  };
  let base = root_dir.join("ende.json").map_err(|err| invalid(&err))?;
  let approved_grant = crate::args::extension_grant_permissions_options(
    &ende_config::ende_json::grant_from_json(run.grant.clone(), &base)
      .map_err(|err| invalid(&err))?,
  )
  .map_err(|err| invalid(&err))?;
  let (mut options, _) = grant
    .intersect_requested(&approved_grant, parser)
    .map_err(|err| invalid(&err))?;
  options.prompt = false;
  let manifests = run
    .manifests
    .iter()
    .map(|permissions| manifest_from_json(rule, permissions))
    .collect::<Result<Vec<_>, _>>()?;
  Ok(
    effective_options(
      rule,
      &options,
      manifests.iter().map(|manifest| ("", manifest)),
      parser,
    )?
    .0,
  )
}

/// `Ok` if `options` is within what one of `runs` got; otherwise what the
/// closest one lacks. A run that can't be read here (a manifest path of
/// another OS, like `C:\\data` on macOS) doesn't match, and isn't an error:
/// another run may still match, and nothing removes it.
fn fits(
  rule: &str,
  grant: &PermissionsOptions,
  options: &PermissionsOptions,
  runs: &[IsolateRunPin],
  root_dir: &Url,
  parser: &dyn PermissionDescriptorParser,
) -> Result<Result<(), Vec<UngrantedPermission>>, JsErrorBox> {
  let mut closest: Option<Vec<UngrantedPermission>> = None;
  for run in runs {
    let Ok(approved) = run_options(rule, grant, run, root_dir, parser) else {
      continue;
    };
    let (_, more) =
      approved
        .intersect_requested(options, parser)
        .map_err(|err| {
          JsErrorBox::type_error(format!("isolate[\"{rule}\"]: {err}"))
        })?;
    if more.is_empty() {
      return Ok(Ok(()));
    }
    if closest
      .as_ref()
      .is_none_or(|closest| more.len() < closest.len())
    {
      closest = Some(more);
    }
  }
  Ok(Err(closest.unwrap_or_default()))
}

/// What [`decide_run`] works from.
pub struct RunInputs<'a> {
  pub key: String,
  pub matcher: &'a str,
  /// The grant as written, and as it reads.
  pub grant_json: &'a Value,
  pub grant: &'a PermissionsOptions,
  /// What this run gets: the grant ∩ every manifest it imports.
  pub options: &'a PermissionsOptions,
  /// The entries this run imports, by entry key, with the `permissions` of
  /// each one's manifest (`null` for none).
  pub manifests: &'a BTreeMap<String, Value>,
  /// What a first pin yields to (see `SandboxRegistry::first_yields`).
  pub yields_to: Vec<IsolateYield>,
  pub root_dir: &'a Url,
  /// `ende install --entrypoint` with this run's entry points.
  pub entrypoint_command: String,
  /// Whether this run printed no notice for the rule yet.
  pub notice: bool,
}

/// Whether a run may start, and what it records:
///
/// - a rule with no pin is trusted on first use (unless it takes a module
///   another rule's pin covers, see `super::PinChecks`): its grant, this
///   run, its manifests and the rules it yields to are recorded;
/// - otherwise the grant must be the one recorded, and what this run gets
///   must be within what one approved run got. Anything else needs
///   `ende install --entrypoint` with this run's entry points, which
///   approves this run. A run that fits writes nothing.
pub fn decide_run(
  inputs: RunInputs<'_>,
  approved: Option<&IsolateLockEntry>,
  parser: &dyn PermissionDescriptorParser,
) -> Result<RulePin, JsErrorBox> {
  let rule = inputs.matcher;
  let run = run_of(inputs.grant_json, inputs.manifests.values());
  let mut manifests = approved
    .map(|approved| approved.manifests.clone())
    .unwrap_or_default();
  let mut lines = Vec::new();
  for (key, manifest) in inputs.manifests {
    if let Some(old) = manifests.get(key)
      && old != manifest
    {
      lines.push(format!(
        "  manifest of {key} (asks for):\n    - {}\n    + {}",
        shown_manifest(old),
        shown_manifest(manifest)
      ));
    }
    manifests.insert(key.clone(), manifest.clone());
  }
  let mut needs = Vec::new();
  let record = match approved {
    None => IsolateLockEntry {
      grant: inputs.grant_json.clone(),
      runs: vec![run.clone()],
      manifests,
      yields_to: inputs.yields_to,
    },
    Some(approved) => {
      if approved.grant != *inputs.grant_json {
        needs.push("its grant in ende.json".to_string());
        lines.push(format!(
          "  grant (ende.json):\n    - {}\n    + {}",
          approved.grant, inputs.grant_json
        ));
      } else if let Err(more) = fits(
        rule,
        inputs.grant,
        inputs.options,
        &approved.runs,
        inputs.root_dir,
        parser,
      )? {
        needs.push(format!(
          "this run would get more than any run ende.lock approved ({})",
          describe_ungranted(&more)
        ));
      }
      approved.clone()
    }
  };
  lines.push(format!(
    "  this run (asks for: {}) gets: {}",
    shown_run(&run),
    crate::pins::describe_permissions(inputs.options)
  ));
  Ok(RulePin {
    key: inputs.key,
    matcher: rule.to_string(),
    record,
    needs_approval: needs,
    description: describe(rule, &lines),
    approve_command: inputs.entrypoint_command,
    options: inputs.options.clone(),
    notice: inputs.notice && approved.is_none(),
    keep: true,
    takes_from: Vec::new(),
  })
}

/// What [`decide_install`] works from.
pub struct InstallInputs<'a> {
  pub key: String,
  pub matcher: &'a str,
  pub grant_json: &'a Value,
  pub grant: &'a PermissionsOptions,
  /// Modules another pin covers (that pin's key, the module as shown) that
  /// now fall under this rule: moving them needs approval.
  pub moves: Vec<(String, String)>,
  /// What a first pin yields to (see `SandboxRegistry::first_yields`).
  pub yields_to: Vec<IsolateYield>,
  /// With `--entrypoint`: each entry point's run, with what it gets and its
  /// entries' manifests.
  pub runs: Vec<(String, PermissionsOptions, BTreeMap<String, Value>)>,
  pub root_dir: &'a Url,
}

/// What `ende install` records for a rule. It never removes an approved
/// run: a grant change keeps them (each is held to the grant it was
/// approved under too, so a narrower grant narrows it and a wider one
/// doesn't widen it), and only `--entrypoint` adds runs, which needs
/// approval unless the rule had no pin.
pub fn decide_install(
  inputs: InstallInputs<'_>,
  approved: Option<&IsolateLockEntry>,
  parser: &dyn PermissionDescriptorParser,
) -> Result<Option<RulePin>, JsErrorBox> {
  let rule = inputs.matcher;
  let mut needs = Vec::new();
  let mut lines = Vec::new();
  let base = approved.cloned();
  let mut takes_from = Vec::new();
  for (other, module) in &inputs.moves {
    needs.push(format!("it takes over {module} from isolate[\"{other}\"]"));
    lines.push(format!(
      "  takes over {module}, which isolate[\"{other}\"] sandboxed"
    ));
    if !takes_from.contains(other) {
      takes_from.push(other.clone());
    }
  }
  if base.is_none() && inputs.runs.is_empty() && takes_from.is_empty() {
    return Ok(None);
  }
  let mut record = base.clone().unwrap_or_else(|| IsolateLockEntry {
    grant: inputs.grant_json.clone(),
    runs: Vec::new(),
    manifests: Default::default(),
    yields_to: inputs.yields_to.clone(),
  });
  if let Some(base) = &base
    && base.grant != *inputs.grant_json
  {
    if approved.is_some() {
      needs.push("its grant in ende.json".to_string());
    }
    lines.push(format!(
      "  grant (ende.json):\n    - {}\n    + {}",
      base.grant, inputs.grant_json
    ));
  }
  record.grant = inputs.grant_json.clone();
  let mut options_shown = PermissionsOptions::default();
  for (root, options, manifests) in &inputs.runs {
    for (key, manifest) in manifests {
      if let Some(old) = record.manifests.get(key)
        && old != manifest
      {
        lines.push(format!(
          "  manifest of {key} (asks for):\n    - {}\n    + {}",
          shown_manifest(old),
          shown_manifest(manifest)
        ));
      }
      record.manifests.insert(key.clone(), manifest.clone());
    }
    if fits(
      rule,
      inputs.grant,
      options,
      &record.runs,
      inputs.root_dir,
      parser,
    )?
    .is_ok()
    {
      continue;
    }
    let run = run_of(inputs.grant_json, manifests.values());
    if base.is_some() {
      needs.push(format!("a new run, from {root}"));
    }
    lines.push(format!(
      "  new run from {root} (asks for: {}): {}",
      shown_run(&run),
      crate::pins::describe_permissions(options)
    ));
    options_shown = options.clone();
    record.runs.push(run);
  }
  Ok(Some(RulePin {
    key: inputs.key,
    matcher: rule.to_string(),
    record,
    needs_approval: needs,
    description: describe(rule, &lines),
    approve_command: "ende install --entrypoint".to_string(),
    options: options_shown,
    notice: true,
    keep: true,
    takes_from,
  }))
}

fn describe(rule: &str, lines: &[String]) -> String {
  let mut description = format!("isolate[\"{rule}\"] changed:");
  for line in lines {
    description.push('\n');
    description.push_str(line);
  }
  description.push_str("\n  (never more than the app)");
  description
}

fn shown_run(run: &IsolateRunPin) -> String {
  if run.manifests.is_empty() {
    "no manifest".to_string()
  } else {
    run
      .manifests
      .iter()
      .map(|manifest| manifest.to_string())
      .collect::<Vec<_>>()
      .join(", ")
  }
}

fn shown_manifest(manifest: &Value) -> String {
  if manifest.is_null() {
    "(no manifest)".to_string()
  } else {
    manifest.to_string()
  }
}

fn describe_ungranted(ungranted: &[UngrantedPermission]) -> String {
  ungranted
    .iter()
    .map(|permission| match &permission.entry {
      Some(entry) => format!("{} \"{entry}\"", permission.name),
      None => permission.name.to_string(),
    })
    .collect::<Vec<_>>()
    .join(", ")
}

#[cfg(test)]
mod test {
  use pretty_assertions::assert_eq;

  use super::*;
  use crate::pins::Pinned;

  #[test]
  fn entry_keys_are_relative_for_local_files() {
    let root = Url::parse("file:///app/").unwrap();
    let jsr = Url::parse("https://jsr.io/").unwrap();
    // A jsr import is named by the version it resolved to.
    let entry = ModuleSpecifier::parse("jsr:@std/csv@^1/parse").unwrap();
    let target =
      ModuleSpecifier::parse("https://jsr.io/@std/csv/1.2.0/parse.ts").unwrap();
    assert_eq!(
      entry_key(&entry, &target, &root, &jsr),
      "jsr:@std/csv@1.2.0/parse"
    );
    for (entry, key) in [
      ("file:///app/vendor/a/mod.ts", "./vendor/a/mod.ts"),
      ("file:///shared/b/mod.ts", "../shared/b/mod.ts"),
      (
        "https://cdn.example.com/lib/x.js",
        "https://cdn.example.com/lib/x.js",
      ),
    ] {
      let entry = ModuleSpecifier::parse(entry).unwrap();
      assert_eq!(entry_key(&entry, &entry, &root, &jsr), key);
    }
  }

  fn parser()
  -> ende_runtime::ende_permissions::RuntimePermissionDescriptorParser<
    crate::sys::CliSys,
  > {
    ende_runtime::ende_permissions::RuntimePermissionDescriptorParser::new(
      crate::sys::CliSys::default(),
    )
  }

  /// A run of the rule granted read and env, importing entries with
  /// `manifests` (by key).
  fn run(
    manifests: &[(&str, Value)],
    approved: Option<&IsolateLockEntry>,
  ) -> RulePin {
    let grant_json = ende_core::serde_json::json!(["read", "env"]);
    let grant = crate::args::extension_grant_permissions_options(
      &ende_config::ende_json::ExtensionPermissions::Names(vec![
        "read".to_string(),
        "env".to_string(),
      ]),
    )
    .unwrap();
    let read = manifests
      .iter()
      .filter(|(_, manifest)| !manifest.is_null())
      .map(|(_, manifest)| manifest_from_json("./vendor/a/", manifest).unwrap())
      .collect::<Vec<_>>();
    let options = effective_options(
      "./vendor/a/",
      &grant,
      read.iter().map(|manifest| ("", manifest)),
      &parser(),
    )
    .unwrap()
    .0;
    let manifests = manifests
      .iter()
      .map(|(key, manifest)| (key.to_string(), manifest.clone()))
      .collect();
    decide_run(
      RunInputs {
        key: "./vendor/a/".to_string(),
        matcher: "./vendor/a/",
        grant_json: &grant_json,
        grant: &grant,
        options: &options,
        manifests: &manifests,
        yields_to: vec![IsolateYield {
          rule: "jsr:@x/y".to_string(),
          grant: ende_core::serde_json::json!([]),
        }],
        root_dir: &Url::parse("file:///app/").unwrap(),
        entrypoint_command: "ende install --entrypoint main.ts".to_string(),
        notice: true,
      },
      approved,
      &parser(),
    )
    .unwrap()
  }

  #[test]
  fn a_run_needs_an_approved_run_that_got_as_much() {
    let read = ende_core::serde_json::json!({ "read": true });
    let env = ende_core::serde_json::json!({ "env": true });
    let both = ende_core::serde_json::json!({ "read": true, "env": true });
    // First use: recorded with the notice.
    let first = run(&[("a.ts", read.clone())], None);
    assert_eq!(first.changed_parts(None), Vec::<String>::new());
    assert!(first.notice());
    let mut approved = first.pin(None);
    assert_eq!(approved.yields_to.len(), 1);
    // Another run, asking for env alone, approved.
    approved.runs.push(run_of(
      &ende_core::serde_json::json!(["read", "env"]),
      [&env].into_iter(),
    ));
    // Within one approved run: fine, and a new entry says nothing.
    let within = run(
      &[("a.ts", read.clone()), ("e.ts", env.clone())],
      Some(&approved),
    );
    assert_eq!(within.changed_parts(Some(&approved)), Vec::<String>::new());
    assert!(!within.notice());
    // Both in one run, or no manifest at all: no approved run got that.
    for manifests in [vec![("c.ts", both)], vec![("d.ts", Value::Null)]] {
      let wider = run(&manifests, Some(&approved));
      let parts = wider.changed_parts(Some(&approved));
      assert_eq!(parts.len(), 1, "{parts:?}");
      assert!(
        parts[0].starts_with(
          "this run would get more than any run ende.lock approved"
        ),
        "{parts:?}"
      );
      assert_eq!(wider.approve_command(), "ende install --entrypoint main.ts");
    }
    // A manifest that narrows fits: nothing to approve, nothing said, and
    // nothing written.
    let narrowed = run(
      &[("a.ts", ende_core::serde_json::json!([]))],
      Some(&approved),
    );
    assert_eq!(
      narrowed.changed_parts(Some(&approved)),
      Vec::<String>::new()
    );
    assert!(!narrowed.notice());
    assert_eq!(narrowed.pin(Some(&approved)), approved);
  }
}
