#!/usr/bin/env -S deno run -A --lock=tools/deno.lock.json
// Copyright 2018-2026 the Deno authors. MIT license.
// deno-lint-ignore-file no-console

// Writes the given release tag into the latest-version file and prints the
// file name to stdout. Exits with code 1 if the version is not greater than
// the current published latest (to skip the upload).

import { isNewerRelease } from "./chronver.ts";

const version = Deno.args[0];
if (!version) {
  console.error("Usage: upload_version_file.ts <version-tag>");
  Deno.exit(1);
}

const latestFile = "release-latest.txt";

console.error("Version:", version);
console.error("Latest file:", latestFile);

Deno.writeTextFileSync(latestFile, version + "\n");

// Check that this version is actually newer
const response = await fetch("https://dl.deno.land/release-latest.txt");
if (!response.ok) {
  throw new Error(`Failed to fetch: ${response.statusText}`);
}
const latestVersionText = (await response.text()).trim();
console.error("Currently published latest:", latestVersionText);

if (!isNewerRelease(version, latestVersionText)) {
  console.error(
    "Skipping upload because this version is not greater than the latest.",
  );
  Deno.exit(1);
}

// Print the file name so the caller knows what to upload
console.log(latestFile);
