// Copyright 2018-2026 the Deno authors. MIT license.

import { $, ReleasesMdFile, Repo } from "./deps.ts";

export class EndeWorkspace {
  #repo: Repo;

  static get rootDirPath() {
    const currentDirPath = $.path(import.meta.dirname!);
    return currentDirPath.parentOrThrow().parentOrThrow();
  }

  static load(): EndeWorkspace {
    return new EndeWorkspace(new Repo(EndeWorkspace.rootDirPath));
  }

  private constructor(repo: Repo) {
    this.#repo = repo;
  }

  get repo() {
    return this.#repo;
  }

  getReleasesMdFile() {
    return new ReleasesMdFile(
      EndeWorkspace.rootDirPath.join("Releases.md").toString(),
    );
  }

  async runFormatter() {
    await this.#repo.command([
      Deno.execPath(),
      "run",
      "--allow-write",
      "--allow-read",
      "--allow-net",
      "--allow-run",
      "./tools/format.js",
    ]);
  }
}
