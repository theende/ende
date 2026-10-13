// Copyright 2018-2026 the Deno authors. MIT license.

import { $, ReleasesMdFile, Repo } from "./deps.ts";

export class EndeWorkspace {
  #repo: Repo;

  static get rootDirPath() {
    const currentDirPath = $.path(import.meta.dirname!);
    return currentDirPath.parentOrThrow().parentOrThrow();
  }

  static async load(): Promise<EndeWorkspace> {
    return new EndeWorkspace(
      await Repo.load({
        name: "deno",
        path: EndeWorkspace.rootDirPath,
      }),
    );
  }

  private constructor(repo: Repo) {
    this.#repo = repo;
  }

  get repo() {
    return this.#repo;
  }

  get crates() {
    return this.#repo.crates;
  }

  /** Gets the CLI dependency crates that should be published. */
  getCliDependencyCrates() {
    return this.getCliCrate()
      .descendantDependenciesInRepo()
      .filter((c) =>
        c.name !== "test_server" && c.name !== "test_macro" &&
        c.name !== "test_util"
      );
  }

  getCliCrate() {
    return this.getCrate("ende");
  }

  getEndeRtCrate() {
    return this.getCrate("endert");
  }

  getEndeLibCrate() {
    return this.getCrate("ende_lib");
  }

  getCrate(name: string) {
    return this.#repo.getCrate(name);
  }

  getReleasesMdFile() {
    return new ReleasesMdFile(
      EndeWorkspace.rootDirPath.join("Releases.md").toString(),
    );
  }

  async runFormatter() {
    await this.#repo.command(
      "deno run --allow-write --allow-read --allow-net --allow-run ./tools/format.js",
    );
  }
}
