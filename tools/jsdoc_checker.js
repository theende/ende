#!/usr/bin/env -S deno run --allow-read --allow-env --allow-sys --config=tests/config/deno.json
// Copyright 2018-2026 the Deno authors. MIT license.
import { Node, Project, ts } from "npm:ts-morph@25.0.1";
import { join, ROOT_PATH } from "./util.js";

const libs = [
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_cache.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_console.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_url.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_web.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_fetch.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_websocket.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_webstorage.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_canvas.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_crypto.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_net.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende.ns.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende.shared_globals.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende.window.d.ts"),
  join(ROOT_PATH, "cli/tsc/dts/lib.ende_broadcast_channel.d.ts"),
];

const unstableLibs = [
  join(ROOT_PATH, "cli/tsc/dts/lib.ende.unstable.d.ts"),
];

const errors = [];

const project = new Project();
project.addSourceFilesAtPaths(libs);
const unstableFiles = project.addSourceFilesAtPaths(unstableLibs);

for (const file of project.getSourceFiles()) {
  for (
    const node of file.getDescendants().filter((descendant) =>
      Node.isExportable(descendant)
    )
  ) {
    if (
      node.getKind() === ts.SyntaxKind.ModuleDeclaration &&
      node.getName() === "Ende"
    ) {
      continue;
    }

    // `import Deno = Ende;` aliases the namespace above; an import-equals
    // declaration can't take `declare`, `export` or a @category tag.
    if (
      node.getKind() === ts.SyntaxKind.ImportEqualsDeclaration &&
      node.getName() === "Deno"
    ) {
      continue;
    }

    const parent = node.getFirstAncestorByKind(ts.SyntaxKind.ModuleDeclaration);
    const isInterfaceOrType =
      node.getKind() === ts.SyntaxKind.InterfaceDeclaration ||
      node.getKind() === ts.SyntaxKind.TypeAliasDeclaration;

    if (parent) {
      if (!node.isExported() && !isBrandVarStmt(node)) {
        errors.push(getMissingErrorPrefix(node) + "export keyword");
        continue;
      }
    } else if (!isInterfaceOrType && !node.hasDeclareKeyword()) {
      errors.push(getMissingErrorPrefix(node) + "declare keyword");
      continue;
    } else if (isInterfaceOrType && node.hasDeclareKeyword()) {
      errors.push(getErrorPrefix(node) + "has incorrect declare keyword");
      continue;
    }

    const jsDoc = node.getFirstChildIfKind(ts.SyntaxKind.JSDoc);
    if (!jsDoc) {
      errors.push(getMissingErrorPrefix(node) + "JSDoc comment");
      continue;
    }

    const tags = jsDoc.getTags();

    if (!tags.find((tag) => tag.getTagName() === "category")) {
      errors.push(getMissingErrorPrefix(node) + "JSDoc @category tag");
      continue;
    }

    if (unstableFiles.includes(file)) {
      if (!tags.find((tag) => tag.getTagName() === "experimental")) {
        errors.push(getMissingErrorPrefix(node) + "JSDoc @experimental tag");
      }
    }
  }
}

// Every unstable API names the flag that enables it, as a sentence of the form
// "Requires the `--unstable-cron` flag, or `"unstable": ["cron"]` in
// `ende.json`." This applies to each `@experimental` JSDoc whose enclosing
// declarations aren't `@experimental` themselves; members of an unstable
// namespace, class or interface are covered by their parent's sentence.
const unstableFlagSentence =
  /Requires the `--unstable-([a-z0-9-]+)` flag, or `"unstable": \["([a-z0-9-]+)"\]` in `ende\.json`\./;
const unstableFeatureNames = new Set(
  Deno.readTextFileSync(join(ROOT_PATH, "runtime/features/data.rs"))
    .matchAll(/^\s*name: "([a-z0-9-]+)",$/gm)
    .map((match) => match[1]),
);

// `@experimental` symbols that are available without any `--unstable-*` flag.
const geometryReason =
  "Geometry Interfaces are always exposed (runtime/js/98_global_scope_shared.js)";
const spawnReason =
  "always defined on the Ende namespace (runtime/js/90_ende_ns.js)";
const v8Reason = "V8 built-in, available without a flag";
const noUnstableFlag = {
  "Ende.spawn": spawnReason,
  "Ende.spawnAndWait": spawnReason,
  "Ende.spawnAndWaitSync": spawnReason,
  "Ende.lint":
    "types for `ende lint` plugins; `runPlugin` works under `ende test`",
  "WebSocketOptions.client":
    "accepted by `new WebSocket()` without a flag (ext/websocket/01_websocket.js)",
  "Intl": v8Reason,
  "RegExpConstructor": v8Reason,
  "Uint8Array": v8Reason,
  "Uint8ArrayConstructor": v8Reason,
  "DOMMatrix2DInit": geometryReason,
  "DOMMatrixInit": geometryReason,
  "DOMMatrix": geometryReason,
  "DOMMatrixReadOnly": geometryReason,
  "DOMPointInit": geometryReason,
  "DOMPoint": geometryReason,
  "DOMPointReadOnly": geometryReason,
  "DOMQuadInit": geometryReason,
  "DOMQuad": geometryReason,
  "DOMRectInit": geometryReason,
  "DOMRect": geometryReason,
  "DOMRectReadOnly": geometryReason,
};

for (const file of project.getSourceFiles()) {
  for (const jsDoc of file.getDescendantsOfKind(ts.SyntaxKind.JSDoc)) {
    if (!isExperimental(jsDoc)) {
      continue;
    }
    const node = jsDoc.getParent();
    const coveredByParent = node.getAncestors().some((ancestor) =>
      Node.isJSDocable(ancestor) && ancestor.getJsDocs().some(isExperimental)
    );
    const name = getQualifiedName(node);
    if (coveredByParent || Object.hasOwn(noUnstableFlag, name)) {
      continue;
    }

    const text = jsDoc.getText().replace(/\s*\n\s*\*\s*/g, " ");
    const match = unstableFlagSentence.exec(text);
    const line = node.getStartLineNumber();
    const prefix = `Symbol ${name} at file://${file.getFilePath()}:${line} `;
    if (!match) {
      errors.push(
        prefix +
          'is @experimental but is missing a "Requires the `--unstable-<name>` flag, ' +
          'or `"unstable": ["<name>"]` in `ende.json`." sentence (or an entry in ' +
          "noUnstableFlag in tools/jsdoc_checker.js if no flag is needed)",
      );
    } else if (match[1] !== match[2] || !unstableFeatureNames.has(match[1])) {
      errors.push(
        prefix +
          `names an unknown unstable feature "${match[1]}"/"${match[2]}" ` +
          "(see runtime/features/data.rs)",
      );
    }
  }
}

if (errors.length > 0) {
  throw new AggregateError(errors);
}

function isExperimental(jsDoc) {
  return jsDoc.getTags().some((tag) => tag.getTagName() === "experimental");
}

function getQualifiedName(node) {
  return [...node.getAncestors().reverse(), node]
    .filter((n) =>
      n === node || Node.isModuleDeclaration(n) ||
      Node.isInterfaceDeclaration(n) || Node.isClassDeclaration(n)
    )
    .map((n) => {
      if (Node.isVariableStatement(n)) return n.getDeclarations()[0].getName();
      if (Node.isConstructorDeclaration(n)) return "constructor";
      return n.getName();
    })
    .join(".");
}

function isBrandVarStmt(node) {
  // this variable statement is intentionally private
  return Node.isVariableStatement(node) &&
    node.getDeclarations()[0].getName() === "brand";
}

function getMissingErrorPrefix(node) {
  return getErrorPrefix(node) + `is missing a `;
}

function getErrorPrefix(node) {
  return `Symbol at file://${node.getSourceFile().getFilePath()}:${node.getStartLineNumber()} `;
}
