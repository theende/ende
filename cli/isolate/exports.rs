// Copyright 2018-2026 the Ende authors. MIT license.

//! What crosses a sandbox's boundary, read from source: the exports of a
//! sandboxed module and what kind each is, and which exports an importer
//! asks for. Only functions and structured-clonable constants cross (see
//! `runtime/js/11_workers.js`); classes and `let`/`var` bindings never do,
//! and the kind of the rest shows only when the sandbox runs.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use deno_ast::MediaType;
use deno_ast::ModuleSpecifier;
use deno_ast::swc::ast;
use deno_graph::ModuleGraph;

/// What an export is, as far as the source shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
  /// A function declaration or a `const` holding a function expression.
  Function,
  /// A class: it can't cross.
  Class,
  /// A `let` or `var` binding: its value can change, so it can't cross.
  Mutable,
  /// A `const` holding a literal, or an enum.
  Value,
  /// Anything else: decided when the sandbox runs.
  Unknown,
}

impl ExportKind {
  /// Why an export of this kind can't cross, if it can't.
  pub fn unsupported(&self) -> Option<&'static str> {
    match self {
      Self::Class => Some("a class"),
      Self::Mutable => {
        Some("a let or var binding, whose value can change after a snapshot")
      }
      Self::Function | Self::Value | Self::Unknown => None,
    }
  }
}

/// The exports of a module, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleExports {
  pub names: BTreeMap<String, ExportKind>,
  /// False when an `export *` reached a module whose exports couldn't be
  /// read, so names may be missing.
  pub complete: bool,
}

/// A binding at the top level of a module.
#[derive(Clone, Debug)]
enum Local {
  Kind(ExportKind),
  /// `import { name } from "src"` (or `import name from`, as "default").
  Import {
    src: String,
    name: String,
  },
}

/// A module as [`module_exports`] reads it.
pub enum SourceModule {
  /// JavaScript or TypeScript: its specifier after redirects, text and
  /// media type.
  Js {
    specifier: ModuleSpecifier,
    text: std::sync::Arc<str>,
    media_type: MediaType,
  },
  /// A JSON module: data, exported as `default`.
  Json,
}

/// Where [`module_exports`] reads modules from: the module graph when
/// running and checking, the editor's documents and cache in the editor.
pub trait ExportsSource {
  /// The module `specifier` names, if it is loaded.
  fn module(&self, specifier: &ModuleSpecifier) -> Option<SourceModule>;
  /// What the code import `src` in module `referrer` resolves to.
  fn dependency(
    &self,
    referrer: &ModuleSpecifier,
    src: &str,
  ) -> Option<ModuleSpecifier>;
}

impl ExportsSource for ModuleGraph {
  fn module(&self, specifier: &ModuleSpecifier) -> Option<SourceModule> {
    match self.get(specifier)? {
      deno_graph::Module::Js(module) => Some(SourceModule::Js {
        specifier: module.specifier.clone(),
        text: module.source.text.clone(),
        media_type: module.media_type,
      }),
      deno_graph::Module::Json(_) => Some(SourceModule::Json),
      _ => None,
    }
  }

  fn dependency(
    &self,
    referrer: &ModuleSpecifier,
    src: &str,
  ) -> Option<ModuleSpecifier> {
    let deno_graph::Module::Js(module) = self.get(referrer)? else {
      return None;
    };
    module
      .dependencies
      .get(src)
      .and_then(|dependency| dependency.get_code())
      .cloned()
  }
}

/// The exports of the JavaScript or TypeScript module `specifier` in
/// `graph`, following re-exports through the graph.
pub fn module_exports(
  graph: &ModuleGraph,
  specifier: &ModuleSpecifier,
) -> ModuleExports {
  module_exports_from(graph, specifier)
}

/// The exports of the JavaScript or TypeScript module `specifier`, read
/// from `source`, following re-exports.
pub fn module_exports_from(
  source: &dyn ExportsSource,
  specifier: &ModuleSpecifier,
) -> ModuleExports {
  let mut cache = HashMap::new();
  exports_of(source, specifier, &mut Vec::new(), &mut cache)
}

fn exports_of(
  modules: &dyn ExportsSource,
  specifier: &ModuleSpecifier,
  visiting: &mut Vec<ModuleSpecifier>,
  cache: &mut HashMap<ModuleSpecifier, ModuleExports>,
) -> ModuleExports {
  let (specifier, text, media_type) = match modules.module(specifier) {
    Some(SourceModule::Js {
      specifier,
      text,
      media_type,
    }) => (specifier, text, media_type),
    Some(SourceModule::Json) => {
      return ModuleExports {
        names: BTreeMap::from([("default".to_string(), ExportKind::Value)]),
        complete: true,
      };
    }
    None => return ModuleExports::default(),
  };
  if let Some(exports) = cache.get(&specifier) {
    return exports.clone();
  }
  if visiting.contains(&specifier) {
    // a cycle of `export *`: the other end adds its own names
    return ModuleExports {
      names: BTreeMap::new(),
      complete: true,
    };
  }
  let Ok(parsed) = deno_ast::parse_module(deno_ast::ParseParams {
    specifier: specifier.clone(),
    text,
    media_type,
    capture_tokens: false,
    scope_analysis: false,
    maybe_syntax: None,
  }) else {
    return ModuleExports::default();
  };
  let deno_ast::ProgramRef::Module(program) = parsed.program_ref() else {
    return ModuleExports::default();
  };
  visiting.push(specifier.clone());
  let resolve = |src: &str| -> Option<ModuleSpecifier> {
    modules.dependency(&specifier, src)
  };

  let mut locals = HashMap::new();
  for item in &program.body {
    match item {
      ast::ModuleItem::Stmt(ast::Stmt::Decl(decl)) => {
        add_decl(decl, &mut locals)
      }
      ast::ModuleItem::ModuleDecl(ast::ModuleDecl::ExportDecl(export)) => {
        add_decl(&export.decl, &mut locals)
      }
      ast::ModuleItem::ModuleDecl(ast::ModuleDecl::Import(import))
        if !import.type_only =>
      {
        let src = import.src.value.to_string_lossy().into_owned();
        for specifier in &import.specifiers {
          match specifier {
            ast::ImportSpecifier::Named(named) if !named.is_type_only => {
              let name = named
                .imported
                .as_ref()
                .map(export_name)
                .unwrap_or_else(|| named.local.sym.to_string());
              locals.insert(
                named.local.sym.to_string(),
                Local::Import {
                  src: src.clone(),
                  name,
                },
              );
            }
            ast::ImportSpecifier::Named(_) => {}
            ast::ImportSpecifier::Default(default) => {
              locals.insert(
                default.local.sym.to_string(),
                Local::Import {
                  src: src.clone(),
                  name: "default".to_string(),
                },
              );
            }
            ast::ImportSpecifier::Namespace(namespace) => {
              locals.insert(
                namespace.local.sym.to_string(),
                Local::Kind(ExportKind::Unknown),
              );
            }
          }
        }
      }
      _ => {}
    }
  }

  let mut exports = ModuleExports {
    names: BTreeMap::new(),
    complete: true,
  };
  let mut star_sources = Vec::new();
  let local_kind = |name: &str,
                    visiting: &mut Vec<ModuleSpecifier>,
                    cache: &mut HashMap<ModuleSpecifier, ModuleExports>|
   -> ExportKind {
    match locals.get(name) {
      Some(Local::Kind(kind)) => *kind,
      Some(Local::Import { src, name }) => {
        imported_kind(modules, resolve(src), name, visiting, cache)
      }
      None => ExportKind::Unknown,
    }
  };
  for item in &program.body {
    let ast::ModuleItem::ModuleDecl(decl) = item else {
      continue;
    };
    match decl {
      ast::ModuleDecl::ExportDecl(export) => {
        for (name, kind) in decl_bindings(&export.decl) {
          exports.names.insert(name, kind);
        }
      }
      ast::ModuleDecl::ExportDefaultDecl(export) => {
        let kind = match &export.decl {
          ast::DefaultDecl::Fn(_) => ExportKind::Function,
          ast::DefaultDecl::Class(_) => ExportKind::Class,
          ast::DefaultDecl::TsInterfaceDecl(_) => continue,
        };
        exports.names.insert("default".to_string(), kind);
      }
      ast::ModuleDecl::ExportDefaultExpr(export) => {
        let kind = match unwrap_expr(&export.expr) {
          ast::Expr::Ident(ident) => {
            local_kind(ident.sym.as_ref(), visiting, cache)
          }
          expr => expr_kind(expr),
        };
        exports.names.insert("default".to_string(), kind);
      }
      ast::ModuleDecl::ExportNamed(export) if !export.type_only => {
        let source = export
          .src
          .as_ref()
          .map(|src| src.value.to_string_lossy().into_owned());
        for specifier in &export.specifiers {
          match specifier {
            ast::ExportSpecifier::Named(named) if !named.is_type_only => {
              let orig = export_name(&named.orig);
              let name = named
                .exported
                .as_ref()
                .map(export_name)
                .unwrap_or_else(|| orig.clone());
              let kind = match &source {
                Some(src) => {
                  imported_kind(modules, resolve(src), &orig, visiting, cache)
                }
                None => local_kind(&orig, visiting, cache),
              };
              exports.names.insert(name, kind);
            }
            ast::ExportSpecifier::Named(_) => {}
            ast::ExportSpecifier::Namespace(namespace) => {
              exports
                .names
                .insert(export_name(&namespace.name), ExportKind::Unknown);
            }
            ast::ExportSpecifier::Default(default) => {
              exports
                .names
                .insert(default.exported.sym.to_string(), ExportKind::Unknown);
            }
          }
        }
      }
      ast::ModuleDecl::ExportAll(export) if !export.type_only => {
        star_sources.push(export.src.value.to_string_lossy().into_owned());
      }
      _ => {}
    }
  }
  // `export *` never re-exports `default`, and a module's own exports win.
  for src in star_sources {
    match resolve(&src) {
      Some(target) => {
        let star = exports_of(modules, &target, visiting, cache);
        exports.complete &= star.complete;
        for (name, kind) in star.names {
          if name != "default" {
            exports.names.entry(name).or_insert(kind);
          }
        }
      }
      None => exports.complete = false,
    }
  }
  visiting.pop();
  cache.insert(specifier, exports.clone());
  exports
}

fn imported_kind(
  modules: &dyn ExportsSource,
  target: Option<ModuleSpecifier>,
  name: &str,
  visiting: &mut Vec<ModuleSpecifier>,
  cache: &mut HashMap<ModuleSpecifier, ModuleExports>,
) -> ExportKind {
  let Some(target) = target else {
    return ExportKind::Unknown;
  };
  exports_of(modules, &target, visiting, cache)
    .names
    .get(name)
    .copied()
    .unwrap_or(ExportKind::Unknown)
}

fn add_decl(decl: &ast::Decl, locals: &mut HashMap<String, Local>) {
  for (name, kind) in decl_bindings(decl) {
    locals.insert(name, Local::Kind(kind));
  }
}

/// The value bindings a declaration makes, with their kinds. Types and
/// `declare`d bindings make none.
fn decl_bindings(decl: &ast::Decl) -> Vec<(String, ExportKind)> {
  match decl {
    ast::Decl::Fn(decl) if !decl.declare => {
      vec![(decl.ident.sym.to_string(), ExportKind::Function)]
    }
    ast::Decl::Class(decl) if !decl.declare => {
      vec![(decl.ident.sym.to_string(), ExportKind::Class)]
    }
    ast::Decl::Var(decl) if !decl.declare => {
      let mut bindings = Vec::new();
      for declarator in &decl.decls {
        let kind = match decl.kind {
          ast::VarDeclKind::Let | ast::VarDeclKind::Var => ExportKind::Mutable,
          ast::VarDeclKind::Const => match (&declarator.name, &declarator.init)
          {
            (ast::Pat::Ident(_), Some(init)) => expr_kind(unwrap_expr(init)),
            _ => ExportKind::Unknown,
          },
        };
        let mut names = Vec::new();
        pat_names(&declarator.name, &mut names);
        bindings.extend(names.into_iter().map(|name| (name, kind)));
      }
      bindings
    }
    ast::Decl::TsEnum(decl) if !decl.declare => {
      vec![(decl.id.sym.to_string(), ExportKind::Value)]
    }
    ast::Decl::TsModule(decl) if !decl.declare => match &decl.id {
      ast::TsModuleName::Ident(ident) => {
        vec![(ident.sym.to_string(), ExportKind::Unknown)]
      }
      ast::TsModuleName::Str(_) => Vec::new(),
    },
    _ => Vec::new(),
  }
}

fn pat_names(pat: &ast::Pat, names: &mut Vec<String>) {
  match pat {
    ast::Pat::Ident(ident) => names.push(ident.id.sym.to_string()),
    ast::Pat::Array(array) => {
      for element in array.elems.iter().flatten() {
        pat_names(element, names);
      }
    }
    ast::Pat::Object(object) => {
      for prop in &object.props {
        match prop {
          ast::ObjectPatProp::KeyValue(prop) => pat_names(&prop.value, names),
          ast::ObjectPatProp::Assign(prop) => {
            names.push(prop.key.id.sym.to_string())
          }
          ast::ObjectPatProp::Rest(rest) => pat_names(&rest.arg, names),
        }
      }
    }
    ast::Pat::Rest(rest) => pat_names(&rest.arg, names),
    ast::Pat::Assign(assign) => pat_names(&assign.left, names),
    ast::Pat::Invalid(_) | ast::Pat::Expr(_) => {}
  }
}

/// The expression inside parentheses and TypeScript-only wrappers.
fn unwrap_expr(expr: &ast::Expr) -> &ast::Expr {
  match expr {
    ast::Expr::Paren(paren) => unwrap_expr(&paren.expr),
    ast::Expr::TsAs(ts) => unwrap_expr(&ts.expr),
    ast::Expr::TsSatisfies(ts) => unwrap_expr(&ts.expr),
    ast::Expr::TsNonNull(ts) => unwrap_expr(&ts.expr),
    ast::Expr::TsConstAssertion(ts) => unwrap_expr(&ts.expr),
    ast::Expr::TsTypeAssertion(ts) => unwrap_expr(&ts.expr),
    expr => expr,
  }
}

fn expr_kind(expr: &ast::Expr) -> ExportKind {
  match expr {
    ast::Expr::Arrow(_) | ast::Expr::Fn(_) => ExportKind::Function,
    ast::Expr::Class(_) => ExportKind::Class,
    expr if is_literal(expr) => ExportKind::Value,
    _ => ExportKind::Unknown,
  }
}

/// A literal made only of literals: data, whatever runs.
fn is_literal(expr: &ast::Expr) -> bool {
  match unwrap_expr(expr) {
    ast::Expr::Lit(lit) => !matches!(lit, ast::Lit::JSXText(_)),
    ast::Expr::Tpl(tpl) => tpl.exprs.is_empty(),
    ast::Expr::Unary(unary) => {
      matches!(unary.op, ast::UnaryOp::Minus | ast::UnaryOp::Plus)
        && matches!(&*unary.arg, ast::Expr::Lit(ast::Lit::Num(_)))
    }
    ast::Expr::Array(array) => {
      array.elems.iter().all(|element| match element {
        Some(element) => element.spread.is_none() && is_literal(&element.expr),
        None => true,
      })
    }
    ast::Expr::Object(object) => object.props.iter().all(|prop| match prop {
      ast::PropOrSpread::Prop(prop) => match &**prop {
        ast::Prop::KeyValue(prop) => {
          !matches!(prop.key, ast::PropName::Computed(_))
            && is_literal(&prop.value)
        }
        _ => false,
      },
      ast::PropOrSpread::Spread(_) => false,
    }),
    _ => false,
  }
}

fn export_name(name: &ast::ModuleExportName) -> String {
  match name {
    ast::ModuleExportName::Ident(ident) => ident.sym.to_string(),
    ast::ModuleExportName::Str(str) => str.value.to_string_lossy().into_owned(),
  }
}

/// The exports an importer asks one module for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Requested {
  /// Imported or re-exported by name (`default` for a default import).
  pub names: BTreeSet<String>,
  /// A namespace import or `export *`: every export that can cross.
  pub all: bool,
}

/// What each import of the JavaScript module `code` (the code that runs,
/// so TypeScript's type-only imports are already gone) asks for, by
/// specifier as written. `None` if it doesn't parse as a module.
pub fn requested_exports(
  specifier: &ModuleSpecifier,
  code: &str,
) -> Option<HashMap<String, Requested>> {
  let parsed = deno_ast::parse_module(deno_ast::ParseParams {
    specifier: specifier.clone(),
    text: code.into(),
    media_type: MediaType::JavaScript,
    capture_tokens: false,
    scope_analysis: false,
    maybe_syntax: None,
  })
  .ok()?;
  let deno_ast::ProgramRef::Module(program) = parsed.program_ref() else {
    return None;
  };
  let mut requested: HashMap<String, Requested> = HashMap::new();
  for item in &program.body {
    let ast::ModuleItem::ModuleDecl(decl) = item else {
      continue;
    };
    match decl {
      ast::ModuleDecl::Import(import) => {
        let entry = requested
          .entry(import.src.value.to_string_lossy().into_owned())
          .or_default();
        for specifier in &import.specifiers {
          match specifier {
            ast::ImportSpecifier::Named(named) => {
              entry.names.insert(
                named
                  .imported
                  .as_ref()
                  .map(export_name)
                  .unwrap_or_else(|| named.local.sym.to_string()),
              );
            }
            ast::ImportSpecifier::Default(_) => {
              entry.names.insert("default".to_string());
            }
            ast::ImportSpecifier::Namespace(_) => entry.all = true,
          }
        }
      }
      ast::ModuleDecl::ExportNamed(export) => {
        let Some(src) = &export.src else {
          continue;
        };
        let entry = requested
          .entry(src.value.to_string_lossy().into_owned())
          .or_default();
        for specifier in &export.specifiers {
          match specifier {
            ast::ExportSpecifier::Named(named) => {
              entry.names.insert(export_name(&named.orig));
            }
            ast::ExportSpecifier::Namespace(_) => entry.all = true,
            ast::ExportSpecifier::Default(_) => {
              entry.names.insert("default".to_string());
            }
          }
        }
      }
      ast::ModuleDecl::ExportAll(export) => {
        requested
          .entry(export.src.value.to_string_lossy().into_owned())
          .or_default()
          .all = true;
      }
      _ => {}
    }
  }
  Some(requested)
}

/// One name a module imports by value from another, where it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueImport {
  /// The specifier as written.
  pub src: String,
  /// The export it names (`default` for a default import).
  pub name: String,
  /// Where the name is written: zero-based line and column of its start
  /// and end.
  pub start: (usize, usize),
  pub end: (usize, usize),
}

/// Every name the module `text` (JavaScript or TypeScript, of
/// `media_type`) imports or re-exports by value, by name: what would cross
/// a sandbox's boundary if `src` is sandboxed. Type-only imports and
/// specifiers are left out (TypeScript drops them), and so are namespace
/// imports and `export *`, which get only what can cross. `None` if it
/// doesn't parse as a module.
pub fn value_imports(
  specifier: &ModuleSpecifier,
  media_type: MediaType,
  text: &str,
) -> Option<Vec<ValueImport>> {
  use deno_ast::SourceRangedForSpanned as _;
  let parsed = deno_ast::parse_module(deno_ast::ParseParams {
    specifier: specifier.clone(),
    text: text.into(),
    media_type,
    capture_tokens: false,
    scope_analysis: false,
    maybe_syntax: None,
  })
  .ok()?;
  let deno_ast::ProgramRef::Module(program) = parsed.program_ref() else {
    return None;
  };
  let text_info = parsed.text_info_lazy();
  let position = |pos: deno_ast::SourcePos| {
    let index = text_info.line_and_column_index(pos);
    (index.line_index, index.column_index)
  };
  let mut imports = Vec::new();
  let mut push = |src: &str, name: String, range: deno_ast::SourceRange| {
    imports.push(ValueImport {
      src: src.to_string(),
      name,
      start: position(range.start),
      end: position(range.end),
    });
  };
  for item in &program.body {
    let ast::ModuleItem::ModuleDecl(decl) = item else {
      continue;
    };
    match decl {
      ast::ModuleDecl::Import(import) if !import.type_only => {
        let src = import.src.value.to_string_lossy();
        for specifier in &import.specifiers {
          match specifier {
            ast::ImportSpecifier::Named(named) if !named.is_type_only => {
              let name = named
                .imported
                .as_ref()
                .map(export_name)
                .unwrap_or_else(|| named.local.sym.to_string());
              push(&src, name, named.range());
            }
            ast::ImportSpecifier::Default(default) => {
              push(&src, "default".to_string(), default.range());
            }
            _ => {}
          }
        }
      }
      ast::ModuleDecl::ExportNamed(export) if !export.type_only => {
        let Some(src) = &export.src else {
          continue;
        };
        let src = src.value.to_string_lossy();
        for specifier in &export.specifiers {
          match specifier {
            ast::ExportSpecifier::Named(named) if !named.is_type_only => {
              push(&src, export_name(&named.orig), named.range());
            }
            ast::ExportSpecifier::Default(default) => {
              push(&src, "default".to_string(), default.range());
            }
            _ => {}
          }
        }
      }
      _ => {}
    }
  }
  Some(imports)
}

#[cfg(test)]
mod test {
  use deno_graph::source::MemoryLoader;
  use deno_graph::source::Source;
  use pretty_assertions::assert_eq;

  use super::*;

  async fn graph(modules: &[(&str, &str)]) -> ModuleGraph {
    let loader = MemoryLoader::new(
      modules
        .iter()
        .map(|(specifier, content)| {
          (
            specifier.to_string(),
            Source::Module {
              specifier: specifier.to_string(),
              maybe_headers: None,
              content: content.to_string(),
            },
          )
        })
        .collect(),
      Vec::new(),
    );
    let analyzer = deno_graph::ast::CapturingModuleAnalyzer::default();
    let mut graph = ModuleGraph::new(deno_graph::GraphKind::CodeOnly);
    graph
      .build(
        vec![ModuleSpecifier::parse(modules[0].0).unwrap()],
        Vec::new(),
        &loader,
        deno_graph::BuildOptions {
          module_analyzer: &analyzer,
          ..Default::default()
        },
      )
      .await;
    graph
  }

  #[tokio::test]
  async fn kinds_follow_declarations_and_re_exports() {
    let graph = graph(&[
      (
        "file:///lib/mod.ts",
        "export function parse() {}
         export const stringify = (x: unknown) => String(x);
         export class Parser {}
         export let counter = 0;
         export var legacy = 1;
         export const VERSION = '1.0';
         export const OPTIONS = { sep: ',', quote: ['\"'], n: -1 };
         export const made = make();
         export enum Mode { A, B }
         export interface Options {}
         export type Row = string[];
         export declare const ambient: number;
         function local() {}
         const Local = class {};
         export { local as renamed, Local };
         import { helper, Helper as H } from './helper.ts';
         export { helper, H };
         export { other } from './helper.ts';
         export * from './star.ts';
         export default class {}
         function make() { return 1; }",
      ),
      (
        "file:///lib/helper.ts",
        "export function helper() {}
         export class Helper {}
         export let other = 1;",
      ),
      (
        "file:///lib/star.ts",
        "export const fromStar = () => 1;
         export function parse() {}
         export default function () {}",
      ),
    ])
    .await;
    let exports = module_exports(
      &graph,
      &ModuleSpecifier::parse("file:///lib/mod.ts").unwrap(),
    );
    let kinds = exports
      .names
      .iter()
      .map(|(name, kind)| (name.as_str(), *kind))
      .collect::<Vec<_>>();
    use ExportKind::*;
    assert_eq!(
      kinds,
      vec![
        ("H", Class),
        ("Local", Class),
        ("Mode", Value),
        ("OPTIONS", Value),
        ("Parser", Class),
        ("VERSION", Value),
        ("counter", Mutable),
        ("default", Class),
        ("fromStar", Function),
        ("helper", Function),
        ("legacy", Mutable),
        ("made", Unknown),
        ("other", Mutable),
        ("parse", Function),
        ("renamed", Function),
        ("stringify", Function),
      ]
    );
    assert!(exports.complete);
  }

  #[test]
  fn requested_exports_of_the_code_that_runs() {
    let requested = requested_exports(
      &ModuleSpecifier::parse("file:///app/main.js").unwrap(),
      "import { parse, stringify as s } from 'jsr:@std/csv';
       import csv from 'jsr:@std/csv';
       import * as all from './vendor/a.ts';
       import './vendor/side.ts';
       export { x, default as y } from './vendor/b.ts';
       export * from './vendor/c.ts';
       export * as ns from './vendor/d.ts';",
    )
    .unwrap();
    let get = |src: &str| requested.get(src).cloned().unwrap();
    assert_eq!(
      get("jsr:@std/csv"),
      Requested {
        names: ["default", "parse", "stringify"]
          .into_iter()
          .map(String::from)
          .collect(),
        all: false,
      }
    );
    assert!(get("./vendor/a.ts").all);
    assert_eq!(get("./vendor/side.ts"), Requested::default());
    assert_eq!(
      get("./vendor/b.ts").names,
      ["default", "x"].into_iter().map(String::from).collect()
    );
    assert!(get("./vendor/c.ts").all);
    assert!(get("./vendor/d.ts").all);
  }

  #[test]
  fn value_imports_leave_out_types_and_namespaces() {
    let specifier = ModuleSpecifier::parse("file:///app/main.ts").unwrap();
    let imports = value_imports(
      &specifier,
      MediaType::TypeScript,
      "import type { A } from \"./a.ts\";\nimport { type B, C, D as E } from \"./a.ts\";\nimport F, * as G from \"./b.ts\";\nexport { H } from \"./c.ts\";\nexport type { I } from \"./c.ts\";\nexport * from \"./d.ts\";\n",
    )
    .unwrap();
    assert_eq!(
      imports
        .iter()
        .map(|import| (import.src.as_str(), import.name.as_str(), import.start))
        .collect::<Vec<_>>(),
      [
        ("./a.ts", "C", (1, 17)),
        ("./a.ts", "D", (1, 20)),
        ("./b.ts", "default", (2, 7)),
        ("./c.ts", "H", (3, 9)),
      ]
    );
  }
}
