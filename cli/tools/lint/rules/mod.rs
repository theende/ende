// Copyright 2018-2026 the Deno authors. MIT license.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::LazyLock;

use deno_ast::ModuleSpecifier;
use deno_graph::ModuleGraph;
use deno_lint::diagnostic::LintDiagnostic;
use deno_lint::rules::LintRule;
use deno_lint::tags;
use deno_terminal::colors;
use ende_config::ende_json::LintRulesConfig;
use ende_config::workspace::WorkspaceDirectory;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::parking_lot::Mutex;
use ende_resolver::workspace::WorkspaceResolver;

use crate::sys::CliSys;

mod no_sloppy_imports;
mod no_slow_types;

// used for publishing
pub use no_slow_types::collect_no_slow_type_diagnostics;

pub trait PackageLintRule: std::fmt::Debug + Send + Sync {
  fn code(&self) -> &'static str;

  fn tags(&self) -> tags::Tags {
    &[]
  }

  fn help_docs_url(&self) -> Cow<'static, str>;

  fn lint_package(
    &self,
    graph: &ModuleGraph,
    entrypoints: &[ModuleSpecifier],
  ) -> Vec<LintDiagnostic>;
}

pub(super) trait ExtendedLintRule: LintRule {
  /// If the rule supports the incremental cache.
  fn supports_incremental_cache(&self) -> bool;

  fn help_docs_url(&self) -> Cow<'static, str>;

  fn into_base(self: Box<Self>) -> Box<dyn LintRule>;
}

pub enum FileOrPackageLintRule {
  File(Box<dyn LintRule>),
  Package(Box<dyn PackageLintRule>),
}

#[derive(Debug)]
enum CliLintRuleKind {
  DenoLint(Box<dyn LintRule>),
  Extended(Box<dyn ExtendedLintRule>),
  Package(Box<dyn PackageLintRule>),
}

#[derive(Debug)]
pub struct CliLintRule(CliLintRuleKind);

impl PartialEq for CliLintRule {
  fn eq(&self, other: &Self) -> bool {
    self.code() == other.code()
  }
}

impl CliLintRule {
  pub fn code(&self) -> &'static str {
    use CliLintRuleKind::*;
    match &self.0 {
      DenoLint(rule) => rule.code(),
      Extended(rule) => rule.code(),
      Package(rule) => rule.code(),
    }
  }

  pub fn tags(&self) -> Vec<&'static tags::Tag> {
    use CliLintRuleKind::*;
    let tags = match &self.0 {
      DenoLint(rule) => rule.tags(),
      Extended(rule) => rule.tags(),
      Package(rule) => rule.tags(),
    };
    tags.iter().filter(|t| **t != tags::FRESH).collect()
  }

  pub fn help_docs_url(&self) -> Cow<'static, str> {
    use CliLintRuleKind::*;
    match &self.0 {
      DenoLint(rule) => {
        Cow::Owned(format!("https://docs.ende.sh/lint/rules/{}", rule.code()))
      }
      Extended(rule) => rule.help_docs_url(),
      Package(rule) => rule.help_docs_url(),
    }
  }

  pub fn supports_incremental_cache(&self) -> bool {
    use CliLintRuleKind::*;
    match &self.0 {
      DenoLint(_) => true,
      Extended(rule) => rule.supports_incremental_cache(),
      // graph rules don't go through the incremental cache, so allow it
      Package(_) => true,
    }
  }

  pub fn into_file_or_pkg_rule(self) -> FileOrPackageLintRule {
    use CliLintRuleKind::*;
    match self.0 {
      DenoLint(rule) => FileOrPackageLintRule::File(rule),
      Extended(rule) => FileOrPackageLintRule::File(rule.into_base()),
      Package(rule) => FileOrPackageLintRule::Package(rule),
    }
  }
}

#[derive(Debug)]
pub struct ConfiguredRules {
  pub all_rule_codes: HashSet<Cow<'static, str>>,
  pub rules: Vec<CliLintRule>,
}

impl ConfiguredRules {
  pub fn supports_incremental_cache(&self) -> bool {
    self.rules.iter().all(|r| r.supports_incremental_cache())
  }

  pub fn incremental_cache_state(&self) -> impl std::hash::Hash + use<> {
    // use a hash of the rule names in order to bust the cache
    let mut codes = self.rules.iter().map(|r| r.code()).collect::<Vec<_>>();
    // ensure this is stable by sorting it
    codes.sort_unstable();
    Some(codes)
  }
}

/// Rules from deno_lint for the Fresh framework, which Ende doesn't include.
const FRESH_RULE_CODES: [&str; 2] =
  ["fresh-handler-export", "fresh-server-event-handlers"];

/// Rule codes Ende renamed, as `(old, new)`. The old codes still work in
/// `include`/`exclude`, with a warning.
const RENAMED_RULE_CODES: [(&str, &str); 1] =
  [("no-deprecated-deno-api", "no-deprecated-ende-api")];

pub struct LintRuleProvider {
  workspace_resolver: Option<Arc<WorkspaceResolver<CliSys>>>,
}

impl LintRuleProvider {
  pub fn new(
    workspace_resolver: Option<Arc<WorkspaceResolver<CliSys>>>,
  ) -> Self {
    Self { workspace_resolver }
  }

  pub fn resolve_lint_rules_err_empty(
    &self,
    rules: LintRulesConfig,
    maybe_workspace_dir: Option<&WorkspaceDirectory>,
  ) -> Result<ConfiguredRules, AnyError> {
    let lint_rules = self.resolve_lint_rules(rules, maybe_workspace_dir);
    if lint_rules.rules.is_empty() {
      bail!("No rules have been configured")
    }
    Ok(lint_rules)
  }

  pub fn all_rules(&self) -> Vec<CliLintRule> {
    let deno_lint_rules = deno_lint::rules::get_all_rules();
    let cli_lint_rules = vec![CliLintRule(CliLintRuleKind::Extended(
      Box::new(no_sloppy_imports::NoSloppyImportsRule::new(
        self.workspace_resolver.clone(),
      )),
    ))];
    let cli_graph_rules = vec![CliLintRule(CliLintRuleKind::Package(
      Box::new(no_slow_types::NoSlowTypesRule),
    ))];
    deno_lint_rules
      .into_iter()
      .filter(|rule| !FRESH_RULE_CODES.contains(&rule.code()))
      .map(|rule| CliLintRule(CliLintRuleKind::DenoLint(rule)))
      .chain(cli_lint_rules)
      .chain(cli_graph_rules)
      .collect()
  }

  pub fn resolve_lint_rules(
    &self,
    rules: LintRulesConfig,
    maybe_workspace_dir: Option<&WorkspaceDirectory>,
  ) -> ConfiguredRules {
    let rules = rename_rule_codes(rules);
    let all_rules = self.all_rules();
    let mut all_rule_names = HashSet::with_capacity(all_rules.len());
    let mut all_tags = HashSet::new();
    for rule in &all_rules {
      all_rule_names.insert(rule.code().into());
      all_tags.extend(rule.tags().iter().map(|t| t.display()));
    }
    warn_unknown_tags_and_rules(&rules, &all_tags, &all_rule_names);
    let rules = filtered_rules(
      all_rules.into_iter(),
      rules
        .tags
        .or_else(|| Some(get_default_tags(maybe_workspace_dir))),
      rules.exclude,
      rules.include,
    );
    ConfiguredRules {
      rules,
      all_rule_codes: all_rule_names,
    }
  }
}

/// Replaces renamed rule codes in `include`/`exclude` with their new names,
/// warning once per old code.
fn rename_rule_codes(mut rules: LintRulesConfig) -> LintRulesConfig {
  static WARNED: LazyLock<Mutex<HashSet<&'static str>>> =
    LazyLock::new(Default::default);

  for code in rules
    .include
    .iter_mut()
    .chain(rules.exclude.iter_mut())
    .flatten()
  {
    let Some((old, new)) =
      RENAMED_RULE_CODES.iter().find(|(old, _)| code == old)
    else {
      continue;
    };
    if WARNED.lock().insert(old) {
      log::warn!(
        "{}: Lint rule \"{old}\" is deprecated. Rename it to \"{new}\".",
        colors::yellow("warn"),
      );
    }
    *code = new.to_string();
  }
  rules
}

/// Warns once per tag or rule code in the config or flags that doesn't exist.
fn warn_unknown_tags_and_rules(
  rules: &LintRulesConfig,
  all_tags: &HashSet<&'static str>,
  all_rule_codes: &HashSet<Cow<'static, str>>,
) {
  static WARNED: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(Default::default);

  let unknown_tags = rules
    .tags
    .iter()
    .flatten()
    .filter(|tag| !all_tags.contains(tag.as_str()))
    .map(|tag| format!("Unknown lint tag \"{tag}\" is ignored."));
  let unknown_rules = rules
    .include
    .iter()
    .chain(rules.exclude.iter())
    .flatten()
    // plugin rules (`plugin-name/rule-name`) are only known once plugins load
    .filter(|code| {
      !code.contains('/') && !all_rule_codes.contains(code.as_str())
    })
    .map(|code| format!("Unknown lint rule \"{code}\" is ignored."));
  let mut warned = WARNED.lock();
  for message in unknown_tags.chain(unknown_rules) {
    if warned.insert(message.clone()) {
      log::warn!("{}: {}", colors::yellow("warn"), message);
    }
  }
}

fn get_default_tags(
  maybe_workspace_dir: Option<&WorkspaceDirectory>,
) -> Vec<String> {
  let mut tags = Vec::with_capacity(2);
  tags.push("recommended".to_string());
  if let Some(member_dir) = maybe_workspace_dir {
    if member_dir
      .member_or_root_ende_json()
      .map(|c| c.is_package())
      .unwrap_or(false)
    {
      tags.push("jsr".to_string());
    }
    if member_dir.member_or_root_ende_json().is_some() {
      tags.push("workspace".to_string());
    }
  }
  tags
}

fn filtered_rules(
  all_rules: impl Iterator<Item = CliLintRule>,
  maybe_tags: Option<Vec<String>>,
  maybe_exclude: Option<Vec<String>>,
  maybe_include: Option<Vec<String>>,
) -> Vec<CliLintRule> {
  let tags_set =
    maybe_tags.map(|tags| tags.into_iter().collect::<HashSet<_>>());

  let mut rules = all_rules
    .filter(|rule| {
      let mut passes = if let Some(tags_set) = &tags_set {
        rule
          .tags()
          .iter()
          .any(|t| tags_set.contains(&t.to_string()))
      } else {
        true
      };

      if let Some(includes) = &maybe_include
        && includes.contains(&rule.code().to_owned())
      {
        passes |= true;
      }

      if let Some(excludes) = &maybe_exclude
        && excludes.contains(&rule.code().to_owned())
      {
        passes &= false;
      }

      passes
    })
    .collect::<Vec<_>>();

  rules.sort_by_key(|r| r.code());

  rules
}

#[cfg(test)]
mod test {
  use super::*;
  use crate::args::LintRulesConfig;

  #[test]
  fn recommended_rules_when_no_tags_in_config() {
    let rules_config = LintRulesConfig {
      exclude: Some(vec!["no-debugger".to_string()]),
      include: None,
      tags: None,
    };
    let rules_provider = LintRuleProvider::new(None);
    let rules = rules_provider.resolve_lint_rules(rules_config, None);
    let mut rule_names = rules
      .rules
      .into_iter()
      .map(|r| r.code().to_string())
      .collect::<Vec<_>>();
    rule_names.sort();
    let mut recommended_rule_names = rules_provider
      .resolve_lint_rules(Default::default(), None)
      .rules
      .into_iter()
      .filter(|r| r.tags().contains(&&tags::RECOMMENDED))
      .map(|r| r.code().to_string())
      .filter(|n| n != "no-debugger")
      .collect::<Vec<_>>();
    recommended_rule_names.sort();
    assert_eq!(rule_names, recommended_rule_names);
  }

  #[test]
  fn fresh_rules_are_not_available() {
    let rules = LintRuleProvider::new(None).all_rules();
    assert!(!rules.iter().any(|r| r.code().starts_with("fresh-")));
    assert!(
      !rules
        .iter()
        .any(|r| r.tags().iter().any(|t| t.display() == "fresh"))
    );
  }

  #[test]
  fn renamed_rule_codes_still_apply() {
    let rules = LintRuleProvider::new(None).resolve_lint_rules(
      LintRulesConfig {
        exclude: Some(vec!["no-deprecated-deno-api".to_string()]),
        include: None,
        tags: None,
      },
      None,
    );
    assert!(
      !rules
        .rules
        .iter()
        .any(|r| r.code() == "no-deprecated-ende-api")
    );
  }

  #[test]
  fn fresh_tag_enables_no_rules() {
    let rules = LintRuleProvider::new(None).resolve_lint_rules(
      LintRulesConfig {
        exclude: None,
        include: Some(vec!["fresh-handler-export".to_string()]),
        tags: Some(vec!["fresh".to_string()]),
      },
      None,
    );
    assert!(rules.rules.is_empty());
  }
}
