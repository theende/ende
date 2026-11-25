// Copyright 2018-2026 the Ende authors. MIT license.

//! The `extensions` key of the root ende.json: which sandboxed extensions an
//! app may import as `extension:@scope/name`, where each one's code comes
//! from, and what it is granted.
//!
//! ```json
//! {
//!   "extensions": {
//!     "@scope/name": {
//!       "source": "https://host/1.2.3/mod.ts",
//!       "permissions": { "read": ["./data"], "net": ["api.example.com:443"] }
//!     },
//!     "@scope/pure": { "source": "jsr:@scope/pure@1", "permissions": [] },
//!     "@local/dev": { "source": "./ext/mod.ts", "permissions": [] }
//!   }
//! }
//! ```
//!
//! `source` is an https: or file: URL, a jsr: package, or a path relative to
//! the config file starting with `./` or `../`.
//!
//! `permissions` is required: `[]` grants nothing, and a missing field is an
//! error, so no extension runs with permissions nobody wrote down.

use deno_error::JsError;
use indexmap::IndexMap;
use serde_json::Value;
use thiserror::Error;
use url::Url;

use super::PermissionsObject;
use super::PermissionsObjectWithBase;

/// The permission names an extension can be granted, as in a permissions
/// object.
pub const EXTENSION_PERMISSION_NAMES: [&str; 9] = [
  "all", "read", "write", "import", "env", "net", "run", "ffi", "sys",
];

/// The parsed `extensions` key, in the order it was written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionsConfig {
  pub extensions: IndexMap<String, ExtensionConfig>,
}

/// One extension: where its code comes from and what the app grants it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionConfig {
  /// An absolute `https:` or `file:` URL of the extension's entry module.
  pub source: Url,
  pub permissions: ExtensionPermissions,
  /// `source` as written in ende.json. The lockfile pins this form, which
  /// doesn't depend on where the project is checked out.
  pub source_specifier: String,
  /// `permissions` as written in ende.json, pinned by the lockfile.
  pub permissions_json: Value,
}

/// What the app grants an extension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionPermissions {
  /// Scoped grants, like a permission set. Relative paths resolve against
  /// the config file (`base`).
  Object(Box<PermissionsObjectWithBase>),
  /// Bare names, each granting that whole kind (`["read", "net"]`). Empty
  /// grants nothing.
  Names(Vec<String>),
}

#[derive(Debug, Error, JsError, Clone, PartialEq, Eq)]
#[class(type)]
pub enum ExtensionsConfigError {
  #[error(
    "\"extensions\" must be an object mapping extension names to their configuration."
  )]
  NotAnObject,
  #[error(
    "Invalid extension name \"{name}\" in \"extensions\". Use the format '@scope/name'."
  )]
  InvalidName { name: String },
  #[error(
    "extensions[\"{name}\"] must be an object with \"source\" and \"permissions\"."
  )]
  EntryNotAnObject { name: String },
  #[error(
    "Unknown field \"{field}\" in extensions[\"{name}\"]. Expected \"source\" and \"permissions\"."
  )]
  UnknownField { name: String, field: String },
  #[error("extensions[\"{name}\"].source is required.")]
  MissingSource { name: String },
  #[error(
    "extensions[\"{name}\"].source must be a jsr:, https: or file: specifier, or a path starting with ./ or ../, got {specifier}."
  )]
  InvalidSource { name: String, specifier: String },
  #[error(
    "extensions[\"{name}\"].source \"{specifier}\" is not a relative path or URL. Did you mean \"{suggestion}\"?{hint}"
  )]
  BareSource {
    name: String,
    specifier: String,
    suggestion: String,
    hint: &'static str,
  },
  #[error(
    "extensions[\"{name}\"].source is not a valid jsr: specifier \"{specifier}\": {message}"
  )]
  InvalidJsrSource {
    name: String,
    specifier: String,
    message: String,
  },
  #[error(
    "extensions[\"{name}\"].source: npm: packages are not supported for extensions. Use a jsr:, https: or file: specifier, or a path starting with ./ or ../."
  )]
  NpmSource { name: String },
  #[error(
    "extensions[\"{name}\"].source \"{specifier}\" is in the folder that holds ende.json, or above it. A local extension can read its whole folder, and the app can't write to it, so give it a folder of its own, like \"./ext/{package}/mod.ts\"."
  )]
  SourceHoldsProjectRoot {
    name: String,
    specifier: String,
    package: String,
  },
  #[error(
    "extensions[\"{name}\"].permissions is required. Use [] to grant no permissions."
  )]
  MissingPermissions { name: String },
  #[error(
    "Unknown permission \"{permission}\" in extensions[\"{name}\"].permissions. Expected one of: all, read, write, import, env, net, run, ffi, sys."
  )]
  UnknownPermission { name: String, permission: String },
  #[error(
    "extensions[\"{name}\"].permissions must be an array of permission names or a permissions object: {message}"
  )]
  InvalidPermissions { name: String, message: String },
}

impl ExtensionsConfig {
  /// Parses the value of the `extensions` key of the config file at `base`.
  pub fn parse(
    value: Value,
    base: &Url,
  ) -> Result<Self, ExtensionsConfigError> {
    let Value::Object(entries) = value else {
      return Err(ExtensionsConfigError::NotAnObject);
    };
    let mut extensions = IndexMap::with_capacity(entries.len());
    for (name, entry) in entries {
      let extension = parse_extension(&name, entry, base)?;
      extensions.insert(name, extension);
    }
    Ok(Self { extensions })
  }
}

fn parse_extension(
  name: &str,
  entry: Value,
  base: &Url,
) -> Result<ExtensionConfig, ExtensionsConfigError> {
  if !is_valid_extension_name(name) {
    return Err(ExtensionsConfigError::InvalidName {
      name: name.to_string(),
    });
  }
  let Value::Object(mut entry) = entry else {
    return Err(ExtensionsConfigError::EntryNotAnObject {
      name: name.to_string(),
    });
  };
  if let Some(field) = entry
    .keys()
    .find(|field| *field != "source" && *field != "permissions")
  {
    return Err(ExtensionsConfigError::UnknownField {
      name: name.to_string(),
      field: field.clone(),
    });
  }
  let written = match entry.get("source") {
    Some(Value::String(text)) => Some(text.clone()),
    _ => None,
  };
  let source = parse_source(name, entry.remove("source"), base)?;
  if folder_holds_config(&source, base) {
    return Err(ExtensionsConfigError::SourceHoldsProjectRoot {
      name: name.to_string(),
      specifier: written.unwrap_or_else(|| source.to_string()),
      package: name.split_once('/').map(|(_, p)| p).unwrap_or(name).into(),
    });
  }
  let permissions_json = entry.get("permissions").cloned().unwrap_or_default();
  let permissions = parse_permissions(name, entry.remove("permissions"), base)?;
  Ok(ExtensionConfig {
    source_specifier: written.unwrap_or_else(|| source.to_string()),
    source,
    permissions,
    permissions_json,
  })
}

/// Whether a local extension's folder (its source's folder) is the folder
/// of the config file at `base`, or above it. The extension may read its
/// whole folder and the app may not write to it, so such a source would hand
/// the extension the project and lock the app out of it. Compares the URLs
/// as written; the CLI repeats the check on canonical paths, which also
/// catches a folder reached through a symlink.
fn folder_holds_config(source: &Url, base: &Url) -> bool {
  if source.scheme() != "file" || base.scheme() != "file" {
    return false;
  }
  let (Ok(source_folder), Ok(config_folder)) =
    (source.join("./"), base.join("./"))
  else {
    return false;
  };
  config_folder.as_str().starts_with(source_folder.as_str())
}

/// Extension names are `@scope/name`, each part lowercase ASCII letters,
/// digits and hyphens, not starting with a hyphen, as on JSR. Lowercase only,
/// so two declarations can't differ by case alone.
fn is_valid_extension_name(name: &str) -> bool {
  fn is_valid_part(part: &str) -> bool {
    !part.is_empty()
      && !part.starts_with('-')
      && part
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
  }
  match name.strip_prefix('@').and_then(|rest| rest.split_once('/')) {
    Some((scope, package)) => is_valid_part(scope) && is_valid_part(package),
    None => false,
  }
}

/// Parses an extension's `source`: an https: or file: URL, a jsr: package
/// reference, or a path relative to the config file. A path must start with
/// `./` or `../` (as `exports` values must), so it can't be mistaken for a
/// package name. A jsr: source stays a `jsr:` URL, the form module graph
/// specifiers take; resolving it is left to whoever loads the extension.
fn parse_source(
  name: &str,
  source: Option<Value>,
  base: &Url,
) -> Result<Url, ExtensionsConfigError> {
  let invalid = |specifier: String| ExtensionsConfigError::InvalidSource {
    name: name.to_string(),
    specifier,
  };
  let text = match source {
    None | Some(Value::Null) => {
      return Err(ExtensionsConfigError::MissingSource {
        name: name.to_string(),
      });
    }
    Some(Value::String(text)) => text,
    Some(other) => return Err(invalid(other.to_string())),
  };
  if text.starts_with("./") || text.starts_with("../") {
    return base.join(&text).map_err(|_| invalid(format!("\"{text}\"")));
  }
  if deno_path_util::specifier_has_uri_scheme(&text) {
    let scheme = text.split_once(':').map(|(scheme, _)| scheme);
    return match scheme.map(|scheme| scheme.to_ascii_lowercase()).as_deref() {
      Some("https" | "file") => match Url::parse(&text) {
        Ok(url) => Ok(url),
        Err(_) => Err(invalid(format!("\"{text}\""))),
      },
      Some("jsr") => parse_jsr_source(name, &text),
      Some("npm") => Err(ExtensionsConfigError::NpmSource {
        name: name.to_string(),
      }),
      _ => Err(invalid(format!("\"{text}\""))),
    };
  }
  let looks_like_a_path =
    !text.is_empty() && !text.starts_with('.') && !text.contains(['\\', ':']);
  if looks_like_a_path {
    let (suggestion, hint) = match text.strip_prefix('/') {
      Some(rest) => (
        format!("./{rest}"),
        " For an absolute path, use a file: URL.",
      ),
      None => (format!("./{text}"), ""),
    };
    return Err(ExtensionsConfigError::BareSource {
      name: name.to_string(),
      specifier: text,
      suggestion,
      hint,
    });
  }
  Err(invalid(format!("\"{text}\"")))
}

fn parse_jsr_source(
  name: &str,
  text: &str,
) -> Result<Url, ExtensionsConfigError> {
  let invalid = |message: String| ExtensionsConfigError::InvalidJsrSource {
    name: name.to_string(),
    specifier: text.to_string(),
    message,
  };
  let reference = deno_semver::jsr::JsrPackageReqReference::from_str(text)
    .map_err(|err| invalid(err.to_string()))?;
  if !is_valid_jsr_package_name(&reference.req().name) {
    return Err(invalid(
      "JSR package names are scoped, like @scope/name".to_string(),
    ));
  }
  Url::parse(text).map_err(|err| invalid(err.to_string()))
}

fn is_valid_jsr_package_name(name: &str) -> bool {
  matches!(
    name.strip_prefix('@').and_then(|rest| rest.split_once('/')),
    Some((scope, package)) if !scope.is_empty() && !package.is_empty()
  )
}

fn parse_permissions(
  name: &str,
  permissions: Option<Value>,
  base: &Url,
) -> Result<ExtensionPermissions, ExtensionsConfigError> {
  let unknown = |permission: &str| ExtensionsConfigError::UnknownPermission {
    name: name.to_string(),
    permission: permission.to_string(),
  };
  let invalid = |message: String| ExtensionsConfigError::InvalidPermissions {
    name: name.to_string(),
    message,
  };
  match permissions {
    None | Some(Value::Null) => {
      Err(ExtensionsConfigError::MissingPermissions {
        name: name.to_string(),
      })
    }
    Some(Value::Array(items)) => items
      .into_iter()
      .map(|item| match item {
        Value::String(permission)
          if EXTENSION_PERMISSION_NAMES.contains(&permission.as_str()) =>
        {
          Ok(permission)
        }
        Value::String(permission) => Err(unknown(&permission)),
        other => {
          Err(invalid(format!("expected a permission name, got {other}")))
        }
      })
      .collect::<Result<Vec<_>, _>>()
      .map(ExtensionPermissions::Names),
    Some(Value::Object(object)) => {
      if let Some(permission) = object
        .keys()
        .find(|key| !EXTENSION_PERMISSION_NAMES.contains(&key.as_str()))
      {
        return Err(unknown(permission));
      }
      let permissions: PermissionsObject =
        serde_json::from_value(Value::Object(object))
          .map_err(|error| invalid(error.to_string()))?;
      Ok(ExtensionPermissions::Object(Box::new(
        PermissionsObjectWithBase {
          base: base.clone(),
          permissions,
        },
      )))
    }
    Some(other) => Err(invalid(format!(
      "expected an array or an object, got {other}"
    ))),
  }
}

#[cfg(test)]
mod test {
  use pretty_assertions::assert_eq;
  use serde_json::json;

  use super::*;
  use crate::ende_json::AllowDenyIgnorePermissionConfig;
  use crate::ende_json::AllowDenyPermissionConfig;
  use crate::ende_json::ConfigFile;
  use crate::ende_json::PermissionConfigValue;

  fn config_url() -> Url {
    Url::parse("file:///project/ende.json").unwrap()
  }

  fn parse(
    extensions: Value,
  ) -> Result<ExtensionsConfig, ExtensionsConfigError> {
    let text = json!({ "extensions": extensions }).to_string();
    ConfigFile::new(&text, config_url())
      .unwrap()
      .to_extensions_config()
  }

  fn parse_err(extensions: Value) -> String {
    parse(extensions).unwrap_err().to_string()
  }

  #[test]
  fn absent_is_empty() {
    let config = ConfigFile::new("{}", config_url()).unwrap();
    assert_eq!(config.to_extensions_config().unwrap(), Default::default());
  }

  #[test]
  fn object_form() {
    let config = parse(json!({
      "@scope/name": {
        "source": "https://example.com/1.2.3/mod.ts",
        "permissions": {
          "read": ["./data"],
          "net": ["api.example.com:443"],
        },
      },
    }))
    .unwrap();
    assert_eq!(
      config.extensions["@scope/name"],
      ExtensionConfig {
        source: Url::parse("https://example.com/1.2.3/mod.ts").unwrap(),
        permissions: ExtensionPermissions::Object(Box::new(
          PermissionsObjectWithBase {
            base: config_url(),
            permissions: PermissionsObject {
              read: AllowDenyIgnorePermissionConfig {
                allow: Some(PermissionConfigValue::Some(vec![
                  "./data".to_string()
                ])),
                ..Default::default()
              },
              net: AllowDenyPermissionConfig {
                allow: Some(PermissionConfigValue::Some(vec![
                  "api.example.com:443".to_string()
                ])),
                ..Default::default()
              },
              ..Default::default()
            },
          }
        )),
        source_specifier: "https://example.com/1.2.3/mod.ts".to_string(),
        permissions_json: json!({
          "read": ["./data"],
          "net": ["api.example.com:443"],
        }),
      }
    );
  }

  #[test]
  fn names_form() {
    let config = parse(json!({
      "@scope/name": {
        "source": "file:///project/ext/mod.ts",
        "permissions": ["read", "net"],
      },
    }))
    .unwrap();
    assert_eq!(
      config.extensions["@scope/name"].permissions,
      ExtensionPermissions::Names(vec!["read".to_string(), "net".to_string()])
    );
  }

  #[test]
  fn empty_names_grant_nothing() {
    let config = parse(json!({
      "@scope/pure": {
        "source": "https://example.com/mod.ts",
        "permissions": [],
      },
    }))
    .unwrap();
    assert_eq!(
      config.extensions["@scope/pure"].permissions,
      ExtensionPermissions::Names(vec![])
    );
  }

  #[test]
  fn keeps_declaration_order() {
    let config = parse(json!({
      "@b/second": { "source": "https://example.com/b.ts", "permissions": [] },
      "@a/first": { "source": "https://example.com/a.ts", "permissions": [] },
    }))
    .unwrap();
    assert_eq!(
      config.extensions.keys().collect::<Vec<_>>(),
      vec!["@b/second", "@a/first"]
    );
  }

  #[test]
  fn missing_permissions_is_an_error() {
    let message = parse_err(json!({
      "@scope/name": { "source": "https://example.com/mod.ts" },
    }));
    assert_eq!(
      message,
      "extensions[\"@scope/name\"].permissions is required. Use [] to grant no permissions."
    );
  }

  #[test]
  fn null_permissions_is_missing() {
    assert_eq!(
      parse(json!({
        "@scope/name": {
          "source": "https://example.com/mod.ts",
          "permissions": null,
        },
      }))
      .unwrap_err(),
      ExtensionsConfigError::MissingPermissions {
        name: "@scope/name".to_string()
      }
    );
  }

  #[test]
  fn bad_names() {
    for name in [
      "name",
      "scope/name",
      "@scope",
      "@Scope/name",
      "@scope/name/x",
      "@scope/",
      "@-scope/name",
      "@scope/na me",
    ] {
      assert_eq!(
        parse(json!({
          name: { "source": "https://example.com/mod.ts", "permissions": [] },
        }))
        .unwrap_err(),
        ExtensionsConfigError::InvalidName {
          name: name.to_string()
        },
        "{name}"
      );
    }
  }

  fn source_of(source: &str) -> Result<Url, ExtensionsConfigError> {
    parse(json!({
      "@scope/name": { "source": source, "permissions": [] },
    }))
    .map(|config| config.extensions["@scope/name"].source.clone())
  }

  fn source_err(source: &str) -> String {
    source_of(source).unwrap_err().to_string()
  }

  #[test]
  fn relative_sources_resolve_against_the_config_file() {
    for (source, url) in [
      ("./ext/mod.ts", "file:///project/ext/mod.ts"),
      ("../shared/ext.ts", "file:///shared/ext.ts"),
      ("./ext/../ext/mod.ts", "file:///project/ext/mod.ts"),
    ] {
      assert_eq!(source_of(source).unwrap().as_str(), url, "{source}");
    }
  }

  #[test]
  fn local_sources_need_a_folder_below_the_project_root() {
    for source in [
      "./mod.ts",
      "../mod.ts",
      "./ext/../mod.ts",
      "file:///project/mod.ts",
      "file:///mod.ts",
    ] {
      assert_eq!(
        source_err(source),
        format!(
          "extensions[\"@scope/name\"].source \"{source}\" is in the folder that holds ende.json, or above it. A local extension can read its whole folder, and the app can't write to it, so give it a folder of its own, like \"./ext/name/mod.ts\"."
        ),
        "{source}"
      );
    }
    for source in [
      "./ext/fmt/mod.ts",
      "./ext/mod.ts",
      "../shared/ext.ts",
      "file:///project/ext/mod.ts",
      "https://host/mod.ts",
    ] {
      source_of(source).unwrap_or_else(|err| panic!("{source}: {err}"));
    }
  }

  #[test]
  fn jsr_sources() {
    for source in [
      "jsr:@scope/pkg",
      "jsr:@scope/pkg@1.2.3",
      "jsr:@scope/pkg@^1/sub/path",
    ] {
      let url = source_of(source).unwrap();
      assert_eq!(url.scheme(), "jsr", "{source}");
      // Stage 3 resolves it as a JSR package reference.
      deno_semver::jsr::JsrPackageReqReference::from_specifier(&url)
        .unwrap_or_else(|err| panic!("{source}: {err}"));
    }
  }

  #[test]
  fn invalid_jsr_sources() {
    for source in [
      "jsr:",
      "jsr:pkg",
      "jsr:@scope",
      "jsr:@scope/pkg@not a version",
    ] {
      let err = source_err(source);
      assert!(
        err.starts_with(&format!(
          "extensions[\"@scope/name\"].source is not a valid jsr: specifier \"{source}\""
        )),
        "{source}: {err}"
      );
    }
  }

  #[test]
  fn npm_sources_are_not_supported() {
    assert_eq!(
      source_err("npm:some-package@1"),
      "extensions[\"@scope/name\"].source: npm: packages are not supported for extensions. Use a jsr:, https: or file: specifier, or a path starting with ./ or ../."
    );
  }

  #[test]
  fn bare_paths_need_a_dot_slash() {
    assert_eq!(
      source_err("ext/mod.ts"),
      "extensions[\"@scope/name\"].source \"ext/mod.ts\" is not a relative path or URL. Did you mean \"./ext/mod.ts\"?"
    );
    assert_eq!(
      source_err("/abs/ext.ts"),
      "extensions[\"@scope/name\"].source \"/abs/ext.ts\" is not a relative path or URL. Did you mean \"./abs/ext.ts\"? For an absolute path, use a file: URL."
    );
  }

  #[test]
  fn other_sources_are_rejected() {
    for source in [
      "http://example.com/mod.ts",
      "data:text/javascript,export%20const%20x%20%3D%201",
      ".\\ext\\mod.ts",
      "C:\\ext\\mod.ts",
      ".",
      "..",
      "",
    ] {
      let err = source_err(source);
      assert!(
        err.contains("must be a jsr:, https: or file: specifier, or a path starting with ./ or ../"),
        "{source}: {err}"
      );
    }
  }

  #[test]
  fn missing_and_non_string_source() {
    assert_eq!(
      parse(json!({ "@scope/name": { "permissions": [] } })).unwrap_err(),
      ExtensionsConfigError::MissingSource {
        name: "@scope/name".to_string()
      }
    );
    assert!(matches!(
      parse(json!({ "@scope/name": { "source": 1, "permissions": [] } })),
      Err(ExtensionsConfigError::InvalidSource { .. })
    ));
  }

  #[test]
  fn unknown_permission_kind() {
    for permissions in [json!(["read", "fly"]), json!({ "fly": true })] {
      assert_eq!(
        parse(json!({
          "@scope/name": {
            "source": "https://example.com/mod.ts",
            "permissions": permissions,
          },
        }))
        .unwrap_err(),
        ExtensionsConfigError::UnknownPermission {
          name: "@scope/name".to_string(),
          permission: "fly".to_string(),
        },
        "{permissions}"
      );
    }
  }

  #[test]
  fn permissions_of_the_wrong_shape() {
    for permissions in
      [json!(true), json!("read"), json!([1]), json!({ "read": 1 })]
    {
      assert!(
        matches!(
          parse(json!({
            "@scope/name": {
              "source": "https://example.com/mod.ts",
              "permissions": permissions,
            },
          })),
          Err(ExtensionsConfigError::InvalidPermissions { .. })
        ),
        "{permissions}"
      );
    }
  }

  #[test]
  fn unknown_field_in_an_entry() {
    assert_eq!(
      parse(json!({
        "@scope/name": {
          "source": "https://example.com/mod.ts",
          "permissions": [],
          "permision": ["read"],
        },
      }))
      .unwrap_err(),
      ExtensionsConfigError::UnknownField {
        name: "@scope/name".to_string(),
        field: "permision".to_string(),
      }
    );
  }

  #[test]
  fn not_objects() {
    assert_eq!(
      parse(json!([])).unwrap_err(),
      ExtensionsConfigError::NotAnObject
    );
    assert_eq!(
      parse(json!({ "@scope/name": "https://example.com/mod.ts" }))
        .unwrap_err(),
      ExtensionsConfigError::EntryNotAnObject {
        name: "@scope/name".to_string()
      }
    );
  }

  #[test]
  fn schema_matches_the_parser() {
    let schema: Value = serde_json::from_str(include_str!(concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/../../cli/schemas/config-file.v1.json"
    )))
    .unwrap();
    let property = &schema["properties"]["extensions"];
    assert_eq!(
      property["additionalProperties"]["$ref"],
      json!("#/$defs/extension")
    );
    // The same rule as `is_valid_extension_name`.
    assert_eq!(
      property["propertyNames"]["pattern"],
      json!("^@[a-z0-9][a-z0-9-]*/[a-z0-9][a-z0-9-]*$")
    );
    let extension = &schema["$defs"]["extension"];
    assert_eq!(extension["required"], json!(["source", "permissions"]));
    // The prefixes `parse_source` accepts.
    assert_eq!(
      extension["properties"]["source"]["pattern"],
      json!("^(https:|file:|jsr:|\\./|\\.\\./)")
    );
    assert_eq!(extension["additionalProperties"], json!(false));
    let permissions = &extension["properties"]["permissions"]["oneOf"];
    assert_eq!(
      permissions[0]["items"]["enum"],
      json!(EXTENSION_PERMISSION_NAMES)
    );
    assert_eq!(permissions[1]["$ref"], json!("#/$defs/permissionSet"));
  }

  #[test]
  fn duplicate_names_keep_the_last() {
    // The JSONC parser lets a repeated key through and keeps its last value,
    // as for every other key in ende.json.
    let text = r#"{
      "extensions": {
        "@scope/name": { "source": "https://example.com/a.ts", "permissions": ["net"] },
        "@scope/name": { "source": "https://example.com/b.ts", "permissions": [] }
      }
    }"#;
    let config = ConfigFile::new(text, config_url())
      .unwrap()
      .to_extensions_config()
      .unwrap();
    assert_eq!(config.extensions.len(), 1);
    assert_eq!(
      config.extensions["@scope/name"],
      ExtensionConfig {
        source: Url::parse("https://example.com/b.ts").unwrap(),
        permissions: ExtensionPermissions::Names(vec![]),
        source_specifier: "https://example.com/b.ts".to_string(),
        permissions_json: json!([]),
      }
    );
  }
}
