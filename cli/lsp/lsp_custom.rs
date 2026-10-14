// Copyright 2018-2026 the Deno authors. MIT license.

use ende_core::serde::Deserialize;
use ende_core::serde::Serialize;
use ende_core::serde_json;
use ende_core::serde_json::Value;
use tower_lsp::lsp_types as lsp;

// Custom requests are accepted under their `ende/*` names and, from legacy
// clients, under their `deno/*` names.
pub const PERFORMANCE_REQUEST: &str = "ende/performance";
pub const LEGACY_PERFORMANCE_REQUEST: &str = "deno/performance";
pub const INFERRED_TYPE_REQUEST: &str = "ende/inferredType";
pub const LEGACY_INFERRED_TYPE_REQUEST: &str = "deno/inferredType";
/// Sent as `ende.inferredType`, or `deno.inferredType` to legacy clients.
pub const INFERRED_TYPE_COMMAND: &str = "inferredType";
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
pub struct EndeConfigurationData {
  pub scope_uri: lsp::Uri,
  pub workspace_root_scope_uri: Option<lsp::Uri>,
  #[serde(rename = "endeJson", alias = "denoJson")]
  pub ende_json: Option<lsp::TextDocumentIdentifier>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DidRefreshEndeConfigurationTreeNotificationParams {
  pub data: Vec<EndeConfigurationData>,
}

impl DidRefreshEndeConfigurationTreeNotificationParams {
  /// The params with the `deno*` field names that legacy clients expect.
  pub fn into_legacy(self) -> Value {
    let mut value = serde_json::to_value(self).unwrap();
    if let Some(data) = value.get_mut("data").and_then(|d| d.as_array_mut()) {
      for data in data {
        rename_field(data, "endeJson", "denoJson");
      }
    }
    value
  }
}

fn rename_field(value: &mut Value, from: &str, to: &str) {
  if let Some(object) = value.as_object_mut()
    && let Some(field) = object.remove(from)
  {
    object.insert(to.to_string(), field);
  }
}

/// Sent as `ende/didRefreshEndeConfigurationTree`, or under its old
/// `deno/didRefreshDenoConfigurationTree` name and with
/// [`DidRefreshEndeConfigurationTreeNotificationParams::into_legacy`] params
/// to legacy clients (`LEGACY`).
pub enum DidRefreshEndeConfigurationTreeNotification<const LEGACY: bool> {}

impl lsp::notification::Notification
  for DidRefreshEndeConfigurationTreeNotification<false>
{
  type Params = DidRefreshEndeConfigurationTreeNotificationParams;
  const METHOD: &'static str = "ende/didRefreshEndeConfigurationTree";
}

impl lsp::notification::Notification
  for DidRefreshEndeConfigurationTreeNotification<true>
{
  type Params = Value;
  const METHOD: &'static str = "deno/didRefreshDenoConfigurationTree";
}

#[derive(Debug, Eq, Hash, PartialEq, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EndeConfigurationChangeType {
  Added,
  Changed,
  Removed,
}

impl EndeConfigurationChangeType {
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
pub enum EndeConfigurationType {
  #[serde(rename = "endeJson", alias = "denoJson")]
  EndeJson,
}

#[derive(Debug, Eq, Hash, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndeConfigurationChangeEvent {
  pub scope_uri: lsp::Uri,
  pub file_uri: lsp::Uri,
  #[serde(rename = "type")]
  pub typ: EndeConfigurationChangeType,
  pub configuration_type: EndeConfigurationType,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeEndeConfigurationNotificationParams {
  pub changes: Vec<EndeConfigurationChangeEvent>,
}

impl DidChangeEndeConfigurationNotificationParams {
  /// The params with the `deno*` values that legacy clients expect.
  pub fn into_legacy(self) -> Value {
    let mut value = serde_json::to_value(self).unwrap();
    if let Some(changes) =
      value.get_mut("changes").and_then(|c| c.as_array_mut())
    {
      for change in changes {
        if change["configurationType"] == "endeJson" {
          change["configurationType"] = Value::from("denoJson");
        }
      }
    }
    value
  }
}

// TODO(nayeemrmn): This is being replaced by
// `DidRefreshEndeConfigurationTreeNotification` for Deno > v2.0.0. Remove it
// soon.
/// Sent as `ende/didChangeEndeConfiguration`, or under its old
/// `deno/didChangeDenoConfiguration` name and with
/// [`DidChangeEndeConfigurationNotificationParams::into_legacy`] params to
/// legacy clients (`LEGACY`).
pub enum DidChangeEndeConfigurationNotification<const LEGACY: bool> {}

impl lsp::notification::Notification
  for DidChangeEndeConfigurationNotification<false>
{
  type Params = DidChangeEndeConfigurationNotificationParams;
  const METHOD: &'static str = "ende/didChangeEndeConfiguration";
}

impl lsp::notification::Notification
  for DidChangeEndeConfigurationNotification<true>
{
  type Params = Value;
  const METHOD: &'static str = "deno/didChangeDenoConfiguration";
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
