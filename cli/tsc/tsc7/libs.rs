// Copyright 2026 the Ende authors. MIT license.

//! TypeScript 7's stock libs, with Ende's additions.
//!
//! Ende's copies of TypeScript's `lib.dom.d.ts` and `lib.webworker.d.ts`
//! (cli/tsc/dts/) add a few things the stock ones lack. tsc 7 reads its stock
//! libs from its own directory, which is checksum-verified and never changed,
//! so the additions are applied to copies in the project's `types/`, and
//! `libReplacement` makes tsc use those: tsc looks up `@typescript/lib-<name>`
//! for every lib it loads, however it was named (`lib`, a
//! `/// <reference lib="..." />`, or another lib's reference), and `paths`
//! answers. Every stock lib gets an exact `paths` entry, so tsc never searches
//! `node_modules` directories for one.
//!
//! The copies start from the stock files of the tsc that runs, so they follow
//! TypeScript 7 rather than Ende's TS 6-era files, which conflict with TS 7's
//! when both are loaded.

use std::path::Path;

use ende_core::anyhow::Context;
use ende_core::error::AnyError;
use ende_core::serde_json::Map;
use ende_core::serde_json::Value;

use super::project::to_tsc_path;

/// One addition: `find` in the stock text becomes `replace`.
struct Addition {
  find: &'static str,
  replace: &'static str,
}

const BROTLI: Addition = Addition {
  find: r#"type CompressionFormat = "deflate" | "deflate-raw" | "gzip";"#,
  replace: r#"type CompressionFormat = "deflate" | "deflate-raw" | "gzip" | "brotli";"#,
};

const READABLE_STREAM_FROM: Addition = Addition {
  find: "    new<R = any>(underlyingSource?: UnderlyingSource<R>, strategy?: \
         QueuingStrategy<R>): ReadableStream<R>;\n};",
  replace: "    new<R = any>(underlyingSource?: UnderlyingSource<R>, strategy?: \
            QueuingStrategy<R>): ReadableStream<R>;\n    \
            from<R>(asyncIterable: AsyncIterable<R> | Iterable<R | \
            PromiseLike<R>>): ReadableStream<R>;\n};",
};

/// lib.dom.extras.d.ts has `Error.captureStackTrace` and
/// `Error.stackTraceLimit`. tsc doesn't know the lib name, so it's referenced
/// by path.
const DOM_EXTRAS: Addition = Addition {
  find: "/// <reference lib=\"es2018.asynciterable\" />\n",
  replace: "/// <reference lib=\"es2018.asynciterable\" />\n\
            /// <reference path=\"./lib.dom.extras.d.ts\" />\n",
};

/// The stock libs Ende adds to, and what it adds. Keep in sync with the
/// differences between cli/tsc/dts/ and TypeScript's own files (a test
/// checks that Ende's files have every addition).
const ADDITIONS: &[(&str, &[Addition])] = &[
  ("lib.dom.d.ts", &[DOM_EXTRAS, READABLE_STREAM_FROM, BROTLI]),
  ("lib.webworker.d.ts", &[READABLE_STREAM_FROM, BROTLI]),
];

/// Files the additions need next to the copies.
const EXTRA_FILES: &[&str] = &["lib.dom.extras.d.ts"];

/// Writes the copies of the stock libs in `tsc_dir` with Ende's additions to
/// `types_dir`, and returns `paths` entries for every stock lib, pointing at
/// the copy if there is one. Empty if `tsc_dir` has no lib files (a tsc with
/// its libs built in): Ende's additions are then missing.
pub fn write_replacements(
  tsc_dir: &Path,
  types_dir: &Path,
  write: impl Fn(&Path, &[u8]) -> Result<(), AnyError>,
) -> Result<Map<String, Value>, AnyError> {
  let entries = std::fs::read_dir(tsc_dir)
    .with_context(|| format!("failed to read {}", tsc_dir.display()))?;
  let mut paths = Map::new();
  for entry in entries {
    let entry =
      entry.with_context(|| format!("failed to read {}", tsc_dir.display()))?;
    let file_name = entry.file_name();
    let Some(file_name) = file_name.to_str() else {
      continue;
    };
    let Some(library) = library_name(file_name) else {
      continue;
    };
    let stock = entry.path();
    let path = match ADDITIONS.iter().find(|(name, _)| *name == file_name) {
      Some((_, additions)) => {
        let text = std::fs::read_to_string(&stock)
          .with_context(|| format!("failed to read {}", stock.display()))?;
        let copy = types_dir.join(file_name);
        write(&copy, apply(file_name, &text, additions).as_bytes())?;
        copy
      }
      None => stock,
    };
    paths.insert(library, Value::from(vec![to_tsc_path(&path)]));
  }
  if !paths.is_empty() {
    for name in EXTRA_FILES {
      let text = crate::tsc::get_lazily_loaded_asset(name)
        .with_context(|| format!("{name} isn't in this build"))?;
      write(&types_dir.join(name), text.as_bytes())?;
    }
  }
  Ok(paths)
}

/// Applies `additions` to the text of the stock lib `name`. One whose text is
/// already there (a later TypeScript has it) or isn't found is skipped; the
/// specs in tests/specs/check/tsc7 catch a missing one.
fn apply(name: &str, text: &str, additions: &[Addition]) -> String {
  let mut text = text.to_string();
  for addition in additions {
    if text.contains(addition.replace) {
      continue;
    }
    if text.contains(addition.find) {
      text = text.replacen(addition.find, addition.replace, 1);
    } else {
      log::debug!("tsc7: no place for an addition in {name}");
    }
  }
  text
}

/// The module name tsc looks up to replace the lib file `file_name`
/// (`getLibraryNameFromLibFileName` in TypeScript): `lib.dom.d.ts` is
/// `@typescript/lib-dom`, `lib.dom.iterable.d.ts` `@typescript/lib-dom/
/// iterable`, `lib.es2015.symbol.wellknown.d.ts` `@typescript/lib-es2015/
/// symbol-wellknown`. `None` for other files, and for `lib.d.ts`, which Ende
/// never loads (it always sets `lib`).
fn library_name(file_name: &str) -> Option<String> {
  let rest = file_name.strip_prefix("lib.")?.strip_suffix(".d.ts")?;
  let mut components = rest.split('.');
  let mut path = components.next().filter(|c| !c.is_empty())?.to_string();
  for (i, component) in components.enumerate() {
    path.push(if i == 0 { '/' } else { '-' });
    path.push_str(component);
  }
  Some(format!("@typescript/lib-{path}"))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn library_names() {
    assert_eq!(library_name("lib.dom.d.ts").unwrap(), "@typescript/lib-dom");
    assert_eq!(
      library_name("lib.dom.iterable.d.ts").unwrap(),
      "@typescript/lib-dom/iterable"
    );
    assert_eq!(
      library_name("lib.es2015.symbol.wellknown.d.ts").unwrap(),
      "@typescript/lib-es2015/symbol-wellknown"
    );
    assert_eq!(library_name("lib.d.ts"), None);
    assert_eq!(library_name("tsc"), None);
    assert_eq!(library_name("NOTICE.txt"), None);
  }

  /// Ende's own copies of the libs (from TypeScript 6, whose stock dom and
  /// webworker libs are the same as TypeScript 7.0's) have every addition,
  /// and undoing them all leaves none of Ende's changes behind.
  #[test]
  fn additions_match_endes_libs() {
    for (name, additions) in ADDITIONS {
      // TS 6 knows the lib name; the copies reference the file instead.
      let ende = crate::tsc::get_lazily_loaded_asset(name).unwrap().replace(
        "/// <reference lib=\"dom.extras\" />",
        "/// <reference path=\"./lib.dom.extras.d.ts\" />",
      );
      let ende = ende.as_str();
      let mut stock = ende.to_string();
      for addition in additions.iter() {
        assert_eq!(
          ende.matches(addition.replace).count(),
          1,
          "{name}: {}",
          addition.replace
        );
        stock = stock.replacen(addition.replace, addition.find, 1);
      }
      assert_eq!(apply(name, &stock, additions), ende, "{name}");
      assert!(!stock.contains("brotli"), "{name}");
      assert!(!stock.contains("dom.extras"), "{name}");
      assert!(!stock.contains("    from<R>(asyncIterable"), "{name}");
      // Applying twice changes nothing.
      assert_eq!(apply(name, ende, additions), ende, "{name}");
    }
  }

  #[test]
  fn writes_copies_and_paths() {
    let dir = tempfile::tempdir().unwrap();
    let tsc_dir = dir.path().join("tsc");
    let types_dir = dir.path().join("types");
    std::fs::create_dir_all(&tsc_dir).unwrap();
    std::fs::create_dir_all(&types_dir).unwrap();
    std::fs::write(
      tsc_dir.join("lib.dom.d.ts"),
      format!("{}\n{}\n", DOM_EXTRAS.find, BROTLI.find),
    )
    .unwrap();
    std::fs::write(tsc_dir.join("lib.es5.d.ts"), "").unwrap();
    std::fs::write(tsc_dir.join("tsc"), "").unwrap();
    let paths = write_replacements(&tsc_dir, &types_dir, |path, bytes| {
      std::fs::write(path, bytes).map_err(Into::into)
    })
    .unwrap();
    assert_eq!(
      paths,
      ende_core::serde_json::json!({
        "@typescript/lib-dom": [to_tsc_path(&types_dir.join("lib.dom.d.ts"))],
        "@typescript/lib-es5": [to_tsc_path(&tsc_dir.join("lib.es5.d.ts"))],
      })
      .as_object()
      .unwrap()
      .clone()
    );
    let copy = std::fs::read_to_string(types_dir.join("lib.dom.d.ts")).unwrap();
    assert!(copy.contains("\"brotli\""));
    assert!(copy.contains("./lib.dom.extras.d.ts"));
    assert!(types_dir.join("lib.dom.extras.d.ts").is_file());

    // No lib files: nothing to replace.
    let empty = dir.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let paths =
      write_replacements(&empty, &types_dir, |_, _| unreachable!()).unwrap();
    assert!(paths.is_empty());
  }
}
