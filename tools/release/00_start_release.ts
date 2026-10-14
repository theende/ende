#!/usr/bin/env -S deno run -A --quiet --lock=tools/deno.lock.json
// Copyright 2018-2026 the Deno authors. MIT license.

// deno-lint-ignore-file no-console

import { $, createOctoKit } from "./deps.ts";
import { nextReleaseVersion, readProductVersion } from "./chronver.ts";

const currentDirPath = $.path(import.meta.dirname!);

$.logStep("Getting next version...");
const currentVersion = readProductVersion();
const nextVersion = nextReleaseVersion(new Date(), await getReleaseTags());

$.logStep("Creating gist with instructions...");
const releaseInstructions = buildEndeReleaseInstructionsDoc();
if (Deno.args.some((a) => a === "--dry-run")) {
  console.log(releaseInstructions);
} else {
  const octoKit = createOctoKit();
  const result = await octoKit.request("POST /gists", {
    description: `Ende CLI v${nextVersion} release checklist`,
    public: false,
    files: {
      [`release_${nextVersion}.md`]: {
        content: releaseInstructions,
      },
    },
  });

  $.log("==============================================");
  $.log("Created gist with instructions!");
  $.log("");
  $.log(`  ${result.data.html_url}`);
  $.log("");
  $.log("Please fork the gist and follow the checklist.");
  $.log("==============================================");
}

/** The release tags on origin, so a second release today gets `.1`. */
async function getReleaseTags() {
  const lines = await $`git ls-remote --tags --refs origin`.lines();
  return new Set(
    lines.filter((line) => line.length > 0).map((line) =>
      line.split("\t")[1].replace(/^refs\/tags\//, "")
    ),
  );
}

function buildEndeReleaseInstructionsDoc() {
  const templateText = currentDirPath
    .join("release_doc_template.md")
    .readTextSync()
    .replaceAll("$VERSION", nextVersion)
    .replaceAll("$PAST_VERSION", currentVersion);
  return `# Ende CLI ${nextVersion} Release Checklist\n\n${templateText}`;
}
