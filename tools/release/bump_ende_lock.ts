#!/usr/bin/env -S ende run --allow-read=tools/ende_version.lock --allow-write=tools/ende_version.lock --allow-env=GITHUB_TOKEN,GITHUB_REPOSITORY,GH_WORKFLOW_ACTOR --allow-net=dl.ende.sh,github.com,release-assets.githubusercontent.com,objects.githubusercontent.com,api.github.com --allow-run=git
// Copyright 2026 the Ende authors. MIT license.
// deno-lint-ignore-file no-console

// Opens a PR that pins CI's tooling Ende (tools/ende_version.lock) to a new
// release. The post_publish workflow runs it, from a checkout of primary,
// after the release is published. The PR's own CI run installs the new pin,
// so the new Ende is tested on CI's tooling before anyone merges it.
//
// For each target in the lock, the zip on dl.ende.sh must match both the
// GitHub release's .zip.sha256sum and dl.ende.sh's own, and both must name
// the zip. Any mismatch or missing file, or a version not newer than the
// pin, fails before anything is written. A version that is already pinned
// exits 0 without a PR.
//
// Then it rewrites only the `# version:` line and the hashes, and commits
// the lock on a new branch, pushes it to origin and opens the PR with
// GITHUB_TOKEN (a PAT, so the PR's CI runs).
//
// Usage: bump_ende_lock.ts <version>    (e.g. v2026.11.10 or 2026.11.10)

import { compareChronVer, parseChronVer } from "./chronver.ts";
import { createGitHubClient, getGitHubRepository } from "./github.ts";

const LOCK_PATH = "tools/ende_version.lock";
const VERSION_RE = /^(#\s*version:\s*)(\S*)(.*)$/;
const TARGET_RE = /^(\S+)(\s+)([0-9a-f]{64})$/;

function fail(message: string): never {
  console.error(`bump_ende_lock.ts: ${message}`);
  Deno.exit(1);
}

const arg = Deno.args[0];
if (arg == null || Deno.args.length !== 1) {
  fail("Usage: bump_ende_lock.ts <version>");
}
const newVersion = parseChronVer(arg);
if (newVersion == null) {
  fail(`Not a release version: '${arg}' (expected vYYYY.MM.DD[.N])`);
}
const version = arg.replace(/^v/, "");
const tag = `v${version}`;

const lockText = Deno.readTextFileSync(LOCK_PATH);
const lockLines = lockText.split("\n");
const versionLines = lockLines.flatMap((line, i) =>
  VERSION_RE.test(line) ? [i] : []
);
if (versionLines.length !== 1) {
  fail(`${LOCK_PATH} must have exactly one '# version:' line`);
}
const versionLine = versionLines[0];
const pinned = VERSION_RE.exec(lockLines[versionLine])![2];
const pinnedVersion = parseChronVer(pinned);
if (pinnedVersion == null) {
  fail(`${LOCK_PATH} pins '${pinned}', which is not a release version`);
}
const order = compareChronVer(newVersion, pinnedVersion);
if (order === 0) {
  console.error(`${LOCK_PATH} already pins Ende ${version}. Nothing to do.`);
  Deno.exit(0);
}
if (order < 0) {
  fail(`${tag} is older than the Ende ${pinned} that ${LOCK_PATH} pins`);
}

const targetLines = new Map<string, number>();
lockLines.forEach((line, i) => {
  if (line === "" || line.startsWith("#")) {
    return;
  }
  const match = TARGET_RE.exec(line);
  if (match == null) {
    fail(`${LOCK_PATH} line ${i + 1} is not '<target>  <sha256>': ${line}`);
  }
  if (targetLines.has(match[1])) {
    fail(`${LOCK_PATH} pins ${match[1]} twice`);
  }
  targetLines.set(match[1], i);
});
if (targetLines.size === 0) {
  fail(`${LOCK_PATH} pins no targets`);
}

const { owner, repo } = getGitHubRepository();
const branch = `ende_lock_${tag}`;
const remoteBranch = await git("ls-remote", "--heads", "origin", branch);
if (remoteBranch !== "") {
  fail(
    `origin already has a ${branch} branch. If it has no PR yet, open one: ` +
      `https://github.com/${owner}/${repo}/pull/new/${branch}`,
  );
}

const newLines = [...lockLines];
newLines[versionLine] = lockLines[versionLine].replace(
  VERSION_RE,
  (_, prefix, _old, rest) => `${prefix}${version}${rest}`,
);
for (const [target, i] of targetLines) {
  const hash = await checkedZipSha256(target);
  newLines[i] = lockLines[i].replace(
    TARGET_RE,
    (_, name, space) => `${name}${space}${hash}`,
  );
}

// Everything is checked; only now write and push.
await git("checkout", "-b", branch);
Deno.writeTextFileSync(LOCK_PATH, newLines.join("\n"));
const title = `chore: pin CI's Ende to ${tag}`;
await git("commit", "-m", title, "--", LOCK_PATH);
await git("push", "origin", branch);

console.error("Opening PR...");
const pr = await createGitHubClient().post(`/repos/${owner}/${repo}/pulls`, {
  base: "primary",
  head: branch,
  // CI skips most jobs on draft PRs, and this PR exists for its CI run.
  draft: false,
  title,
  body: prBody(),
});
console.error(`Opened PR at ${pr.html_url}`);

/**
 * Downloads ende-<target>.zip from dl.ende.sh and returns its SHA-256, after
 * checking it against the GitHub release's and dl.ende.sh's .zip.sha256sum.
 */
async function checkedZipSha256(target: string): Promise<string> {
  const zip = `ende-${target}.zip`;
  const dlUrl = `https://dl.ende.sh/release/${tag}/${zip}`;
  const ghUrl =
    `https://github.com/${owner}/${repo}/releases/download/${tag}/${zip}`;
  const missingDl = "Did the release workflow's dl_ende_sh job upload it?";
  const fromGitHub = parseSha256sum(
    await download(`${ghUrl}.sha256sum`, ""),
    zip,
    `${ghUrl}.sha256sum`,
  );
  const fromDl = parseSha256sum(
    await download(`${dlUrl}.sha256sum`, missingDl),
    zip,
    `${dlUrl}.sha256sum`,
  );
  console.error(`Downloading ${dlUrl}`);
  const actual = await sha256(await download(dlUrl, missingDl));
  if (fromDl !== fromGitHub) {
    fail(
      `${dlUrl}.sha256sum says ${fromDl}, but ${ghUrl}.sha256sum says ` +
        `${fromGitHub}`,
    );
  }
  if (actual !== fromGitHub) {
    fail(
      `${dlUrl} has SHA-256 ${actual}, but ${ghUrl}.sha256sum says ` +
        `${fromGitHub}`,
    );
  }
  console.error(`${zip}: ${actual}`);
  return actual;
}

async function download(url: string, hint: string): Promise<ArrayBuffer> {
  let response: Response;
  try {
    response = await fetch(url);
  } catch (err) {
    fail(`Failed to download ${url}: ${err}`);
  }
  if (!response.ok) {
    await response.body?.cancel();
    fail(
      `Failed to download ${url}: ${response.status} ${response.statusText}` +
        (hint === "" ? "" : `. ${hint}`),
    );
  }
  return await response.arrayBuffer();
}

/** The hash in a `<sha256>  <file name>` line that must name `zip`. */
function parseSha256sum(bytes: ArrayBuffer, zip: string, url: string): string {
  const fields = new TextDecoder().decode(bytes).trim().split(/\s+/);
  if (fields.length !== 2 || !/^[0-9a-f]{64}$/.test(fields[0])) {
    fail(`${url} is not '<sha256>  ${zip}'`);
  }
  if (fields[1] !== zip) {
    fail(`${url} names '${fields[1]}', not ${zip}`);
  }
  return fields[0];
}

async function sha256(bytes: ArrayBuffer): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

/** Runs git in the current directory and returns its trimmed stdout. */
async function git(...args: string[]): Promise<string> {
  const output = await new Deno.Command("git", { args, stdin: "null" })
    .output();
  const decoder = new TextDecoder();
  if (!output.success) {
    fail(
      `git ${args.join(" ")} failed (exit ${output.code}):\n` +
        decoder.decode(output.stderr),
    );
  }
  return decoder.decode(output.stdout).trim();
}

function prBody() {
  let text = `Pins the Ende that CI's tooling jobs install ` +
    `(\`tools/ende_version.lock\`) to the published ${tag}.\n\n` +
    `This PR's CI run installs ${tag} with \`tools/release/fetch_ende.sh\`, ` +
    `so it tests the new Ende on CI's tooling. Merge it once CI is green.\n\n` +
    `The version line and each target's zip SHA-256 changed. Every zip on ` +
    `dl.ende.sh matched both its GitHub release \`.zip.sha256sum\` and ` +
    `dl.ende.sh's own.\n\n` +
    "Opened by `tools/release/bump_ende_lock.ts` in the post_publish " +
    "workflow. Don't need this PR? Close it.\n";

  const actor = Deno.env.get("GH_WORKFLOW_ACTOR");
  if (actor != null) {
    text += `\ncc @${actor}`;
  }

  return text;
}
