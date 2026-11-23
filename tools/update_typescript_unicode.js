#!/usr/bin/env -S ende run --allow-read --allow-write --allow-net=www.unicode.org
// Copyright 2026 the Ende authors. MIT license.
// deno-lint-ignore-file no-console

// Brings the Unicode data in the bundled TypeScript 6 compiler
// (cli/tsc/00_typescript.js) up to the engine this script runs on, so that
// `ende check` accepts the identifiers and regular expression script names
// `ende run` accepts. TypeScript ships its own tables, which trail the
// engine's Unicode version.
//
// It rewrites three tables:
// - `unicodeESNextIdentifierStart` and `unicodeESNextIdentifierPart`, from the
//   engine's ID_Start and ID_Continue, the way TypeScript's own
//   scripts/regenerate-unicode-identifier-parts.mjs does;
// - the `Script` names (also used for `Script_Extensions`), from the `sc`
//   lines of Unicode's PropertyValueAliases.txt, keeping the names the engine
//   accepts in `\p{Script=...}`.
//
// Run it with the `ende` whose engine the tables should match, after copying a
// new typescript.js and after an engine upgrade that changes its Unicode
// version. The argument is the path or URL of PropertyValueAliases.txt for
// that Unicode version:
//
//   ./target/debug/ende run --allow-read --allow-write \
//     --allow-net=www.unicode.org tools/update_typescript_unicode.js \
//     https://www.unicode.org/Public/17.0.0/ucd/PropertyValueAliases.txt
//
// The identifier tables only ever grow: it fails without writing if a code
// point would be removed. The script names follow the engine both ways, so a
// name TypeScript lists and the engine rejects (Katakana_Or_Hiragana) is
// removed; it prints the names it adds and removes.

const TYPESCRIPT_PATH = new URL("../cli/tsc/00_typescript.js", import.meta.url);
const MAX_CODE_POINT = 0x10FFFF;

const aliasesLocation = Deno.args[0];
if (Deno.args.length !== 1) {
  console.error(
    "Usage: update_typescript_unicode.js <path or URL of PropertyValueAliases.txt>",
  );
  Deno.exit(1);
}

/** Code points matching `regexp`, as a flat list of inclusive [start, end] pairs. */
function ranges(regexp) {
  const result = [];
  let start = -1;
  for (let cp = 0; cp <= MAX_CODE_POINT + 1; cp++) {
    const matches = cp <= MAX_CODE_POINT &&
      regexp.test(String.fromCodePoint(cp));
    if (matches && start === -1) {
      start = cp;
    } else if (!matches && start !== -1) {
      result.push(start, cp - 1);
      start = -1;
    }
  }
  return result;
}

function inRanges(table, cp) {
  for (let i = 0; i < table.length; i += 2) {
    if (cp >= table[i] && cp <= table[i + 1]) return true;
  }
  return false;
}

/** Code points in `oldTable` that `newTable` lacks. */
function removedCodePoints(oldTable, newTable) {
  const removed = [];
  for (let i = 0; i < oldTable.length; i += 2) {
    for (let cp = oldTable[i]; cp <= oldTable[i + 1]; cp++) {
      if (!inRanges(newTable, cp)) removed.push(cp);
    }
  }
  return removed;
}

function engineHasScript(name) {
  try {
    new RegExp(`\\p{Script=${name}}`, "u");
    return true;
  } catch {
    return false;
  }
}

/** Script names in file order: short alias, long name, then other aliases. */
function scriptNames(aliasesText) {
  const names = [];
  for (const line of aliasesText.split("\n")) {
    const fields = line.split("#")[0].split(";").map((field) => field.trim());
    if (fields[0] !== "sc") continue;
    for (const name of fields.slice(1)) {
      if (!names.includes(name)) names.push(name);
    }
  }
  return names;
}

/** Replaces the one line `pattern` matches; `pattern` captures the old value. */
function replaceTable(source, name, pattern, render, check) {
  const matches = [...source.matchAll(pattern)];
  if (matches.length !== 1) {
    throw new Error(
      `expected exactly one ${name} in 00_typescript.js, found ${matches.length}`,
    );
  }
  const [line, prefix, oldValue, suffix] = matches[0];
  const replacement = prefix + render(check(JSON.parse(`[${oldValue}]`))) +
    suffix;
  return source.replace(line, () => replacement);
}

function identifierTable(source, name, table) {
  return replaceTable(
    source,
    name,
    new RegExp(`^(var ${name} = \\[)([^\\]]*)(\\];)$`, "gm"),
    (value) => value.join(", "),
    (oldTable) => {
      const removed = removedCodePoints(oldTable, table);
      if (removed.length > 0) {
        throw new Error(
          `${name} would lose ${removed.length} code points: ` +
            removed.slice(0, 20).map((cp) => "U+" + cp.toString(16)).join(" "),
        );
      }
      console.log(
        `${name}: ${oldTable.length / 2} -> ${table.length / 2} ranges`,
      );
      return table;
    },
  );
}

const aliasesText = /^https?:/.test(aliasesLocation)
  ? await (await fetch(aliasesLocation)).text()
  : await Deno.readTextFile(aliasesLocation);
const allScripts = scriptNames(aliasesText);
if (allScripts.length === 0) {
  throw new Error(`no script names found in ${aliasesLocation}`);
}

let source = await Deno.readTextFile(TYPESCRIPT_PATH);
source = identifierTable(
  source,
  "unicodeESNextIdentifierStart",
  ranges(/\p{ID_Start}/u),
);
source = identifierTable(
  source,
  "unicodeESNextIdentifierPart",
  ranges(/[\p{ID_Continue}\u200C\u200D]/u),
);
source = replaceTable(
  source,
  "Script set",
  /^( {2}Script: \/\* @__PURE__ \*\/ new Set\(\[)([^\]]*)(\]\),)$/gm,
  (value) => value.map((name) => JSON.stringify(name)).join(", "),
  (oldScripts) => {
    const scripts = allScripts.filter(engineHasScript);
    const removed = oldScripts.filter((name) => !scripts.includes(name));
    const added = scripts.filter((name) => !oldScripts.includes(name));
    console.log(
      `Script: ${oldScripts.length} -> ${scripts.length} names` +
        (added.length > 0 ? `, added ${added.join(" ")}` : "") +
        (removed.length > 0 ? `, removed ${removed.join(" ")}` : ""),
    );
    return scripts;
  },
);
await Deno.writeTextFile(TYPESCRIPT_PATH, source);
