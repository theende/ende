#!/usr/bin/env -S deno run -A --lock=tools/deno.lock.json
// Copyright 2018-2026 the Deno authors. MIT license.
import { EndeWorkspace } from "./ende_workspace.ts";
import { $ } from "./deps.ts";
import {
  nextReleaseVersion,
  readProductVersion,
  writeProductVersion,
} from "./chronver.ts";

// Sets Ende's product version (`cli/lib/version.txt`) for a new release. The
// version is today's UTC date in ChronVer form, `YYYY.MM.DD`, with `.N`
// appended when a release (tag `v<version>`) already exists for today.
//
// Crate versions in Cargo.toml are not bumped: they are internal and
// independent of the product version.

const workspace = await EndeWorkspace.load();
const repo = workspace.repo;
const originalVersion = readProductVersion();

await bumpCiCacheVersion();

await repo.gitFetchTags("origin");
const tags = await repo.getGitTags();
const newVersion = nextReleaseVersion(new Date(), tags);
$.logStep(`Setting the Ende version to ${newVersion}...`);
writeProductVersion(newVersion);

await assertEndeBinaryVersion(newVersion);

// try to update the Releases.md markdown text
try {
  $.logStep("Updating Releases.md...");
  await updateReleasesMd();
} catch (err) {
  $.log(err);
  $.logError(
    "Error Updating Releases.md failed. Please manually run " +
      "`git log --oneline VERSION_FROM..VERSION_TO` and " +
      "use the output to update Releases.md",
  );
}

async function updateReleasesMd() {
  const gitLog = await getGitLog();
  const releasesMdFile = workspace.getReleasesMdFile();
  releasesMdFile.updateWithGitLog({
    version: newVersion,
    gitLog,
  });

  await workspace.runFormatter();
}

async function getGitLog() {
  // make the repo unshallow so the history since the last release is there
  if (await repo.gitIsShallow()) {
    await repo.gitFetchUnshallow("origin");
  }

  return await repo.getGitLogFromTags(
    "origin",
    `v${originalVersion}`,
    undefined,
  );
}

async function bumpCiCacheVersion() {
  const generateScript = workspace.repo.folderPath.join(
    ".github/workflows/ci.ts",
  );
  const fileText = generateScript.readTextSync();
  const cacheVersionRegex = /const cacheVersion = ([0-9]+);/;
  const version = fileText.match(cacheVersionRegex)?.[1];
  if (version == null) {
    throw new Error("Could not find cache version in text.");
  }
  const toVersion = parseInt(version, 10) + 1;
  $.logStep(`Bumping cache version from ${version} to ${toVersion}...`);
  const newText = fileText.replace(
    cacheVersionRegex,
    `const cacheVersion = ${toVersion};`,
  );
  generateScript.writeTextSync(newText);

  // run the script
  await $`${generateScript}`;
}

async function assertEndeBinaryVersion(expectedVersion: string) {
  $.logStep("Verifying Ende binary version.");
  const text = (await $`cargo run -p ende -- -v`.text()).replace("ende ", "");
  $.logLight("Version:", text);
  if (text.trim() !== expectedVersion) {
    $.logError("Error: Expected", expectedVersion, "but found", text);
    Deno.exit(1);
  }
}
