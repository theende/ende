// Copyright 2018-2026 the Deno authors. MIT license.

// The git and Releases.md helpers the release scripts use, from
// jsr:@deno/rust-automation@0.22.2's repo.ts, helpers.ts and releases_md.ts
// (MIT, the Deno authors). That package doesn't load under Ende: its GitHub
// client is npm:octokit (github.ts replaces it). Only what the scripts call
// is kept, unchanged except that a Repo no longer reads the Cargo workspace
// with `cargo metadata`, which nothing used, and getGitTags returns a Set.

import { CommandBuilder, type Path } from "jsr:@david/dax@0.43.2";

export interface GitLogLine {
  rev: string;
  message: string;
}

export class GitLogOutput {
  lines: GitLogLine[];

  constructor(lines: GitLogLine[]) {
    this.lines = lines;
  }

  formatForReleaseMarkdown(): string {
    const IGNORED_COMMIT_PREFIX = [
      "bench",
      "build",
      "chore",
      "ci",
      "cleanup",
      "docs",
      "refactor",
      "test",
    ];
    return this.lines
      .filter((l) => {
        // don't include version commits
        if (/^v?[0-9]+\.[0-9]+\.[0-9]+/.test(l.message)) {
          return false;
        }

        return !IGNORED_COMMIT_PREFIX
          .some((prefix) => l.message.startsWith(prefix)) &&
          l.message.length > 0;
      })
      .map((line) => `- ${line.message}`)
      .sort()
      .join("\n");
  }
}

export class Repo {
  constructor(public readonly folderPath: Path) {
  }

  gitCurrentBranch(): Promise<string> {
    return this.command("git rev-parse --abbrev-ref HEAD")
      .text();
  }

  async gitBranch(name: string) {
    await this.command(["git", "checkout", "-b", name]);
  }

  async gitAdd() {
    await this.command(["git", "add", "."]);
  }

  async gitTag(name: string) {
    await this.command(["git", "tag", name]);
  }

  async gitCommit(message: string) {
    await this.command(["git", "commit", "-m", message]);
  }

  async gitPush(...additionalArgs: string[]) {
    await this.command(["git", "push", ...additionalArgs]);
  }

  /** Converts the commit history to be a full clone. */
  async gitFetchUnshallow(remote: string) {
    await this.command(["git", "fetch", remote, "--unshallow"]);
  }

  /** Fetches the commit history up until a specified revision. */
  async gitFetchUntil(remote: string, revision: string) {
    await this.command([
      "git",
      "fetch",
      remote,
      `--shallow-exclude=${revision}`,
    ]);
  }

  async gitIsShallow(): Promise<boolean> {
    const output = await this.command("git rev-parse --is-shallow-repository")
      .text();
    return output === "true";
  }

  /** Fetches from the provided remote. */
  async gitFetchHistory(
    remote: string,
    revision?: string,
  ): Promise<void> {
    if (await this.gitIsShallow()) {
      // only fetch what is necessary
      if (revision != null) {
        await this.gitFetchUntil(remote, revision);
      } else {
        await this.gitFetchUnshallow(remote);
      }
    } else {
      const args = ["git", "fetch", remote, "--recurse-submodules=no"];
      if (revision != null) {
        args.push(revision);
      }
      await this.command(args);
    }
  }

  async gitFetchTags(remote: string) {
    await this.command([
      "git",
      "fetch",
      remote,
      "--tags",
      "--recurse-submodules=no",
    ]);
  }

  async getGitLogFromTags(
    remote: string,
    tagNameFrom: string | undefined,
    tagNameTo: string | undefined,
  ): Promise<GitLogOutput> {
    if (tagNameFrom == null && tagNameTo == null) {
      throw new Error(
        "You must at least supply a tag name from or tag name to.",
      );
    }

    // Ensure we have the git history up to this tag
    // For example, GitHub actions will do a shallow clone.
    try {
      await this.gitFetchHistory(remote, tagNameFrom);
    } catch (err) {
      // deno-lint-ignore no-console
      console.log(`Error fetching commit history: ${err}`);
    }

    // the output of git log is not stable, so use rev-list
    const revs = await this.command([
      "git",
      "rev-list",
      tagNameFrom == null ? tagNameTo! : `${tagNameFrom}..${tagNameTo ?? ""}`,
    ]).lines();

    const lines = await Promise.all(revs.map((rev) => {
      return this.command([
        "git",
        "log",
        "--format=%s",
        "-n",
        "1",
        rev,
      ])
        .text()
        .then((message) => ({
          rev,
          message: message,
        }));
    }));

    return new GitLogOutput(lines);
  }

  /** The names of the repo's git tags. */
  async getGitTags(): Promise<Set<string>> {
    return new Set(await this.command("git tag").lines());
  }

  command(command: string | string[]): CommandBuilder {
    return new CommandBuilder()
      .command(command)
      .cwd(this.folderPath);
  }
}

/** Helpers for dealing with a Releases.md file. */
export class ReleasesMdFile {
  #filePath: string;
  #fileText: string;

  constructor(filePath: string) {
    this.#filePath = filePath;
    this.#fileText = Deno.readTextFileSync(filePath);
  }

  updateWithGitLog({
    gitLog,
    version,
  }: {
    gitLog: GitLogOutput;
    version: string;
  }) {
    const insertText = getInsertText();
    this.#updateText(this.#fileText.replace(/^### /m, insertText + "\n\n### "));

    function getInsertText() {
      const formattedGitLog = gitLog.formatForReleaseMarkdown();
      const formattedDate = getFormattedDate(new Date());

      let text = `### ${version} / ${formattedDate}\n\n`;

      text += formattedGitLog;

      return text;

      function getFormattedDate(date: Date) {
        const formattedMonth = padTwoDigit(date.getMonth() + 1);
        const formattedDay = padTwoDigit(date.getDate());
        return `${date.getFullYear()}.${formattedMonth}.${formattedDay}`;

        function padTwoDigit(val: number) {
          return val.toString().padStart(2, "0");
        }
      }
    }
  }

  #updateText(newText: string) {
    this.#fileText = newText;
    Deno.writeTextFileSync(this.#filePath, this.#fileText);
  }
}
