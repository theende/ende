// Copyright 2018-2026 the Ende authors. MIT license.

//! The `isolate` key of the root ende.json: which of the app's own
//! dependencies run in a sandbox of their own, and what each sandbox is
//! granted.
//!
//! ```json
//! {
//!   "isolate": {
//!     "jsr:@std/csv": { "permissions": [] },
//!     "jsr:@acme/client": { "permissions": { "net": ["api.acme.com:443"] } },
//!     "https://cdn.example.com/lib/": { "permissions": { "read": ["./data"] } },
//!     "./vendor/legacy/": { "permissions": ["read"] }
//!   }
//! }
//! ```
//!
//! Each key is a matcher:
//!
//! - `jsr:@scope/name` matches every version and entry of the package.
//! - An `https:` URL ending in `/` matches every module under it; any other
//!   `https:` URL matches that one module.
//! - `./dir/`, `../dir/` or a `file:` URL ending in `/` matches every module
//!   in that folder.
//!
//! `permissions` is required (`[]` grants nothing) and takes the same forms
//! as an extension's. A sandbox never gets more than the app has.

use deno_error::JsError;
use indexmap::IndexMap;
use serde_json::Value;
use thiserror::Error;
use url::Url;

use super::ExtensionPermissions;
use super::ExtensionsConfig;
use super::ExtensionsConfigError;
use super::extensions::folder_holds_config;
use super::extensions::is_valid_extension_name;
use super::extensions::parse_permissions;

/// The parsed `isolate` key, in the order it was written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IsolateConfig {
  /// The rules, by matcher as written.
  pub rules: IndexMap<String, IsolateRule>,
}

/// One rule: which modules it sandboxes and what it grants them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsolateRule {
  pub matcher: IsolateMatcher,
  pub permissions: ExtensionPermissions,
  /// `permissions` as written in ende.json.
  pub permissions_json: Value,
}

/// Which modules a rule matches. Modules are matched by their resolved
/// specifier, after the import map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsolateMatcher {
  /// `jsr:@scope/name`: every version and entry of the package.
  JsrPackage(String),
  /// An `https:` URL ending in `/`: every module under it.
  UrlPrefix(Url),
  /// An `https:` URL: that one module.
  Url(Url),
  /// A `file:` URL ending in `/`: every module in that folder.
  Folder(Url),
}

impl IsolateMatcher {
  /// Whether `specifier` is matched as written: a `jsr:` specifier by its
  /// package name, others by URL. Registry URLs of jsr: packages and paths
  /// reached through symlinks are left to the caller, which knows the
  /// registry and the file system.
  pub fn matches_specifier(&self, specifier: &Url) -> bool {
    match self {
      Self::JsrPackage(name) => {
        specifier.scheme() == "jsr"
          && deno_semver::jsr::JsrPackageReqReference::from_specifier(specifier)
            .is_ok_and(|reference| reference.req().name.as_str() == name)
      }
      // Compared in a normal form, so another spelling of the same URL
      // (percent-encoded letters, doubled slashes, a query or fragment on
      // an exact URL) can't load it outside its sandbox.
      Self::UrlPrefix(prefix) => normalized_for_match(specifier)
        .as_str()
        .starts_with(normalized_for_match(prefix).as_str()),
      Self::Url(url) => {
        normalized_for_match(specifier) == normalized_for_match(url)
      }
      Self::Folder(folder) => {
        specifier.scheme() == "file"
          && specifier.as_str().starts_with(folder.as_str())
      }
    }
  }
}

/// `url` as rules compare it: without its query and fragment, with
/// percent-encoded unreserved characters (letters, digits, `-._~`) decoded,
/// other percent-encodings in upper case, and runs of `/` in its path
/// collapsed. A server may treat all these spellings as the same file.
pub fn normalized_for_match(url: &Url) -> Url {
  fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
  }
  let bytes = url.path().as_bytes();
  let mut path = String::with_capacity(bytes.len());
  let mut i = 0;
  while i < bytes.len() {
    let byte = bytes[i];
    if byte == b'%'
      && i + 2 < bytes.len()
      && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
    {
      let decoded = high * 16 + low;
      if decoded.is_ascii_alphanumeric() || b"-._~".contains(&decoded) {
        path.push(decoded as char);
      } else {
        path.push('%');
        path.push((bytes[i + 1] as char).to_ascii_uppercase());
        path.push((bytes[i + 2] as char).to_ascii_uppercase());
      }
      i += 3;
      continue;
    }
    if byte == b'/' && path.ends_with('/') {
      i += 1;
      continue;
    }
    path.push(byte as char);
    i += 1;
  }
  let mut normalized = url.clone();
  normalized.set_query(None);
  normalized.set_fragment(None);
  normalized.set_path(&path);
  normalized
}

#[derive(Debug, Error, JsError, Clone, PartialEq, Eq)]
#[class(type)]
pub enum IsolateConfigError {
  #[error(
    "\"isolate\" must be an object mapping module matchers to their configuration."
  )]
  NotAnObject,
  #[error(
    "Invalid matcher \"{matcher}\" in \"isolate\". Use jsr:@scope/name, an https: URL (ending in / for every module under it), or a folder starting with ./ or ../ and ending in /.{hint}"
  )]
  InvalidMatcher { matcher: String, hint: String },
  #[error(
    "isolate[\"{matcher}\"] names a version or an entry of a package. A jsr: matcher names the package and matches every version and entry of it, like \"jsr:{package}\"."
  )]
  JsrMatcherNotAPackage { matcher: String, package: String },
  #[error(
    "isolate[\"{matcher}\"]: npm: packages are not supported. Use a jsr: package, an https: URL or a local folder."
  )]
  NpmMatcher { matcher: String },
  #[error(
    "isolate[\"{matcher}\"] names a local folder and must end in /, like \"{suggestion}\"."
  )]
  FolderWithoutSlash { matcher: String, suggestion: String },
  #[error(
    "isolate[\"{matcher}\"] is the folder that holds ende.json, or above it. A sandbox can't hold the app itself; name the folder of the code to sandbox, like \"./vendor/name/\"."
  )]
  FolderHoldsProjectRoot { matcher: String },
  #[error("isolate[\"{matcher}\"] must be an object with \"permissions\".")]
  EntryNotAnObject { matcher: String },
  #[error(
    "Unknown field \"{field}\" in isolate[\"{matcher}\"]. Expected \"permissions\"."
  )]
  UnknownField { matcher: String, field: String },
  #[error(
    "isolate[\"{matcher}\"].permissions is required. Use [] to grant no permissions."
  )]
  MissingPermissions { matcher: String },
  #[error(
    "Unknown permission \"{permission}\" in isolate[\"{matcher}\"].permissions. Expected one of: all, read, write, import, env, net, run, ffi, sys."
  )]
  UnknownPermission { matcher: String, permission: String },
  #[error(
    "isolate[\"{matcher}\"].permissions must be an array of permission names or a permissions object: {message}"
  )]
  InvalidPermissions { matcher: String, message: String },
  #[error(
    "isolate[\"{matcher}\"] matches the source of extension \"{extension}\". An extension already runs in a sandbox of its own, with the grant in extensions[\"{extension}\"], so an \"isolate\" rule can't match it too."
  )]
  MatchesExtension { matcher: String, extension: String },
  #[error(
    "isolate[\"{matcher}\"] overlaps the folder of extension \"{extension}\". An extension's code and its dependencies run under the grant in extensions[\"{extension}\"], so an \"isolate\" rule can't claim them too."
  )]
  OverlapsExtensionFolder { matcher: String, extension: String },
}

impl IsolateConfig {
  /// Parses the value of the `isolate` key of the config file at `base`.
  pub fn parse(value: Value, base: &Url) -> Result<Self, IsolateConfigError> {
    let Value::Object(entries) = value else {
      return Err(IsolateConfigError::NotAnObject);
    };
    let mut rules = IndexMap::with_capacity(entries.len());
    for (matcher, entry) in entries {
      let rule = parse_rule(&matcher, entry, base)?;
      rules.insert(matcher, rule);
    }
    Ok(Self { rules })
  }

  /// Errors if a rule matches the source of one of `extensions`: an
  /// extension's code runs in its own sandbox under its own grant.
  pub fn check_extensions(
    &self,
    extensions: &ExtensionsConfig,
  ) -> Result<(), IsolateConfigError> {
    for (matcher, rule) in &self.rules {
      for (name, extension) in &extensions.extensions {
        if rule.matcher.matches_specifier(&extension.source) {
          return Err(IsolateConfigError::MatchesExtension {
            matcher: matcher.clone(),
            extension: name.clone(),
          });
        }
      }
    }
    Ok(())
  }
}

fn parse_rule(
  matcher: &str,
  entry: Value,
  base: &Url,
) -> Result<IsolateRule, IsolateConfigError> {
  let parsed = parse_matcher(matcher, base)?;
  let Value::Object(mut entry) = entry else {
    return Err(IsolateConfigError::EntryNotAnObject {
      matcher: matcher.to_string(),
    });
  };
  if let Some(field) = entry.keys().find(|field| *field != "permissions") {
    return Err(IsolateConfigError::UnknownField {
      matcher: matcher.to_string(),
      field: field.clone(),
    });
  }
  let permissions_json = entry.get("permissions").cloned().unwrap_or_default();
  // The same forms as an extension's grant; only the messages name the rule.
  let permissions =
    parse_permissions(matcher, entry.remove("permissions"), base).map_err(
      |error| match error {
        ExtensionsConfigError::UnknownPermission { permission, .. } => {
          IsolateConfigError::UnknownPermission {
            matcher: matcher.to_string(),
            permission,
          }
        }
        ExtensionsConfigError::InvalidPermissions { message, .. } => {
          IsolateConfigError::InvalidPermissions {
            matcher: matcher.to_string(),
            message,
          }
        }
        _ => IsolateConfigError::MissingPermissions {
          matcher: matcher.to_string(),
        },
      },
    )?;
  Ok(IsolateRule {
    matcher: parsed,
    permissions,
    permissions_json,
  })
}

fn parse_matcher(
  text: &str,
  base: &Url,
) -> Result<IsolateMatcher, IsolateConfigError> {
  let invalid = |hint: String| IsolateConfigError::InvalidMatcher {
    matcher: text.to_string(),
    hint,
  };
  if text.starts_with("./") || text.starts_with("../") {
    let url = base.join(text).map_err(|_| invalid(String::new()))?;
    return parse_folder(text, url, base);
  }
  if deno_path_util::specifier_has_uri_scheme(text) {
    let scheme = text.split_once(':').map(|(scheme, _)| scheme);
    return match scheme.map(|scheme| scheme.to_ascii_lowercase()).as_deref() {
      Some("https") => {
        let url = Url::parse(text).map_err(|_| invalid(String::new()))?;
        Ok(if url.path().ends_with('/') && url.query().is_none() {
          IsolateMatcher::UrlPrefix(url)
        } else {
          IsolateMatcher::Url(url)
        })
      }
      Some("file") => {
        let url = Url::parse(text).map_err(|_| invalid(String::new()))?;
        parse_folder(text, url, base)
      }
      Some("jsr") => parse_jsr_matcher(text),
      Some("npm") => Err(IsolateConfigError::NpmMatcher {
        matcher: text.to_string(),
      }),
      _ => Err(invalid(String::new())),
    };
  }
  let looks_like_a_path =
    !text.is_empty() && !text.starts_with('.') && !text.contains(['\\', ':']);
  if looks_like_a_path {
    let rest = text.trim_start_matches('/').trim_end_matches('/');
    return Err(invalid(format!(" Did you mean \"./{rest}/\"?")));
  }
  Err(invalid(String::new()))
}

fn parse_folder(
  text: &str,
  url: Url,
  base: &Url,
) -> Result<IsolateMatcher, IsolateConfigError> {
  if !url.path().ends_with('/') {
    return Err(IsolateConfigError::FolderWithoutSlash {
      matcher: text.to_string(),
      suggestion: format!("{text}/"),
    });
  }
  if folder_holds_config(&url, base) {
    return Err(IsolateConfigError::FolderHoldsProjectRoot {
      matcher: text.to_string(),
    });
  }
  Ok(IsolateMatcher::Folder(url))
}

fn parse_jsr_matcher(text: &str) -> Result<IsolateMatcher, IsolateConfigError> {
  let name = &text["jsr:".len()..];
  if is_valid_extension_name(name) {
    return Ok(IsolateMatcher::JsrPackage(name.to_string()));
  }
  // A package with a version or a path: say which matcher was meant.
  if let Ok(reference) =
    deno_semver::jsr::JsrPackageReqReference::from_str(text)
    && is_valid_extension_name(&reference.req().name)
  {
    return Err(IsolateConfigError::JsrMatcherNotAPackage {
      matcher: text.to_string(),
      package: reference.req().name.to_string(),
    });
  }
  Err(IsolateConfigError::InvalidMatcher {
    matcher: text.to_string(),
    hint: String::new(),
  })
}

#[cfg(test)]
mod test {
  use pretty_assertions::assert_eq;
  use serde_json::json;

  use super::*;
  use crate::ende_json::ConfigFile;

  fn config_url() -> Url {
    Url::parse("file:///project/ende.json").unwrap()
  }

  fn parse_config(json: Value) -> Result<IsolateConfig, IsolateConfigError> {
    ConfigFile::new(&json.to_string(), config_url())
      .unwrap()
      .to_isolate_config()
  }

  fn parse(isolate: Value) -> Result<IsolateConfig, IsolateConfigError> {
    parse_config(json!({ "isolate": isolate }))
  }

  fn matcher_of(matcher: &str) -> Result<IsolateMatcher, IsolateConfigError> {
    parse(json!({ matcher: { "permissions": [] } }))
      .map(|config| config.rules[matcher].matcher.clone())
  }

  fn matcher_err(matcher: &str) -> String {
    matcher_of(matcher).unwrap_err().to_string()
  }

  #[test]
  fn absent_is_empty() {
    let config = ConfigFile::new("{}", config_url()).unwrap();
    assert_eq!(config.to_isolate_config().unwrap(), Default::default());
  }

  #[test]
  fn the_four_matcher_forms() {
    let config = parse(json!({
      "jsr:@std/csv": { "permissions": [] },
      "https://cdn.example.com/lib/": { "permissions": ["read"] },
      "https://cdn.example.com/one.ts": {
        "permissions": { "net": ["api.example.com:443"] },
      },
      "./vendor/legacy/": { "permissions": [] },
    }))
    .unwrap();
    let matchers = config
      .rules
      .values()
      .map(|rule| rule.matcher.clone())
      .collect::<Vec<_>>();
    assert_eq!(
      matchers,
      vec![
        IsolateMatcher::JsrPackage("@std/csv".to_string()),
        IsolateMatcher::UrlPrefix(
          Url::parse("https://cdn.example.com/lib/").unwrap()
        ),
        IsolateMatcher::Url(
          Url::parse("https://cdn.example.com/one.ts").unwrap()
        ),
        IsolateMatcher::Folder(
          Url::parse("file:///project/vendor/legacy/").unwrap()
        ),
      ]
    );
    assert_eq!(
      config.rules["https://cdn.example.com/lib/"].permissions,
      ExtensionPermissions::Names(vec!["read".to_string()])
    );
    assert_eq!(
      config.rules["https://cdn.example.com/one.ts"].permissions_json,
      json!({ "net": ["api.example.com:443"] })
    );
    assert!(matches!(
      config.rules["https://cdn.example.com/one.ts"].permissions,
      ExtensionPermissions::Object(_)
    ));
  }

  #[test]
  fn other_local_forms() {
    assert_eq!(
      matcher_of("../shared/lib/").unwrap(),
      IsolateMatcher::Folder(Url::parse("file:///shared/lib/").unwrap())
    );
    assert_eq!(
      matcher_of("file:///opt/lib/").unwrap(),
      IsolateMatcher::Folder(Url::parse("file:///opt/lib/").unwrap())
    );
  }

  #[test]
  fn missing_permissions_is_an_error() {
    for entry in [json!({}), json!({ "permissions": null })] {
      assert_eq!(
        parse(json!({ "jsr:@std/csv": entry }))
          .unwrap_err()
          .to_string(),
        "isolate[\"jsr:@std/csv\"].permissions is required. Use [] to grant no permissions."
      );
    }
  }

  #[test]
  fn permissions_take_the_extension_forms() {
    assert_eq!(
      parse(json!({ "jsr:@std/csv": { "permissions": ["read", "fly"] } }))
        .unwrap_err(),
      IsolateConfigError::UnknownPermission {
        matcher: "jsr:@std/csv".to_string(),
        permission: "fly".to_string(),
      }
    );
    assert!(matches!(
      parse(json!({ "jsr:@std/csv": { "permissions": true } })),
      Err(IsolateConfigError::InvalidPermissions { .. })
    ));
  }

  #[test]
  fn bad_matchers() {
    for matcher in [
      "http://example.com/lib/",
      "data:text/javascript,export%20const%20x%20%3D%201",
      "jsr:",
      "jsr:csv",
      "jsr:@Std/csv",
      "",
      ".",
      ".\\vendor\\",
    ] {
      assert!(
        matches!(
          matcher_of(matcher),
          Err(IsolateConfigError::InvalidMatcher { .. })
        ),
        "{matcher}: {:?}",
        matcher_of(matcher)
      );
    }
    assert_eq!(
      matcher_err("vendor/legacy"),
      "Invalid matcher \"vendor/legacy\" in \"isolate\". Use jsr:@scope/name, an https: URL (ending in / for every module under it), or a folder starting with ./ or ../ and ending in /. Did you mean \"./vendor/legacy/\"?"
    );
  }

  #[test]
  fn jsr_matchers_name_a_package() {
    for matcher in ["jsr:@std/csv@1", "jsr:@std/csv/parse", "jsr:@std/csv@^1/x"]
    {
      assert_eq!(
        matcher_err(matcher),
        format!(
          "isolate[\"{matcher}\"] names a version or an entry of a package. A jsr: matcher names the package and matches every version and entry of it, like \"jsr:@std/csv\"."
        ),
        "{matcher}"
      );
    }
  }

  #[test]
  fn npm_is_refused() {
    assert_eq!(
      matcher_err("npm:chalk@5"),
      "isolate[\"npm:chalk@5\"]: npm: packages are not supported. Use a jsr: package, an https: URL or a local folder."
    );
  }

  #[test]
  fn folders_end_in_a_slash() {
    assert_eq!(
      matcher_err("./vendor/legacy"),
      "isolate[\"./vendor/legacy\"] names a local folder and must end in /, like \"./vendor/legacy/\"."
    );
    assert!(matches!(
      matcher_of("file:///opt/lib"),
      Err(IsolateConfigError::FolderWithoutSlash { .. })
    ));
  }

  #[test]
  fn folder_holding_the_project_root() {
    for matcher in ["./", "../", "./vendor/../", "file:///project/", "file:///"]
    {
      assert_eq!(
        matcher_err(matcher),
        format!(
          "isolate[\"{matcher}\"] is the folder that holds ende.json, or above it. A sandbox can't hold the app itself; name the folder of the code to sandbox, like \"./vendor/name/\"."
        ),
        "{matcher}"
      );
    }
  }

  #[test]
  fn entry_shape() {
    assert_eq!(
      parse(json!([])).unwrap_err(),
      IsolateConfigError::NotAnObject
    );
    assert_eq!(
      parse(json!({ "jsr:@std/csv": [] })).unwrap_err(),
      IsolateConfigError::EntryNotAnObject {
        matcher: "jsr:@std/csv".to_string()
      }
    );
    assert_eq!(
      parse(json!({
        "jsr:@std/csv": { "permissions": [], "permision": ["read"] },
      }))
      .unwrap_err(),
      IsolateConfigError::UnknownField {
        matcher: "jsr:@std/csv".to_string(),
        field: "permision".to_string(),
      }
    );
  }

  #[test]
  fn matches_specifier() {
    let jsr = IsolateMatcher::JsrPackage("@std/csv".to_string());
    for specifier in
      ["jsr:@std/csv", "jsr:@std/csv@1.0.0", "jsr:@std/csv@1/parse"]
    {
      assert!(jsr.matches_specifier(&Url::parse(specifier).unwrap()));
    }
    assert!(!jsr.matches_specifier(&Url::parse("jsr:@std/csvx").unwrap()));

    let prefix = matcher_of("https://cdn.example.com/lib/").unwrap();
    let exact = matcher_of("https://cdn.example.com/lib/mod.ts").unwrap();
    let inside = Url::parse("https://cdn.example.com/lib/mod.ts").unwrap();
    let nested = Url::parse("https://cdn.example.com/lib/sub/x.ts").unwrap();
    let outside = Url::parse("https://cdn.example.com/library.ts").unwrap();
    assert!(prefix.matches_specifier(&inside));
    assert!(prefix.matches_specifier(&nested));
    assert!(!prefix.matches_specifier(&outside));
    assert!(exact.matches_specifier(&inside));
    assert!(!exact.matches_specifier(&nested));

    let folder = matcher_of("./vendor/").unwrap();
    assert!(folder.matches_specifier(
      &Url::parse("file:///project/vendor/a/mod.ts").unwrap()
    ));
    assert!(!folder.matches_specifier(
      &Url::parse("file:///project/vendored/mod.ts").unwrap()
    ));
  }

  #[test]
  fn an_extension_source_cant_be_isolated_too() {
    for (matcher, source) in [
      ("jsr:@acme/fmt", "jsr:@acme/fmt@1"),
      (
        "https://cdn.example.com/ext/",
        "https://cdn.example.com/ext/mod.ts",
      ),
      ("./ext/", "./ext/fmt/mod.ts"),
    ] {
      let error = parse_config(json!({
        "extensions": {
          "@acme/fmt": { "source": source, "permissions": [] },
        },
        "isolate": { matcher: { "permissions": [] } },
      }))
      .unwrap_err();
      assert_eq!(
        error.to_string(),
        format!(
          "isolate[\"{matcher}\"] matches the source of extension \"@acme/fmt\". An extension already runs in a sandbox of its own, with the grant in extensions[\"@acme/fmt\"], so an \"isolate\" rule can't match it too."
        ),
        "{matcher}"
      );
    }
    // A rule beside the extension is fine.
    parse_config(json!({
      "extensions": {
        "@acme/fmt": { "source": "./ext/fmt/mod.ts", "permissions": [] },
      },
      "isolate": { "./vendor/": { "permissions": [] } },
    }))
    .unwrap();
  }

  #[test]
  fn schema_matches_the_parser() {
    let schema: Value = serde_json::from_str(include_str!(concat!(
      env!("CARGO_MANIFEST_DIR"),
      "/../../cli/schemas/config-file.v1.json"
    )))
    .unwrap();
    let property = &schema["properties"]["isolate"];
    assert_eq!(
      property["additionalProperties"]["$ref"],
      json!("#/$defs/isolateRule")
    );
    // The prefixes `parse_matcher` accepts.
    assert_eq!(
      property["propertyNames"]["pattern"],
      json!("^(https:|file:|jsr:@|\\./|\\.\\./)")
    );
    let rule = &schema["$defs"]["isolateRule"];
    assert_eq!(rule["required"], json!(["permissions"]));
    assert_eq!(rule["additionalProperties"], json!(false));
    // The same grant forms as an extension's.
    assert_eq!(
      rule["properties"]["permissions"]["oneOf"],
      schema["$defs"]["extension"]["properties"]["permissions"]["oneOf"]
    );
  }
}
