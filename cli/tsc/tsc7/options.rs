// Copyright 2026 the Ende authors. MIT license.

//! Turns the compiler options Ende resolves for its TypeScript 6 checker into
//! a `compilerOptions` object that TypeScript 7's `tsc` accepts.
//!
//! Ende's check defaults aren't valid for stock TypeScript (custom `ende.*`
//! lib names, `importsNotUsedAsValues`, a virtual `tsBuildInfoFile`), and TS 7
//! changed several defaults. So this starts from a fixed base, copies the
//! options both versions understand, and sets the changed defaults explicitly.

use ende_config::ende_json::CompilerOptions;
use ende_core::serde_json::Map;
use ende_core::serde_json::Value;
use ende_core::serde_json::json;
use ende_core::url::Url;

/// Options copied unchanged from Ende's resolved options when they're set.
const PASSTHROUGH_OPTIONS: &[&str] = &[
  "allowJs",
  "allowSyntheticDefaultImports",
  "allowUnreachableCode",
  "allowUnusedLabels",
  "checkJs",
  "declaration",
  "emitDecoratorMetadata",
  "erasableSyntaxOnly",
  "exactOptionalPropertyTypes",
  "experimentalDecorators",
  "isolatedDeclarations",
  "isolatedModules",
  "jsxFactory",
  "jsxFragmentFactory",
  "jsxImportSource",
  "moduleDetection",
  "noErrorTruncation",
  "noFallthroughCasesInSwitch",
  "noImplicitAny",
  "noImplicitOverride",
  "noImplicitReturns",
  "noImplicitThis",
  "noPropertyAccessFromIndexSignature",
  "noUncheckedIndexedAccess",
  "noUnusedLocals",
  "noUnusedParameters",
  "resolveJsonModule",
  "skipLibCheck",
  "strict",
  "strictBindCallApply",
  "strictBuiltinIteratorReturn",
  "strictFunctionTypes",
  "strictNullChecks",
  "strictPropertyInitialization",
  "target",
  "useDefineForClassFields",
  "useUnknownInCatchVariables",
  "verbatimModuleSyntax",
];

/// User options that TypeScript 7 removed, with what still uses them. Ende
/// only passes a short list of user options to the checker
/// (`parse_compiler_options` in libs/resolver/ende_json.rs), so this is the
/// only one that can arrive.
const REMOVED_OPTIONS: &[(&str, &str)] = &[(
  "baseUrl",
  "Ende still resolves \"paths\" and \"rootDirs\" against it.",
)];

/// The translated options, plus the Ende libs that `lib` named. Those can't go
/// in `lib` (TS 7 only knows its own lib names), so they become root files.
#[derive(Debug, PartialEq)]
pub struct TranslatedOptions {
  pub compiler_options: Map<String, Value>,
  pub ende_libs: Vec<String>,
}

/// A config file setting an option TypeScript 7 removed. The option is left
/// out of the generated config (it's never passed through), so this is only a
/// warning.
#[derive(Debug, PartialEq)]
pub struct RemovedOption<'a> {
  pub name: &'static str,
  pub specifier: &'a Url,
  hint: &'static str,
}

impl std::fmt::Display for RemovedOption<'_> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "compilerOptions.{} has no effect in TypeScript 7, which removed it, so \
       tsc doesn't get it. {}\n    at {}",
      self.name, self.hint, self.specifier
    )
  }
}

/// The options TypeScript 7 removed that config files set.
pub fn removed_options<'a>(
  sources: impl Iterator<Item = (&'a Url, Option<&'a CompilerOptions>)>,
) -> Vec<RemovedOption<'a>> {
  let mut removed = Vec::new();
  for (specifier, compiler_options) in sources {
    let Some(object) = compiler_options.and_then(|o| o.0.as_object()) else {
      continue;
    };
    for (name, hint) in REMOVED_OPTIONS {
      if object.contains_key(*name) {
        removed.push(RemovedOption {
          name,
          specifier,
          hint,
        });
      }
    }
  }
  removed
}

/// Translates `resolved` (Ende's resolved check options). `root_dirs` are the
/// absolute `rootDirs`, and `paths` the generated module paths. `ende_lib`
/// maps a lowercase `ende.*` or `deno.*` lib name to its `ende.*` name if Ende
/// has that lib.
pub fn translate(
  resolved: &Value,
  root_dirs: Vec<String>,
  paths: Map<String, Value>,
  ende_lib: impl Fn(&str) -> Option<String>,
) -> TranslatedOptions {
  let empty = Map::new();
  let resolved = resolved.as_object().unwrap_or(&empty);
  let mut options = Map::new();
  for key in PASSTHROUGH_OPTIONS {
    if let Some(value) = resolved.get(*key) {
      options.insert(key.to_string(), value.clone());
    }
  }

  // Stock TypeScript doesn't know Ende's precompile transform; it type-checks
  // like react-jsx (the TS 6 checker does the same).
  if let Some(jsx) = resolved.get("jsx").and_then(|v| v.as_str()) {
    let jsx = if jsx == "precompile" {
      "react-jsx"
    } else {
      jsx
    };
    options.insert("jsx".into(), jsx.into());
  }

  let mut stock_libs = Vec::new();
  let mut ende_libs = Vec::new();
  for lib in resolved
    .get("lib")
    .and_then(|v| v.as_array())
    .into_iter()
    .flatten()
    .filter_map(|v| v.as_str())
  {
    match ende_lib(&lib.to_ascii_lowercase()) {
      Some(name) => {
        if !ende_libs.contains(&name) {
          ende_libs.push(name);
        }
      }
      // Unknown names go to tsc, which rejects them with a clear message.
      None => stock_libs.push(Value::from(lib)),
    }
  }
  // An empty list means "no default libs"; the Ende libs reference the stock
  // ones they build on (esnext).
  options.insert("lib".into(), Value::Array(stock_libs));

  // Ende resolves every import itself, so tsc only needs to find what Ende
  // found: `paths` for non-relative specifiers, bundler resolution for the
  // rest. NodeNext would make tsc treat files outside an ESM package as
  // CommonJS.
  let module = match resolved.get("module").and_then(|v| v.as_str()) {
    Some(m) if m.eq_ignore_ascii_case("preserve") => "preserve",
    _ => "esnext",
  };
  options.insert("module".into(), module.into());
  options.insert("moduleResolution".into(), "bundler".into());
  options.insert("allowImportingTsExtensions".into(), true.into());
  options.insert("noEmit".into(), true.into());
  options.insert("paths".into(), Value::Object(paths));
  if !root_dirs.is_empty() {
    options.insert("rootDirs".into(), json!(root_dirs));
  }

  // TS 7 changed these defaults; keep TS 6's behavior. Ende passes
  // `compilerOptions.types` to the checker as root files, not as `types`.
  options.insert("types".into(), json!([]));
  options.insert("noUncheckedSideEffectImports".into(), false.into());

  TranslatedOptions {
    compiler_options: options,
    ende_libs,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn ende_lib(name: &str) -> Option<String> {
    let rest = name
      .strip_prefix("ende.")
      .or_else(|| name.strip_prefix("deno."))?;
    ["window", "unstable", "ns"]
      .contains(&rest)
      .then(|| format!("ende.{rest}"))
  }

  #[test]
  fn translates_ende_check_defaults() {
    let resolved = json!({
      "allowJs": true,
      "allowImportingTsExtensions": true,
      "allowSyntheticDefaultImports": true,
      "checkJs": false,
      "emitDecoratorMetadata": false,
      "experimentalDecorators": false,
      "incremental": true,
      "jsx": "react",
      "importsNotUsedAsValues": "remove",
      "inlineSourceMap": true,
      "inlineSources": true,
      "isolatedModules": true,
      "lib": ["ende.window", "ende.unstable"],
      "module": "NodeNext",
      "moduleDetection": "force",
      "noEmit": true,
      "noImplicitOverride": true,
      "resolveJsonModule": true,
      "sourceMap": false,
      "skipLibCheck": false,
      "strict": true,
      "target": "esnext",
      "tsBuildInfoFile": "internal:///.tsbuildinfo",
      "useDefineForClassFields": true,
    });
    let translated = translate(&resolved, Vec::new(), Map::new(), ende_lib);
    assert_eq!(translated.ende_libs, ["ende.window", "ende.unstable"]);
    assert_eq!(
      Value::Object(translated.compiler_options),
      json!({
        "allowImportingTsExtensions": true,
        "allowJs": true,
        "allowSyntheticDefaultImports": true,
        "checkJs": false,
        "emitDecoratorMetadata": false,
        "experimentalDecorators": false,
        "isolatedModules": true,
        "jsx": "react",
        "lib": [],
        "module": "esnext",
        "moduleDetection": "force",
        "moduleResolution": "bundler",
        "noEmit": true,
        "noImplicitOverride": true,
        "noUncheckedSideEffectImports": false,
        "paths": {},
        "resolveJsonModule": true,
        "skipLibCheck": false,
        "strict": true,
        "target": "esnext",
        "types": [],
        "useDefineForClassFields": true,
      })
    );
  }

  #[test]
  fn splits_stock_and_ende_libs() {
    let resolved = json!({
      "lib": ["Ende.Window", "dom", "deno.ns", "ende.window", "ende.nope"],
    });
    let translated = translate(&resolved, Vec::new(), Map::new(), ende_lib);
    assert_eq!(translated.ende_libs, ["ende.window", "ende.ns"]);
    assert_eq!(
      translated.compiler_options["lib"],
      json!(["dom", "ende.nope"])
    );
  }

  #[test]
  fn maps_jsx_module_and_root_dirs() {
    let resolved = json!({ "jsx": "precompile", "module": "Preserve" });
    let paths = json!({ "jsr:@std/x": ["/cache/remote/x.ts"] });
    let translated = translate(
      &resolved,
      vec!["C:/project/src".into()],
      paths.as_object().unwrap().clone(),
      ende_lib,
    );
    let options = translated.compiler_options;
    assert_eq!(options["jsx"], json!("react-jsx"));
    assert_eq!(options["module"], json!("preserve"));
    assert_eq!(options["rootDirs"], json!(["C:/project/src"]));
    assert_eq!(options["paths"], paths);
  }

  #[test]
  fn removed_option_names_the_option_and_file() {
    let config = Url::parse("file:///project/ende.json").unwrap();
    let options = CompilerOptions(json!({ "baseUrl": "." }));
    let removed = removed_options([(&config, Some(&options))].into_iter());
    assert_eq!(removed.len(), 1);
    assert_eq!(
      removed[0].to_string(),
      "compilerOptions.baseUrl has no effect in TypeScript 7, which removed \
       it, so tsc doesn't get it. Ende still resolves \"paths\" and \
       \"rootDirs\" against it.\n    at file:///project/ende.json"
    );
    let options = CompilerOptions(json!({ "strict": false }));
    assert!(
      removed_options([(&config, Some(&options))].into_iter()).is_empty()
    );
  }
}
