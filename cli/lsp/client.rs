// Copyright 2018-2026 the Deno authors. MIT license.

use std::sync::Arc;

use async_trait::async_trait;
use ende_core::anyhow::anyhow;
use ende_core::anyhow::bail;
use ende_core::error::AnyError;
use ende_core::serde_json::json;
use ende_core::unsync::spawn;
use lsp_types::Uri;
use tower_lsp::lsp_types as lsp;
use tower_lsp::lsp_types::ConfigurationItem;

use super::config::DEPRECATED_SETTINGS_SECTION;
use super::config::SETTINGS_SECTION;
use super::config::WorkspaceSettings;
use super::legacy_client;
use super::lsp_custom;
use super::testing::lsp_custom as testing_lsp_custom;
use crate::lsp::logging::lsp_warn;

#[derive(Debug)]
pub enum TestingNotification {
  Module(testing_lsp_custom::TestModuleNotificationParams),
  DeleteModule(testing_lsp_custom::TestModuleDeleteNotificationParams),
  Progress(testing_lsp_custom::TestRunProgressParams),
}

#[derive(Clone)]
pub struct Client(Arc<dyn ClientTrait>);

impl std::fmt::Debug for Client {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_tuple("Client").finish()
  }
}

impl Client {
  pub fn from_tower(client: tower_lsp::Client) -> Self {
    Self(Arc::new(TowerClient(client)))
  }

  /// Gets additional methods that should only be called outside
  /// the LSP's lock to prevent deadlocking scenarios.
  pub fn when_outside_lsp_lock(&self) -> OutsideLockClient {
    OutsideLockClient(self.0.clone())
  }

  pub async fn publish_diagnostics(
    &self,
    uri: Uri,
    diags: Vec<lsp::Diagnostic>,
    version: Option<i32>,
  ) {
    self.0.publish_diagnostics(uri, diags, version).await;
  }

  pub fn send_registry_state_notification(
    &self,
    params: lsp_custom::RegistryStateNotificationParams,
  ) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    spawn(async move {
      client.send_registry_state_notification(params).await;
    });
  }

  pub fn send_diagnostic_batch_start_notification(&self) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    spawn(async move {
      client.send_diagnostic_batch_start_notification().await;
    });
  }

  pub fn send_diagnostic_batch_end_notification(&self) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    spawn(async move {
      client.send_diagnostic_batch_end_notification().await;
    });
  }

  pub fn send_test_notification(&self, params: TestingNotification) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    spawn(async move {
      client.send_test_notification(params).await;
    });
  }

  pub fn send_did_refresh_ende_configuration_tree_notification(
    &self,
    params: lsp_custom::DidRefreshEndeConfigurationTreeNotificationParams,
  ) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    spawn(async move {
      client
        .send_did_refresh_ende_configuration_tree_notification(params)
        .await;
    });
  }

  pub fn send_did_change_ende_configuration_notification(
    &self,
    params: lsp_custom::DidChangeEndeConfigurationNotificationParams,
  ) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    spawn(async move {
      client
        .send_did_change_ende_configuration_notification(params)
        .await;
    });
  }

  pub fn send_did_upgrade_check_notification(
    &self,
    params: lsp_custom::DidUpgradeCheckNotificationParams,
  ) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    spawn(async move {
      client.send_did_upgrade_check_notification(params).await;
    });
  }

  pub fn log_message(
    &self,
    message_type: lsp::MessageType,
    message: impl std::fmt::Display,
  ) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    let message = message.to_string();
    spawn(async move {
      client.log_message(message_type, message).await;
    });
  }

  pub fn show_message(
    &self,
    message_type: lsp::MessageType,
    message: impl std::fmt::Display,
  ) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    let message = message.to_string();
    spawn(async move {
      client.show_message(message_type, message).await;
    });
  }

  pub fn refresh_diagnostics(&self) {
    // do on a task in case the caller currently is in the lsp lock
    let client = self.0.clone();
    spawn(async move {
      if let Err(err) = client.refresh_diagnostics().await {
        lsp_warn!("Client failed to refresh diagnostics: ${err:#}");
      }
    });
  }
}

/// DANGER: The methods on this client should only be called outside
/// the LSP's lock. The reason is you never want to call into the client
/// while holding the lock because the client might call back into the
/// server and cause a deadlock.
pub struct OutsideLockClient(Arc<dyn ClientTrait>);

impl OutsideLockClient {
  pub async fn register_capability(
    &self,
    registrations: Vec<lsp::Registration>,
  ) -> Result<(), AnyError> {
    self.0.register_capability(registrations).await
  }

  pub async fn workspace_configuration(
    &self,
    scopes: Vec<Option<lsp::Uri>>,
  ) -> Result<Vec<WorkspaceSettings>, AnyError> {
    self.0.workspace_configuration(scopes).await
  }
}

#[async_trait]
trait ClientTrait: Send + Sync {
  async fn publish_diagnostics(
    &self,
    uri: lsp::Uri,
    diagnostics: Vec<lsp::Diagnostic>,
    version: Option<i32>,
  );
  async fn send_registry_state_notification(
    &self,
    params: lsp_custom::RegistryStateNotificationParams,
  );
  async fn send_diagnostic_batch_start_notification(&self);
  async fn send_diagnostic_batch_end_notification(&self);
  async fn send_test_notification(&self, params: TestingNotification);
  async fn send_did_refresh_ende_configuration_tree_notification(
    &self,
    params: lsp_custom::DidRefreshEndeConfigurationTreeNotificationParams,
  );
  async fn send_did_change_ende_configuration_notification(
    &self,
    params: lsp_custom::DidChangeEndeConfigurationNotificationParams,
  );
  async fn send_did_upgrade_check_notification(
    &self,
    params: lsp_custom::DidUpgradeCheckNotificationParams,
  );
  async fn refresh_diagnostics(&self) -> Result<(), AnyError>;
  async fn workspace_configuration(
    &self,
    scopes: Vec<Option<lsp::Uri>>,
  ) -> Result<Vec<WorkspaceSettings>, AnyError>;
  async fn log_message(&self, message_type: lsp::MessageType, text: String);
  async fn show_message(&self, message_type: lsp::MessageType, text: String);
  async fn register_capability(
    &self,
    registrations: Vec<lsp::Registration>,
  ) -> Result<(), AnyError>;
}

#[derive(Clone)]
struct TowerClient(tower_lsp::Client);

/// Sends a custom notification under its `deno/*` name to legacy clients and
/// under its `ende/*` name otherwise.
macro_rules! send_custom_notification {
  ($client:expr, $($notification:ident)::+, $params:expr) => {
    if legacy_client::is_legacy() {
      $client
        .send_notification::<$($notification)::+<true>>($params)
        .await
    } else {
      $client
        .send_notification::<$($notification)::+<false>>($params)
        .await
    }
  };
}

#[async_trait]
impl ClientTrait for TowerClient {
  async fn publish_diagnostics(
    &self,
    uri: lsp::Uri,
    diagnostics: Vec<lsp::Diagnostic>,
    version: Option<i32>,
  ) {
    self.0.publish_diagnostics(uri, diagnostics, version).await
  }

  async fn send_registry_state_notification(
    &self,
    params: lsp_custom::RegistryStateNotificationParams,
  ) {
    send_custom_notification!(
      self.0,
      lsp_custom::RegistryStateNotification,
      params
    )
  }

  async fn send_diagnostic_batch_start_notification(&self) {
    send_custom_notification!(
      self.0,
      lsp_custom::DiagnosticBatchStartNotification,
      ()
    )
  }

  async fn send_diagnostic_batch_end_notification(&self) {
    send_custom_notification!(
      self.0,
      lsp_custom::DiagnosticBatchEndNotification,
      ()
    )
  }

  async fn send_test_notification(&self, notification: TestingNotification) {
    match notification {
      TestingNotification::Module(params) => {
        send_custom_notification!(
          self.0,
          testing_lsp_custom::TestModuleNotification,
          params
        )
      }
      TestingNotification::DeleteModule(params) => send_custom_notification!(
        self.0,
        testing_lsp_custom::TestModuleDeleteNotification,
        params
      ),
      TestingNotification::Progress(params) => {
        send_custom_notification!(
          self.0,
          testing_lsp_custom::TestRunProgressNotification,
          params
        )
      }
    }
  }

  async fn send_did_refresh_ende_configuration_tree_notification(
    &self,
    params: lsp_custom::DidRefreshEndeConfigurationTreeNotificationParams,
  ) {
    if legacy_client::is_legacy() {
      self
        .0
        .send_notification::<
          lsp_custom::DidRefreshEndeConfigurationTreeNotification<true>,
        >(params.into_legacy())
        .await
    } else {
      self
        .0
        .send_notification::<
          lsp_custom::DidRefreshEndeConfigurationTreeNotification<false>,
        >(params)
        .await
    }
  }

  async fn send_did_change_ende_configuration_notification(
    &self,
    params: lsp_custom::DidChangeEndeConfigurationNotificationParams,
  ) {
    if legacy_client::is_legacy() {
      self
        .0
        .send_notification::<
          lsp_custom::DidChangeEndeConfigurationNotification<true>,
        >(params.into_legacy())
        .await
    } else {
      self
        .0
        .send_notification::<
          lsp_custom::DidChangeEndeConfigurationNotification<false>,
        >(params)
        .await
    }
  }

  async fn send_did_upgrade_check_notification(
    &self,
    params: lsp_custom::DidUpgradeCheckNotificationParams,
  ) {
    send_custom_notification!(
      self.0,
      lsp_custom::DidUpgradeCheckNotification,
      params
    )
  }

  async fn refresh_diagnostics(&self) -> Result<(), AnyError> {
    self
      .0
      .send_request::<lsp::request::WorkspaceDiagnosticRefresh>(())
      .await
      .map_err(|err| anyhow!("{err:#}"))
  }

  async fn workspace_configuration(
    &self,
    scopes: Vec<Option<lsp::Uri>>,
  ) -> Result<Vec<WorkspaceSettings>, AnyError> {
    let config_response = self
      .0
      .configuration(
        scopes
          .iter()
          .flat_map(|scope_uri| {
            vec![
              ConfigurationItem {
                scope_uri: scope_uri.clone(),
                section: Some(SETTINGS_SECTION.to_string()),
              },
              ConfigurationItem {
                scope_uri: scope_uri.clone(),
                section: Some(DEPRECATED_SETTINGS_SECTION.to_string()),
              },
              ConfigurationItem {
                scope_uri: scope_uri.clone(),
                section: Some("javascript".to_string()),
              },
              ConfigurationItem {
                scope_uri: scope_uri.clone(),
                section: Some("typescript".to_string()),
              },
            ]
          })
          .collect(),
      )
      .await;
    match config_response {
      Ok(configs) => {
        let mut configs = configs.into_iter();
        let mut result = Vec::with_capacity(scopes.len());
        for _ in 0..scopes.len() {
          let ende = json!(configs.next());
          let deno = json!(configs.next());
          let javascript = json!(configs.next());
          let typescript = json!(configs.next());
          let (settings, warning) = legacy_client::select_settings(ende, deno);
          if let Some(warning) = warning {
            lsp_warn!("{warning}");
            self
              .log_message(lsp::MessageType::WARNING, warning.to_string())
              .await;
          }
          result.push(WorkspaceSettings::from_raw_settings(
            settings, javascript, typescript,
          ));
        }
        Ok(result)
      }
      Err(err) => {
        bail!("Error getting workspace configurations: {}", err)
      }
    }
  }

  async fn log_message(&self, message_type: lsp::MessageType, message: String) {
    self.0.log_message(message_type, message).await
  }

  async fn show_message(
    &self,
    message_type: lsp::MessageType,
    message: String,
  ) {
    self.0.show_message(message_type, message).await
  }

  async fn register_capability(
    &self,
    registrations: Vec<lsp::Registration>,
  ) -> Result<(), AnyError> {
    self
      .0
      .register_capability(registrations)
      .await
      .map_err(|err| anyhow!("{}", err))
  }
}
