// Copyright 2018-2026 the Deno authors. MIT license.

// dax 0.43.2 logs through @david/console-static-text, which writes with the
// Node `process` global from 0.3.1 on, and Ende has no `process`. 0.3.0
// writes with `Deno.stderr`; importing it first makes dax's `^0.3.0` resolve
// to it, with or without a lockfile.
import "jsr:@david/console-static-text@0.3.0";

export { $ } from "jsr:@david/dax@0.43.2";
export { createGitHubClient, getGitHubRepository } from "./github.ts";
export { ReleasesMdFile, Repo } from "./repo.ts";
