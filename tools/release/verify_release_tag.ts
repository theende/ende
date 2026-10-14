#!/usr/bin/env -S deno run --allow-read=.
// Copyright 2026 the Ende authors. MIT license.
// deno-lint-ignore-file no-console

// Fails unless the given tag is `v` plus the ChronVer in cli/lib/version.txt
// of the checked-out commit. Used by the release workflow before building.
//
// Usage: verify_release_tag.ts <tag>

import { readProductVersion, releaseTagError } from "./chronver.ts";

const tag = Deno.args[0];
if (!tag) {
  console.error("Usage: verify_release_tag.ts <tag>");
  Deno.exit(1);
}

const error = releaseTagError(tag, readProductVersion());
if (error != null) {
  console.error(error);
  Deno.exit(1);
}
console.error(`Tag ${tag} matches cli/lib/version.txt.`);
