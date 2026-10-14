#!/usr/bin/env -S deno run -A --lock=tools/deno.lock.json
// Copyright 2018-2026 the Deno authors. MIT license.
import { EndeWorkspace } from "./ende_workspace.ts";
import { $, createOctoKit, getGitHubRepository } from "./deps.ts";
import { readProductVersion } from "./chronver.ts";

const workspace = await EndeWorkspace.load();
const repo = workspace.repo;
const version = readProductVersion();

$.logStep("Creating release tag...");
await createReleaseTag();

$.logStep("Forwarding release commit to primary...");
try {
  await forwardReleaseCommitToMain();
} catch (err) {
  $.logError("Failed. Please manually open a PR.", err);
}

async function createReleaseTag() {
  await repo.gitFetchTags("origin");
  const tags = await repo.getGitTags();
  const tagName = `v${version}`;

  if (tags.has(tagName)) {
    $.log(`Tag ${tagName} already exists.`);
  } else {
    await repo.gitTag(tagName);
    await repo.gitPush("origin", tagName);
  }
}

async function forwardReleaseCommitToMain() {
  // if this is a patch release, open a PR to forward the most recent commit back to primary
  const currentBranch = await repo.gitCurrentBranch();
  const isPatchRelease = currentBranch !== "primary";

  if (!isPatchRelease) {
    $.log("Not doing a patch release. Skipping.");
    return;
  }

  await repo.command("git fetch origin primary");
  const releaseCommitHash = await repo.command("git rev-parse HEAD").text();
  const newBranchName = `forward_v${version}`;
  $.logStep(`Creating branch ${newBranchName}...`);
  await repo.command([
    "git",
    "checkout",
    "-b",
    newBranchName,
    "origin/primary",
  ]);
  const cherryPickResult = await repo.command([
    "git",
    "cherry-pick",
    releaseCommitHash,
  ]).noThrow();
  if (cherryPickResult.code !== 0) {
    // commit with conflicts that can be resolved in the PR
    await repo.command("git add .");
    await repo.command(
      'git commit --no-verify -m "Cherry-pick version bump commit with conflicts"',
    ).noThrow();
  }
  await repo.gitPush("origin", newBranchName);

  $.logStep(`Opening PR...`);

  try {
    const openedPr = await createOctoKit().request(
      "POST /repos/{owner}/{repo}/pulls",
      {
        ...getGitHubRepository(),
        base: "primary",
        head: newBranchName,
        draft: true,
        title: `chore: forward v${version} release commit to primary`,
        body: getPrBody(),
      },
    );
    $.log(`Opened PR at ${openedPr.data.url}`);
  } catch (err) {
    $.logError(
      `Failed to open PR. Please open one manually: https://github.com/theende/ende/pull/new/${newBranchName}`,
      err,
    );
  }

  function getPrBody() {
    let text = "";

    if (cherryPickResult.code !== 0) {
      text += `**THIS PR HAS GIT CONFLICTS THAT MUST BE RESOLVED**\n\n`;
    }

    text +=
      `This is the release commit being forwarded back to primary for ${version}\n\n` +
      `Please ensure:\n` +
      `- [ ] Everything looks ok in the PR\n` +
      `- [ ] The release has been published\n\n` +
      `To make edits to this PR:\n` +
      "```shell\n" +
      `git fetch upstream ${newBranchName} && git checkout -b ${newBranchName} upstream/${newBranchName}\n` +
      "```\n\n" +
      "Don't need this PR? Close it.\n";

    const actor = Deno.env.get("GH_WORKFLOW_ACTOR");
    if (actor != null) {
      text += `\ncc @${actor}`;
    }

    return text;
  }
}
