#!/usr/bin/env -S deno run -RW

// Copyright 2018-2026 the Deno authors. MIT license.
// deno-lint-ignore-file no-console

/**
 * This script generates the `import_map.json` file based on `tests/util/std/*` packages.
 *
 * Run this script when you updated the git submodule at `tests/util/std`.
 */

import endeConfig from "../tests/util/std/ende.json" with { type: "json" };
import { join } from "jsr:@ende/path@^2026.10.15/posix/join";

const rootImportMap: { imports: Record<string, string> } = { imports: {} };
const coreImportMap: { imports: Record<string, string> } = JSON.parse(
  Deno.readTextFileSync("./tools/core_import_map.json"),
);
// Drop entries from a previous run so removed or renamed packages go away.
for (const [specifier, path] of Object.entries(coreImportMap.imports)) {
  if (path.startsWith("../tests/util/std/")) {
    delete coreImportMap.imports[specifier];
  }
}
for (const workspace of endeConfig.workspace) {
  const { default: pkgEndeConfig } = await import(
    `../tests/util/std/${workspace}/ende.json`,
    {
      with: { type: "json" },
    }
  );

  for (
    const [moduleName, modulePath] of Object.entries(
      pkgEndeConfig.exports as Record<string, string>,
    )
  ) {
    rootImportMap.imports[join(pkgEndeConfig.name, moduleName)] =
      `./tests/util/std/${join(workspace, modulePath)}`;
    coreImportMap.imports[join(pkgEndeConfig.name, moduleName)] =
      `../tests/util/std/${join(workspace, modulePath)}`;
  }
}

rootImportMap.imports = sortObjectByKey(rootImportMap.imports);
coreImportMap.imports = sortObjectByKey(coreImportMap.imports);

console.log("Writing to ./import_map.json");
await Deno.writeTextFile(
  "./import_map.json",
  JSON.stringify(rootImportMap, null, 2) + "\n",
);
console.log("Writing to ./tools/core_import_map.json");
await Deno.writeTextFile(
  "./tools/core_import_map.json",
  JSON.stringify(coreImportMap, null, 2) + "\n",
);

function sortObjectByKey<T>(obj: Record<string, T>): Record<string, T> {
  return Object.fromEntries(
    Object.entries(obj).sort(([a], [b]) => a.localeCompare(b)),
  );
}
