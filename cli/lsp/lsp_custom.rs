// Copyright 2018-2026 the Deno authors. MIT license.

use ende_core::serde::Deserialize;
use ende_core::serde::Serialize;
use tower_lsp::lsp_types as lsp;

// Custom requests are accepted under their `ende/*` names and, from legacy
// clients, under their `deno/*` names.
pub const PERFORMANCE_REQUEST: &str = "ende/performance";
pub const LEGACY_PERFORMANCE_REQUEST: &str = "deno/performance";
pub const INFERRED_TYPE_REQUEST: &str = "ende/inferredType";
pub const LEGACY_INFERRED_TYPE_REQUEST: &str = "deno/inferredType";
pub const INFERRED_TYPE_COMMAND: &str = "deno.inferredType";
pub const TASK_REQUEST: &str = "ende/taskDefinitions";
pub const LEGACY_TASK_REQUEST: &str = "deno/taskDefinitions";
pub const VIRTUAL_TEXT_DOCUMENT: &str = "ende/virtualTextDocument";
pub const LEGACY_VIRTUAL_TEXT_DOCUMENT: &str = "deno/virtualTextDocument";

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InferredTypeParams {
  pub text_document: lsp::TextDocumentIdentifier,
  pub position: lsp::Position,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InferredTypeResponse {
  pub text: String,
  pub range: lsp::Range,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDefinition {
  pub name: String,
  pub command: Option<String>,
  pub description: Option<String>,
  pub source_uri: lsp::Uri,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RegistryStateNotificationParams {
  pub origin: String,
  pub suggestions: bool,
}

/// Sent with the `deno/` prefix to legacy clients (`LEGACY`), else `ende/`.
pub enum RegistryStateNotification<const LEGACY: bool> {}

impl<const LEGACY: bool> lsp::notification::Notification
  for RegistryStateNotification<LEGACY>
{
  type Params = RegistryStateNotificationParams;
  const METHOD: &'static str = if LEGACY {
    "deno/registryState"
  } else {
    "ende/registryState"
  };
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualTextDocumentParams {
  pub text_document: lsp::TextDocumentIdentifier,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DenoConfigurationData {
  pub scope_uri: lsp::Uri,
  pub workspace_root_scope_uri: Option<lsp::Uri>,
  pub deno_json: Option<lsp::TextDocumentIdentifier>,
  pub package_json: Option<lsp::TextDocumentIdentifier>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DidRefreshDenoConfigurationTreeNotificationParams {
  pub data: Vec<DenoConfigurationData>,
  pub deno_dir_npm_folder_uri: Option<lsp::Uri>,
}

/// Sent with the `deno/` prefix to legacy clients (`LEGACY`), else `ende/`.
pub enum DidRefreshDenoConfigurationTreeNotification<const LEGACY: bool> {}

impl<const LEGACY: bool> lsp::notification::Notification
  for DidRefreshDenoConfigurationTreeNotification<LEGACY>
{
  type Params = DidRefreshDenoConfigurationTreeNotificationParams;
  const METHOD: &'static str = if LEGACY {
    "deno/didRefreshDenoConfigurationTree"
  } else {
    "ende/didRefreshDenoConfigurationTree"
  };
}

#[derive(Debug, Eq, Hash, PartialEq, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DenoConfigurationChangeType {
  Added,
  Changed,
  Removed,
}

impl DenoConfigurationChangeType {
  pub fn from_file_change_type(file_event: lsp::FileChangeType) -> Self {
    match file_event {
      lsp::FileChangeType::CREATED => Self::Added,
      lsp::FileChangeType::CHANGED => Self::Changed,
      lsp::FileChangeType::DELETED => Self::Removed,
      _ => Self::Changed, // non-exhaustable enum
    }
  }
}

#[derive(Debug, Eq, Hash, PartialEq, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DenoConfigurationType {
  DenoJson,
  PackageJson,
}

#[derive(Debug, Eq, Hash, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DenoConfigurationChangeEvent {
  pub scope_uri: lsp::Uri,
  pub file_uri: lsp::Uri,
  #[serde(rename = "type")]
  pub typ: DenoConfigurationChangeType,
  pub configuration_type: DenoConfigurationType,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeDenoConfigurationNotificationParams {
  pub changes: Vec<DenoConfigurationChangeEvent>,
}

// TODO(nayeemrmn): This is being replaced by
// `DidRefreshDenoConfigurationTreeNotification` for Deno > v2.0.0. Remove it
// soon.
/// Sent with the `deno/` prefix to legacy clients (`LEGACY`), else `ende/`.
pub enum DidChangeDenoConfigurationNotification<const LEGACY: bool> {}

impl<const LEGACY: bool> lsp::notification::Notification
  for DidChangeDenoConfigurationNotification<LEGACY>
{
  type Params = DidChangeDenoConfigurationNotificationParams;
  const METHOD: &'static str = if LEGACY {
    "deno/didChangeDenoConfiguration"
  } else {
    "ende/didChangeDenoConfiguration"
  };
}

/// Sent with the `deno/` prefix to legacy clients (`LEGACY`), else `ende/`.
pub enum DidUpgradeCheckNotification<const LEGACY: bool> {}

impl<const LEGACY: bool> lsp::notification::Notification
  for DidUpgradeCheckNotification<LEGACY>
{
  type Params = DidUpgradeCheckNotificationParams;
  const METHOD: &'static str = if LEGACY {
    "deno/didUpgradeCheck"
  } else {
    "ende/didUpgradeCheck"
  };
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeAvailable {
  pub latest_version: String,
  pub is_canary: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DidUpgradeCheckNotificationParams {
  pub upgrade_available: Option<UpgradeAvailable>,
}

/// This notification is only sent for testing purposes
/// in order to group diagnostics.
/// Sent with the `deno/` prefix to legacy clients (`LEGACY`), else `ende/`.
pub enum DiagnosticBatchStartNotification<const LEGACY: bool> {}

impl<const LEGACY: bool> lsp::notification::Notification
  for DiagnosticBatchStartNotification<LEGACY>
{
  type Params = ();
  const METHOD: &'static str = if LEGACY {
    "deno/internalTestDiagnosticBatchStart"
  } else {
    "ende/internalTestDiagnosticBatchStart"
  };
}

/// This notification is only sent for testing purposes
/// in order to group diagnostics.
/// Sent with the `deno/` prefix to legacy clients (`LEGACY`), else `ende/`.
pub enum DiagnosticBatchEndNotification<const LEGACY: bool> {}

impl<const LEGACY: bool> lsp::notification::Notification
  for DiagnosticBatchEndNotification<LEGACY>
{
  type Params = ();
  const METHOD: &'static str = if LEGACY {
    "deno/internalTestDiagnosticBatchEnd"
  } else {
    "ende/internalTestDiagnosticBatchEnd"
  };
}
