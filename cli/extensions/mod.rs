// Copyright 2018-2026 the Ende authors. MIT license.

//! Sandboxed extensions (`extension:@scope/name`): what each one may do.
//!
//! An extension's effective permissions are what the app grants it in
//! ende.json intersected with what its manifest asks for, so it never gets
//! more than either side allows. `registry` loads them.

mod manifest;
mod registry;
mod types;

use ende_config::ende_json::ExtensionPermissions;
use ende_core::error::AnyError;
use ende_runtime::ende_permissions::PermissionDescriptorParser;
use ende_runtime::ende_permissions::PermissionsOptions;
use ende_runtime::ende_permissions::UngrantedPermission;
pub use manifest::ExtensionManifest;
pub use registry::CliExtensionWorkerProvider;
pub use registry::EXTENSION_SCHEME;
pub use registry::ExtensionRegistry;
pub use registry::check_extension_directory;
pub use registry::check_extension_graph_resolution;
pub use registry::extension_directory;
pub use registry::extension_loader_permissions;
pub use types::ExtensionTypes;
pub use types::extension_only_modules;
pub use types::runtime_specifier;
pub use types::stub_for_source;

use crate::args::extension_grant_permissions_options;

/// The permissions an extension runs with.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionPermissionsOptions {
  /// Grant ∩ manifest, with the grant's denials. Never prompts.
  pub options: PermissionsOptions,
  /// Printed once when the extension loads, if its manifest asks for
  /// permissions the grant doesn't cover.
  pub warning: Option<String>,
}

/// The effective permissions of the extension `name`, granted `grant` in
/// ende.json and asking for what `manifest` says.
pub fn extension_permissions_options(
  name: &str,
  grant: &ExtensionPermissions,
  manifest: &ExtensionManifest,
  parser: &dyn PermissionDescriptorParser,
) -> Result<ExtensionPermissionsOptions, AnyError> {
  let granted = extension_grant_permissions_options(grant)?;
  let (options, ungranted) =
    granted.intersect_requested(&manifest.permissions, parser)?;
  Ok(ExtensionPermissionsOptions {
    options,
    warning: ungranted_warning(name, &ungranted),
  })
}

/// One warning naming every permission the manifest of `name` asks for that
/// ende.json doesn't grant, and where to grant it.
pub fn ungranted_warning(
  name: &str,
  ungranted: &[UngrantedPermission],
) -> Option<String> {
  if ungranted.is_empty() {
    return None;
  }
  let mut warning = format!(
    "Extension \"{name}\" asks for permissions that ende.json doesn't grant, so using them will be denied:"
  );
  for permission in ungranted {
    match &permission.entry {
      Some(entry) => warning.push_str(&format!(
        "\n  {} \"{entry}\": add it to extensions[\"{name}\"].permissions.{}",
        permission.name, permission.name
      )),
      None => warning.push_str(&format!(
        "\n  {}: add \"{}\" to extensions[\"{name}\"].permissions",
        permission.name, permission.name
      )),
    }
  }
  Some(warning)
}

#[cfg(test)]
mod test {
  use std::path::Path;

  use ende_config::ende_json::PermissionsObjectWithBase;
  use ende_core::serde_json;
  use ende_runtime::ende_permissions::Permissions;
  use ende_runtime::ende_permissions::RuntimePermissionDescriptorParser;
  use pretty_assertions::assert_eq;

  use super::*;

  fn parser() -> RuntimePermissionDescriptorParser<crate::sys::CliSys> {
    RuntimePermissionDescriptorParser::new(crate::sys::CliSys::default())
  }

  fn root() -> &'static str {
    if cfg!(windows) { "C:\\" } else { "/" }
  }

  fn manifest(source: &str) -> ExtensionManifest {
    manifest::extract_manifest(
      "@scope/name",
      &deno_ast::ModuleSpecifier::parse("file:///ext/mod.ts").unwrap(),
      deno_ast::MediaType::TypeScript,
      source,
    )
    .unwrap()
  }

  fn object_grant(json: serde_json::Value) -> ExtensionPermissions {
    let config_dir = Path::new(root()).join("app");
    ExtensionPermissions::Object(Box::new(PermissionsObjectWithBase {
      base: deno_path_util::url_from_file_path(&config_dir.join("ende.json"))
        .unwrap(),
      permissions: serde_json::from_value(json).unwrap(),
    }))
  }

  #[test]
  fn grant_intersected_with_manifest() {
    let data = Path::new(root()).join("app").join("data");
    let manifest = manifest(
      "export const manifest = { permissions: { read: true, net: [\"api.example.com\", \"other.example.com\"], env: [\"HOME\"] } };",
    );
    let grant = object_grant(serde_json::json!({
      "read": ["./data"],
      "net": ["api.example.com"],
      "sys": true,
    }));
    let result = extension_permissions_options(
      "@scope/name",
      &grant,
      &manifest,
      &parser(),
    )
    .unwrap();
    assert_eq!(
      result.options,
      PermissionsOptions {
        allow_read: Some(vec![data.to_string_lossy().into_owned()]),
        allow_net: Some(vec!["api.example.com".to_string()]),
        ..Default::default()
      }
    );
    assert_eq!(
      result.warning.as_deref(),
      Some(
        "Extension \"@scope/name\" asks for permissions that ende.json doesn't grant, so using them will be denied:\n  net \"other.example.com\": add it to extensions[\"@scope/name\"].permissions.net\n  env \"HOME\": add it to extensions[\"@scope/name\"].permissions.env"
      )
    );
    Permissions::from_options(&parser(), &result.options).unwrap();
  }

  #[test]
  fn names_grant_and_whole_permissions() {
    let manifest = manifest(
      "export const manifest = { permissions: [\"net\", \"write\", \"run\"] };",
    );
    let grant = ExtensionPermissions::Names(vec!["net".to_string()]);
    let result = extension_permissions_options(
      "@scope/name",
      &grant,
      &manifest,
      &parser(),
    )
    .unwrap();
    assert_eq!(
      result.options,
      PermissionsOptions {
        allow_net: Some(vec![]),
        ..Default::default()
      }
    );
    assert_eq!(
      result.warning.as_deref(),
      Some(
        "Extension \"@scope/name\" asks for permissions that ende.json doesn't grant, so using them will be denied:\n  write: add \"write\" to extensions[\"@scope/name\"].permissions\n  run: add \"run\" to extensions[\"@scope/name\"].permissions"
      )
    );
  }

  #[test]
  fn all_grant_gives_only_what_is_asked() {
    let manifest = manifest(
      "export const manifest = { permissions: { net: [\"a.example.com\"] } };",
    );
    for grant in [
      ExtensionPermissions::Names(vec!["all".to_string()]),
      object_grant(serde_json::json!({ "all": true })),
    ] {
      let result = extension_permissions_options(
        "@scope/name",
        &grant,
        &manifest,
        &parser(),
      )
      .unwrap();
      assert_eq!(
        result.options,
        PermissionsOptions {
          allow_net: Some(vec!["a.example.com".to_string()]),
          ..Default::default()
        }
      );
      assert_eq!(result.warning, None);
    }
  }

  #[test]
  fn empty_grant_and_pure_extension() {
    let pure = manifest("export const manifest = { permissions: [] };");
    let result = extension_permissions_options(
      "@scope/pure",
      &ExtensionPermissions::Names(vec![]),
      &pure,
      &parser(),
    )
    .unwrap();
    assert_eq!(result.options, PermissionsOptions::default());
    assert_eq!(result.warning, None);
  }

  #[test]
  fn grant_denials_apply() {
    let manifest =
      manifest("export const manifest = { permissions: { read: true } };");
    let grant = object_grant(serde_json::json!({
      "read": { "allow": ["./data"], "deny": ["./data/secret"] },
    }));
    let result = extension_permissions_options(
      "@scope/name",
      &grant,
      &manifest,
      &parser(),
    )
    .unwrap();
    let data = Path::new(root()).join("app").join("data");
    assert_eq!(
      result.options.deny_read,
      Some(vec![data.join("secret").to_string_lossy().into_owned()])
    );
    assert!(!result.options.prompt);
  }

  #[test]
  fn no_warning_when_everything_is_granted() {
    assert_eq!(ungranted_warning("@scope/name", &[]), None);
  }
}
