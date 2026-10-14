#!/usr/bin/env -S deno run -A --lock=tools/deno.lock.json
// Copyright 2018-2026 the Deno authors. MIT license.
import { EndeWorkspace } from "./ende_workspace.ts";
import { $, createOctoKit, getGitHubRepository } from "./deps.ts";
import { readProductVersion } from "./chronver.ts";

const octoKit = createOctoKit();
const workspace = await EndeWorkspace.load();
const repo = workspace.repo;
const version = readProductVersion();

const originalBranch = await repo.gitCurrentBranch();
const newBranchName = `release_${version.replaceAll(".", "_")}`;

// Create and push branch
$.logStep(`Creating branch ${newBranchName}...`);
await repo.gitBranch(newBranchName);
await repo.gitAdd();
await repo.gitCommit(version);
$.logStep("Pushing branch...");
await repo.gitPush("-u", "origin", "HEAD");

// Open PR
$.logStep("Opening PR...");
const openedPr = await octoKit.request("POST /repos/{owner}/{repo}/pulls", {
  ...getGitHubRepository(),
  base: originalBranch,
  head: newBranchName,
  draft: true,
  title: version,
  body: getPrBody(),
});
$.log(`Opened PR at ${openedPr.data.url}`);

function getPrBody() {
  let text = `Set the Ende version to ${version}\n\n` +
    `Please ensure:\n` +
    `- [ ] cli/lib/version.txt holds the right version\n` +
    `- [ ] Releases.md is updated correctly (think relevancy and remove reverts)\n\n` +
    `To make edits to this PR:\n` +
    "```shell\n" +
    `git fetch upstream ${newBranchName} && git checkout -b ${newBranchName} upstream/${newBranchName}\n` +
    "```\n";

  const actor = Deno.env.get("GH_WORKFLOW_ACTOR");
  if (actor != null) {
    text += `\ncc @${actor}`;
  }

  return text;
}
