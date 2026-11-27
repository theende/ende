// Copyright 2018-2026 the Ende authors. MIT license.

//! Reads an extension's entry module without running it: the permissions it
//! asks for (`export const manifest = { permissions: … }`) and the functions
//! it exports.
//!
//! ```ts
//! export const manifest = {
//!   permissions: { read: true, net: ["api.example.com"] },
//! };
//! export async function coolThing(x: string): Promise<number> { … }
//! ```
//!
//! The manifest is read from the source text, so it may hold only literals:
//! objects, arrays, strings and booleans.

use deno_ast::MediaType;
use deno_ast::ModuleSpecifier;
use deno_ast::ParsedSource;
use deno_ast::SourceRange;
use deno_ast::SourceRangedForSpanned as _;
use deno_ast::swc::ast;
use ende_config::ende_json::EXTENSION_PERMISSION_NAMES;
use ende_core::serde_json;
use ende_core::serde_json::Value;
use ende_runtime::ende_permissions::AllowRunDescriptor;
use ende_runtime::ende_permissions::PermissionsOptions;

use crate::args::permission_names_to_options;

/// What an extension's entry module declares about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtensionManifest {
  /// The permissions it asks for. Paths are absolute. Never prompts.
  pub permissions: PermissionsOptions,
  /// Its exported functions, in source order.
  pub exports: Vec<String>,
  /// `manifest.permissions` as declared, which the lockfile pins.
  pub permissions_json: Value,
}

#[derive(Debug, thiserror::Error, deno_error::JsError)]
#[class(type)]
#[error("Invalid {subject} ({location}): {message}")]
pub struct ManifestError {
  /// What the manifest belongs to, such as `extension "@scope/name"`.
  pub subject: String,
  /// The entry module, with a line and column when the problem has one.
  pub location: String,
  pub message: String,
}

/// Reads the manifest and exported functions of the extension `name`, whose
/// entry module `specifier` has the text `source`.
pub fn extract_manifest(
  name: &str,
  specifier: &ModuleSpecifier,
  media_type: MediaType,
  source: &str,
) -> Result<ExtensionManifest, ManifestError> {
  let parsed = deno_ast::parse_module(deno_ast::ParseParams {
    specifier: specifier.clone(),
    text: source.into(),
    media_type,
    capture_tokens: false,
    scope_analysis: false,
    maybe_syntax: None,
  })
  .map_err(|diagnostic| {
    let position = diagnostic.display_position();
    ManifestError {
      subject: format!("extension \"{name}\""),
      location: format!(
        "{specifier}:{}:{}",
        position.line_number, position.column_number
      ),
      message: diagnostic.kind().msg().to_string(),
    }
  })?;
  Reader {
    subject: format!("extension \"{name}\""),
    what: "extension",
    specifier,
    parsed: &parsed,
  }
  .read()
}

/// What a sandboxed package's entry module asks for (see `crate::isolate`).
#[derive(Debug, Clone, PartialEq)]
pub struct PackageManifest {
  /// The permissions it asks for. Paths are absolute. Never prompts.
  pub permissions: PermissionsOptions,
  /// `manifest.permissions` as declared, which the lockfile pins.
  pub permissions_json: Value,
}

/// Reads the manifest of `specifier`, an entry module of the sandbox of
/// `isolate[rule]` whose text is `source`, if it declares one: an
/// `export const manifest = { permissions: … }` holding an object literal
/// with a `permissions` key, read as an extension's manifest is. Any other
/// export named `manifest` isn't one, so a package that exports something
/// else by that name still works. The module's other exports aren't
/// checked here.
pub fn extract_package_manifest(
  rule: &str,
  specifier: &ModuleSpecifier,
  media_type: MediaType,
  source: &str,
) -> Result<Option<PackageManifest>, ManifestError> {
  let subject = format!("manifest of a package in isolate[\"{rule}\"]");
  let parsed = deno_ast::parse_module(deno_ast::ParseParams {
    specifier: specifier.clone(),
    text: source.into(),
    media_type,
    capture_tokens: false,
    scope_analysis: false,
    maybe_syntax: None,
  })
  .map_err(|diagnostic| {
    let position = diagnostic.display_position();
    ManifestError {
      subject: subject.clone(),
      location: format!(
        "{specifier}:{}:{}",
        position.line_number, position.column_number
      ),
      message: diagnostic.kind().msg().to_string(),
    }
  })?;
  let reader = Reader {
    subject,
    what: "package",
    specifier,
    parsed: &parsed,
  };
  let deno_ast::ProgramRef::Module(module) = parsed.program_ref() else {
    return Ok(None);
  };
  for item in &module.body {
    let ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(export)) = item
    else {
      continue;
    };
    let ast::Decl::Var(decl) = &export.decl else {
      continue;
    };
    if decl.declare || decl.kind != ast::VarDeclKind::Const {
      continue;
    }
    for declarator in &decl.decls {
      let (ast::Pat::Ident(ident), Some(init)) =
        (&declarator.name, &declarator.init)
      else {
        continue;
      };
      if &*ident.id.sym != MANIFEST || !declares_permissions(init) {
        continue;
      }
      let manifest = reader.literal(init)?;
      let permissions_json = match &manifest {
        Value::Object(object) => {
          object.get("permissions").cloned().unwrap_or_default()
        }
        _ => Value::Null,
      };
      return Ok(Some(PackageManifest {
        permissions: reader.permissions(manifest, declarator.range())?,
        permissions_json,
      }));
    }
  }
  Ok(None)
}

/// Whether `expr` is an object literal with a `permissions` key, under
/// parentheses and TypeScript-only wrappers.
fn declares_permissions(expr: &ast::Expr) -> bool {
  match expr {
    ast::Expr::Paren(paren) => declares_permissions(&paren.expr),
    ast::Expr::TsConstAssertion(expr) => declares_permissions(&expr.expr),
    ast::Expr::TsSatisfies(expr) => declares_permissions(&expr.expr),
    ast::Expr::TsAs(expr) => declares_permissions(&expr.expr),
    ast::Expr::Object(object) => object.props.iter().any(|prop| {
      let ast::PropOrSpread::Prop(prop) = prop else {
        return false;
      };
      let ast::Prop::KeyValue(key_value) = &**prop else {
        return false;
      };
      match &key_value.key {
        ast::PropName::Ident(ident) => &*ident.sym == "permissions",
        ast::PropName::Str(str) => str.value.to_string_lossy() == "permissions",
        _ => false,
      }
    }),
    _ => false,
  }
}

const MANIFEST: &str = "manifest";
struct Reader<'a> {
  /// What the manifest belongs to, for errors.
  subject: String,
  /// "extension" or "package", for errors.
  what: &'static str,
  specifier: &'a ModuleSpecifier,
  parsed: &'a ParsedSource,
}

impl Reader<'_> {
  fn error(
    &self,
    range: Option<SourceRange>,
    message: String,
  ) -> ManifestError {
    let location = match range {
      Some(range) => {
        let position = self
          .parsed
          .text_info_lazy()
          .line_and_column_display(range.start);
        format!(
          "{}:{}:{}",
          self.specifier, position.line_number, position.column_number
        )
      }
      None => self.specifier.to_string(),
    };
    ManifestError {
      subject: self.subject.clone(),
      location,
      message,
    }
  }

  /// How to declare a manifest that can be read.
  fn hint(&self) -> String {
    format!(
      "declare the manifest as `export const manifest = {{ … }}`, so it can be read without running the {}",
      self.what
    )
  }

  fn non_function(&self, range: SourceRange, export: &str) -> ManifestError {
    self.error(
      Some(range),
      format!(
        "export \"{export}\" is not a function. An extension only exports functions, which the app calls across the sandbox."
      ),
    )
  }

  fn read(&self) -> Result<ExtensionManifest, ManifestError> {
    let module = match self.parsed.program_ref() {
      deno_ast::ProgramRef::Module(module) => module,
      deno_ast::ProgramRef::Script(_) => {
        return Err(self.error(
          None,
          "it is not an ES module. Add `export const manifest = { permissions: [] }` and export its functions.".to_string(),
        ));
      }
    };

    // Top-level functions, for `export { f }`.
    let mut local_functions = Vec::new();
    for item in &module.body {
      if let ast::ModuleItem::Stmt(ast::Stmt::Decl(decl)) = item {
        match decl {
          ast::Decl::Fn(decl) if !decl.declare => {
            local_functions.push(decl.ident.sym.to_string());
          }
          ast::Decl::Var(decl)
            if decl.kind == ast::VarDeclKind::Const && !decl.declare =>
          {
            for declarator in &decl.decls {
              if let (ast::Pat::Ident(ident), Some(init)) =
                (&declarator.name, &declarator.init)
                && is_function(init)
              {
                local_functions.push(ident.id.sym.to_string());
              }
            }
          }
          _ => {}
        }
      }
    }

    let mut manifest = None;
    let mut exports = Vec::new();
    for item in &module.body {
      let ast::ModuleItem::ModuleDecl(decl) = item else {
        continue;
      };
      match decl {
        ast::ModuleDecl::Import(_) => {}
        ast::ModuleDecl::ExportDecl(export) => match &export.decl {
          ast::Decl::Fn(decl) => {
            if !decl.declare {
              exports.push(decl.ident.sym.to_string());
            }
          }
          ast::Decl::Var(decl) if decl.declare => {}
          ast::Decl::Var(decl) => {
            for declarator in &decl.decls {
              let ast::Pat::Ident(ident) = &declarator.name else {
                return Err(self.error(
                  Some(declarator.name.range()),
                  "destructured exports aren't supported. An extension only exports named functions.".to_string(),
                ));
              };
              let export_name = ident.id.sym.to_string();
              if export_name == MANIFEST {
                if decl.kind != ast::VarDeclKind::Const {
                  return Err(self.error(
                    Some(declarator.range()),
                    format!(
                      "the manifest must not be reassignable: {}",
                      self.hint()
                    ),
                  ));
                }
                let Some(init) = &declarator.init else {
                  return Err(self.error(
                    Some(declarator.range()),
                    format!("the manifest has no value: {}", self.hint()),
                  ));
                };
                manifest = Some((self.literal(init)?, declarator.range()));
              } else if decl.kind == ast::VarDeclKind::Const
                && declarator.init.as_deref().is_some_and(is_function)
              {
                exports.push(export_name);
              } else {
                return Err(
                  self.non_function(declarator.range(), &export_name),
                );
              }
            }
          }
          ast::Decl::Class(decl) => {
            if !decl.declare {
              return Err(self.non_function(decl.range(), &decl.ident.sym));
            }
          }
          ast::Decl::TsEnum(decl) => {
            if !decl.declare {
              return Err(self.non_function(decl.range(), &decl.id.sym));
            }
          }
          ast::Decl::TsModule(decl) => {
            if !decl.declare {
              let export_name = match &decl.id {
                ast::TsModuleName::Ident(ident) => ident.sym.to_string(),
                ast::TsModuleName::Str(str) => {
                  str.value.to_string_lossy().into_owned()
                }
              };
              return Err(self.non_function(decl.range(), &export_name));
            }
          }
          ast::Decl::Using(decl) => {
            return Err(self.error(
              Some(decl.range()),
              "exported `using` declarations aren't supported. An extension only exports functions.".to_string(),
            ));
          }
          ast::Decl::TsInterface(_) | ast::Decl::TsTypeAlias(_) => {}
        },
        ast::ModuleDecl::ExportNamed(named) => {
          if named.type_only {
            continue;
          }
          if named.src.is_some() {
            let namespace = named.specifiers.iter().any(|specifier| {
              matches!(specifier, ast::ExportSpecifier::Namespace(_))
            });
            return Err(self.error(
              Some(named.range()),
              if namespace {
                "`export * as` isn't supported. Export each function by name.".to_string()
              } else {
                "re-exports from other modules aren't supported. Export a function declared in the entry module instead.".to_string()
              },
            ));
          }
          for specifier in &named.specifiers {
            let ast::ExportSpecifier::Named(specifier) = specifier else {
              return Err(self.error(
                Some(named.range()),
                "only named exports of functions are supported.".to_string(),
              ));
            };
            if specifier.is_type_only {
              continue;
            }
            let local = export_name(&specifier.orig);
            let exported = specifier
              .exported
              .as_ref()
              .map(export_name)
              .unwrap_or_else(|| local.clone());
            if local == MANIFEST || exported == MANIFEST {
              return Err(self.error(
                Some(specifier.range()),
                format!(
                  "the manifest must be exported where it is declared: {}",
                  self.hint()
                ),
              ));
            }
            if local_functions.contains(&local) {
              exports.push(exported);
            } else {
              return Err(self.non_function(specifier.range(), &exported));
            }
          }
        }
        ast::ModuleDecl::ExportDefaultDecl(decl) => {
          if !matches!(decl.decl, ast::DefaultDecl::TsInterfaceDecl(_)) {
            return Err(self.error(
              Some(decl.range()),
              "a default export isn't supported. Export a named function instead.".to_string(),
            ));
          }
        }
        ast::ModuleDecl::ExportDefaultExpr(expr) => {
          return Err(self.error(
            Some(expr.range()),
            "a default export isn't supported. Export a named function instead.".to_string(),
          ));
        }
        ast::ModuleDecl::ExportAll(all) => {
          if !all.type_only {
            return Err(
              self.error(
                Some(all.range()),
                "`export *` isn't supported. Export each function by name."
                  .to_string(),
              ),
            );
          }
        }
        ast::ModuleDecl::TsImportEquals(decl) => {
          if decl.is_export && !decl.is_type_only {
            return Err(
              self.error(
                Some(decl.range()),
                "`export import` isn't supported. Export a function instead."
                  .to_string(),
              ),
            );
          }
        }
        ast::ModuleDecl::TsExportAssignment(decl) => {
          return Err(
            self.error(
              Some(decl.range()),
              "`export =` isn't supported. Export named functions instead."
                .to_string(),
            ),
          );
        }
        ast::ModuleDecl::TsNamespaceExport(_) => {}
      }
    }

    let Some((manifest, range)) = manifest else {
      return Err(self.error(
        None,
        "it has no manifest. Add `export const manifest = { permissions: [] }` and list the permissions it needs.".to_string(),
      ));
    };
    let permissions_json = match &manifest {
      Value::Object(object) => {
        object.get("permissions").cloned().unwrap_or_default()
      }
      _ => Value::Null,
    };
    Ok(ExtensionManifest {
      permissions: self.permissions(manifest, range)?,
      exports,
      permissions_json,
    })
  }

  /// The value of a literal expression, or an error at the first part that
  /// isn't one.
  fn literal(&self, expr: &ast::Expr) -> Result<Value, ManifestError> {
    let not_literal = |range: SourceRange| {
      self.error(
        Some(range),
        format!(
          "the manifest may only hold literal objects, arrays, strings and booleans, since it is read without running the {}; {}",
          self.what,
          self.hint(),
        ),
      )
    };
    match expr {
      ast::Expr::Paren(paren) => self.literal(&paren.expr),
      ast::Expr::TsConstAssertion(expr) => self.literal(&expr.expr),
      ast::Expr::TsSatisfies(expr) => self.literal(&expr.expr),
      ast::Expr::TsAs(expr) => self.literal(&expr.expr),
      ast::Expr::Lit(ast::Lit::Str(str)) => {
        Ok(Value::String(str.value.to_string_lossy().into_owned()))
      }
      ast::Expr::Lit(ast::Lit::Bool(bool)) => Ok(Value::Bool(bool.value)),
      ast::Expr::Tpl(tpl) if tpl.exprs.is_empty() => {
        let text = tpl
          .quasis
          .iter()
          .map(|quasi| match &quasi.cooked {
            Some(cooked) => Ok(cooked.to_string_lossy().into_owned()),
            None => Err(not_literal(quasi.range())),
          })
          .collect::<Result<String, _>>()?;
        Ok(Value::String(text))
      }
      ast::Expr::Array(array) => {
        let mut items = Vec::with_capacity(array.elems.len());
        for elem in &array.elems {
          match elem {
            Some(ast::ExprOrSpread { spread: None, expr }) => {
              items.push(self.literal(expr)?);
            }
            Some(elem) => return Err(not_literal(elem.range())),
            None => return Err(not_literal(array.range())),
          }
        }
        Ok(Value::Array(items))
      }
      ast::Expr::Object(object) => {
        let mut map = serde_json::Map::with_capacity(object.props.len());
        for prop in &object.props {
          let ast::PropOrSpread::Prop(prop) = prop else {
            return Err(not_literal(prop.range()));
          };
          let ast::Prop::KeyValue(key_value) = &**prop else {
            return Err(not_literal(prop.range()));
          };
          let key = match &key_value.key {
            ast::PropName::Ident(ident) => ident.sym.to_string(),
            ast::PropName::Str(str) => str.value.to_string_lossy().into_owned(),
            other => return Err(not_literal(other.range())),
          };
          map.insert(key, self.literal(&key_value.value)?);
        }
        Ok(Value::Object(map))
      }
      other => Err(not_literal(other.range())),
    }
  }

  fn permissions(
    &self,
    manifest: Value,
    range: SourceRange,
  ) -> Result<PermissionsOptions, ManifestError> {
    let error = |message: String| self.error(Some(range), message);
    let Value::Object(mut manifest) = manifest else {
      return Err(error(format!(
        "the manifest must be an object: {}",
        self.hint()
      )));
    };
    if let Some(field) = manifest.keys().find(|key| *key != "permissions") {
      return Err(error(format!(
        "Unknown field \"{field}\" in the manifest. Expected \"permissions\"."
      )));
    }
    let unknown = |permission: &str| {
      error(format!(
        "Unknown permission \"{permission}\" in manifest.permissions. Expected one of: {}.",
        EXTENSION_PERMISSION_NAMES.join(", ")
      ))
    };
    match manifest.remove("permissions") {
      None => Err(error(format!(
        "manifest.permissions is required. Use [] if the {} needs no permissions.",
        self.what
      ))),
      Some(Value::Array(names)) => {
        let mut list = Vec::with_capacity(names.len());
        for name in names {
          match name {
            Value::String(name)
              if EXTENSION_PERMISSION_NAMES.contains(&name.as_str()) =>
            {
              list.push(name);
            }
            Value::String(name) => return Err(unknown(&name)),
            other => {
              return Err(error(format!(
                "manifest.permissions must list permission names, got {other}."
              )));
            }
          }
        }
        Ok(permission_names_to_options(&list))
      }
      Some(Value::Object(permissions)) => {
        let mut options = PermissionsOptions::default();
        let mut all = false;
        let mut named = Vec::new();
        for (permission, value) in permissions {
          if permission == "all" {
            let Value::Bool(value) = value else {
              return Err(error(
                "manifest.permissions \"all\" must be true or false."
                  .to_string(),
              ));
            };
            all = value;
            continue;
          }
          let Some(list) = allow_list(&mut options, &permission) else {
            return Err(unknown(&permission));
          };
          named.push(permission.clone());
          *list = match value {
            Value::Bool(true) => Some(vec![]),
            Value::Bool(false) => None,
            Value::Array(entries) => {
              let mut out = Vec::with_capacity(entries.len());
              for entry in entries {
                let Value::String(entry) = entry else {
                  return Err(error(format!(
                    "manifest.permissions.{permission} must be true, false or a list of strings."
                  )));
                };
                let must_be_absolute = match permission.as_str() {
                  "read" | "write" | "ffi" => true,
                  "run" => AllowRunDescriptor::is_path(&entry),
                  _ => false,
                };
                if must_be_absolute
                  && !std::path::Path::new(&entry).is_absolute()
                {
                  return Err(error(format!(
                    "manifest.permissions.{permission} entry \"{entry}\" must be an absolute path. {} can't know where the app lives; ask for `true` and let the app grant its own paths.",
                    if self.what == "extension" {
                      "An extension"
                    } else {
                      "A package"
                    }
                  )));
                }
                out.push(entry);
              }
              (!out.is_empty()).then_some(out)
            }
            _ => {
              return Err(error(format!(
                "manifest.permissions.{permission} must be true, false or a list of strings."
              )));
            }
          };
        }
        if all {
          let every = permission_names_to_options(&["all".to_string()]);
          for permission in EXTENSION_PERMISSION_NAMES {
            if permission == "all"
              || named.iter().any(|name| name == permission)
            {
              continue;
            }
            let mut every = every.clone();
            if let (Some(list), Some(value)) = (
              allow_list(&mut options, permission),
              allow_list(&mut every, permission),
            ) {
              *list = value.clone();
            }
          }
        }
        Ok(options)
      }
      Some(other) => Err(error(format!(
        "manifest.permissions must be an array of permission names or an object, got {other}."
      ))),
    }
  }
}

/// The allow list in `options` for `permission`.
fn allow_list<'a>(
  options: &'a mut PermissionsOptions,
  permission: &str,
) -> Option<&'a mut Option<Vec<String>>> {
  Some(match permission {
    "read" => &mut options.allow_read,
    "write" => &mut options.allow_write,
    "import" => &mut options.allow_import,
    "env" => &mut options.allow_env,
    "net" => &mut options.allow_net,
    "run" => &mut options.allow_run,
    "ffi" => &mut options.allow_ffi,
    "sys" => &mut options.allow_sys,
    _ => return None,
  })
}

fn is_function(expr: &ast::Expr) -> bool {
  match expr {
    ast::Expr::Arrow(_) | ast::Expr::Fn(_) => true,
    ast::Expr::Paren(paren) => is_function(&paren.expr),
    ast::Expr::TsAs(expr) => is_function(&expr.expr),
    ast::Expr::TsSatisfies(expr) => is_function(&expr.expr),
    _ => false,
  }
}

fn export_name(name: &ast::ModuleExportName) -> String {
  match name {
    ast::ModuleExportName::Ident(ident) => ident.sym.to_string(),
    ast::ModuleExportName::Str(str) => str.value.to_string_lossy().into_owned(),
  }
}

#[cfg(test)]
mod test {
  use pretty_assertions::assert_eq;

  use super::*;

  fn extract(source: &str) -> Result<ExtensionManifest, String> {
    extract_manifest(
      "@scope/name",
      &ModuleSpecifier::parse("file:///ext/mod.ts").unwrap(),
      MediaType::TypeScript,
      source,
    )
    .map_err(|err| err.to_string())
  }

  fn svec(items: &[&str]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
  }

  #[test]
  fn literal_manifest() {
    let manifest = extract(
      r#"export const manifest = {
  permissions: { read: true, net: ["api.example.com", "x.com:443"], env: false },
};
export function f() {}"#,
    )
    .unwrap();
    assert_eq!(
      manifest,
      ExtensionManifest {
        permissions: PermissionsOptions {
          allow_read: Some(vec![]),
          allow_net: Some(svec(&["api.example.com", "x.com:443"])),
          ..Default::default()
        },
        exports: svec(&["f"]),
        permissions_json: serde_json::json!({
          "read": true,
          "net": ["api.example.com", "x.com:443"],
          "env": false,
        }),
      }
    );
  }

  #[test]
  fn names_form_and_empty() {
    let manifest =
      extract(r#"export const manifest = { permissions: ["read", "sys"] };"#)
        .unwrap();
    assert_eq!(
      manifest.permissions,
      PermissionsOptions {
        allow_read: Some(vec![]),
        allow_sys: Some(vec![]),
        ..Default::default()
      }
    );
    let manifest =
      extract("export const manifest = { permissions: [] };").unwrap();
    assert_eq!(manifest.permissions, PermissionsOptions::default());
    let manifest =
      extract("export const manifest = { permissions: {} };").unwrap();
    assert_eq!(manifest.permissions, PermissionsOptions::default());
  }

  #[test]
  fn all_fills_unnamed_permissions() {
    let manifest = extract(
      r#"export const manifest = { permissions: { all: true, net: ["a.com"], write: false } };"#,
    )
    .unwrap();
    assert_eq!(
      manifest.permissions,
      PermissionsOptions {
        allow_env: Some(vec![]),
        allow_net: Some(svec(&["a.com"])),
        allow_ffi: Some(vec![]),
        allow_read: Some(vec![]),
        allow_run: Some(vec![]),
        allow_sys: Some(vec![]),
        allow_import: Some(vec![]),
        ..Default::default()
      }
    );
  }

  #[test]
  fn typescript_wrappers_and_syntax() {
    let manifest = extract(
      r#"type Manifest = { permissions: Record<string, unknown> };
export const manifest = ({
  "permissions": { net: [`api.example.com`] } as const,
}) satisfies Manifest;
export const f = async (x: string): Promise<number> => x.length;
export const g = function (): void {};
export interface Options { a: string }
export type Alias = number;
export declare function h(): void;
export { type Options as O };
export type { Alias as A };"#,
    )
    .unwrap();
    assert_eq!(
      manifest.permissions.allow_net,
      Some(svec(&["api.example.com"]))
    );
    assert_eq!(manifest.exports, svec(&["f", "g"]));
  }

  #[test]
  fn computed_values_are_refused_with_a_position() {
    for (source, column) in [
      ("export const manifest = { permissions: perms };", 40),
      ("export const manifest = { permissions: make() };", 40),
      (
        "export const manifest = { permissions: { net: [`${host}`] } };",
        48,
      ),
      ("export const manifest = { ...base };", 27),
      ("export const manifest = { [key]: [] };", 27),
      ("export const manifest = { permissions };", 27),
      ("export const manifest = { permissions: { net: [1] } };", 48),
      (
        "export const manifest = { permissions: { read: null } };",
        48,
      ),
    ] {
      let err = extract(source).unwrap_err();
      assert!(
        err.starts_with(&format!(
          "Invalid extension \"@scope/name\" (file:///ext/mod.ts:1:{column}): "
        )),
        "{source}: {err}"
      );
      assert!(err.contains("literal"), "{source}: {err}");
    }
  }

  #[test]
  fn manifest_must_be_an_exported_const() {
    for source in [
      "export let manifest = { permissions: [] };",
      "export var manifest = { permissions: [] };",
      "const manifest = { permissions: [] };\nexport { manifest };",
      "const m = { permissions: [] };\nexport { m as manifest };",
    ] {
      let err = extract(source).unwrap_err();
      assert!(err.contains("export const manifest = {"), "{source}: {err}");
    }
  }

  #[test]
  fn missing_manifest() {
    let err = extract("export function f() {}").unwrap_err();
    assert_eq!(
      err,
      "Invalid extension \"@scope/name\" (file:///ext/mod.ts): it has no manifest. Add `export const manifest = { permissions: [] }` and list the permissions it needs."
    );
    let err = extract("export const manifest = {};").unwrap_err();
    assert!(err.contains("manifest.permissions is required"), "{err}");
    let err =
      extract("export const manifest = { permissions: [], name: \"x\" };")
        .unwrap_err();
    assert!(err.contains("Unknown field \"name\""), "{err}");
  }

  #[test]
  fn bad_permissions() {
    let err =
      extract(r#"export const manifest = { permissions: ["hrtime"] };"#)
        .unwrap_err();
    assert!(err.contains("Unknown permission \"hrtime\""), "{err}");
    let err = extract(
      r#"export const manifest = { permissions: { read: { allow: true } } };"#,
    )
    .unwrap_err();
    assert!(err.contains("true, false or a list of strings"), "{err}");
    let err =
      extract(r#"export const manifest = { permissions: { all: ["a"] } };"#)
        .unwrap_err();
    assert!(err.contains("\"all\" must be true or false"), "{err}");
  }

  #[test]
  fn file_system_paths_must_be_absolute() {
    let root = if cfg!(windows) { "C:\\\\data" } else { "/data" };
    let manifest = extract(&format!(
      r#"export const manifest = {{ permissions: {{ read: ["{root}"], run: ["git"] }} }};"#
    ))
    .unwrap();
    assert_eq!(manifest.permissions.allow_run, Some(svec(&["git"])));
    for (kind, entry) in [
      ("read", "./data"),
      ("write", "data"),
      ("ffi", "../lib.so"),
      ("run", "./bin/tool"),
    ] {
      let err = extract(&format!(
        r#"export const manifest = {{ permissions: {{ {kind}: ["{entry}"] }} }};"#
      ))
      .unwrap_err();
      assert!(
        err.contains(&format!(
          "manifest.permissions.{kind} entry \"{entry}\" must be an absolute path"
        )),
        "{err}"
      );
    }
  }

  #[test]
  fn function_exports() {
    let manifest = extract(
      r#"export const manifest = { permissions: [] };
export function a() {}
export async function b() {}
export const c = () => 1, d = async function () {};
function e() {}
const f = (x: number) => x;
export { e, f as g };
import { h } from "./h.ts";"#,
    )
    .unwrap();
    assert_eq!(manifest.exports, svec(&["a", "b", "c", "d", "e", "g"]));
  }

  #[test]
  fn non_function_exports_are_refused() {
    for (source, export) in [
      ("export const answer = 42;", "answer"),
      ("export let f = () => 1;", "f"),
      ("export class C {}", "C"),
      ("export enum E { A }", "E"),
      ("const x = 1;\nexport { x };", "x"),
      ("const x = 1;\nexport { x as y };", "y"),
      ("export namespace N { export const a = 1; }", "N"),
    ] {
      let source =
        format!("export const manifest = {{ permissions: [] }};\n{source}");
      let err = extract(&source).unwrap_err();
      assert!(
        err.contains(&format!("export \"{export}\"")),
        "{source}: {err}"
      );
      assert!(err.contains("only exports functions"), "{source}: {err}");
    }
  }

  #[test]
  fn other_export_forms_are_refused() {
    for (source, needle) in [
      ("export default function f() {}", "default export"),
      ("export default 42;", "default export"),
      ("export * from \"./x.ts\";", "export *"),
      ("export * as ns from \"./x.ts\";", "export *"),
      ("export { f } from \"./x.ts\";", "re-export"),
    ] {
      let source =
        format!("export const manifest = {{ permissions: [] }};\n{source}");
      let err = extract(&source).unwrap_err();
      assert!(err.contains(needle), "{source}: {err}");
    }
    // Type-only re-exports are fine.
    extract(
      "export const manifest = { permissions: [] };\nexport type * from \"./types.ts\";\nexport type { T } from \"./types.ts\";",
    )
    .unwrap();
  }

  #[test]
  fn syntax_error() {
    let err = extract("export const manifest = {").unwrap_err();
    assert!(
      err.starts_with(
        "Invalid extension \"@scope/name\" (file:///ext/mod.ts:1:"
      ),
      "{err}"
    );
  }
  fn package(source: &str) -> Result<Option<PackageManifest>, String> {
    extract_package_manifest(
      "./vendor/csv/",
      &ModuleSpecifier::parse("file:///app/vendor/csv/mod.ts").unwrap(),
      MediaType::TypeScript,
      source,
    )
    .map_err(|err| err.to_string())
  }

  #[test]
  fn package_manifest_is_optional() {
    assert_eq!(package("export function parse() {}").unwrap(), None);
    // Other exports named `manifest` aren't a manifest.
    for source in [
      "export function manifest() {}",
      "export const manifest = { name: \"app\", icons: [] };",
      "export let manifest = { permissions: [] };",
      "const manifest = { permissions: [] };\nexport { manifest };",
      "export class Widget {}\nexport let counter = 0;",
    ] {
      assert_eq!(package(source).unwrap(), None, "{source}");
    }
  }

  #[test]
  fn package_manifest_is_read_as_an_extension_manifest() {
    let manifest = package(
      "export const manifest = { permissions: { net: [\"api.example.com\"] } } as const;\nexport class Widget {}",
    )
    .unwrap()
    .unwrap();
    assert_eq!(
      manifest.permissions,
      PermissionsOptions {
        allow_net: Some(svec(&["api.example.com"])),
        ..Default::default()
      }
    );
    assert_eq!(
      manifest.permissions_json,
      serde_json::json!({ "net": ["api.example.com"] })
    );
    let err = package(
      "export const manifest = { permissions: { read: [\"./data\"] } };",
    )
    .unwrap_err();
    assert!(
      err.starts_with(
        "Invalid manifest of a package in isolate[\"./vendor/csv/\"] (file:///app/vendor/csv/mod.ts:1:14): "
      ),
      "{err}"
    );
    assert!(
      err.contains("A package can't know where the app lives"),
      "{err}"
    );
    let err = package("export const manifest = { permissions: [], x: 1 };")
      .unwrap_err();
    assert!(err.contains("without running the package"), "{err}");
  }
}
