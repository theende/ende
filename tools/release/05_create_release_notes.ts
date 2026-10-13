#!/usr/bin/env -S deno run -A --lock=tools/deno.lock.json
// Copyright 2018-2026 the Deno authors. MIT license.
import { $ } from "./deps.ts";
import { EndeWorkspace } from "./ende_workspace.ts";

const workspace = await EndeWorkspace.load();

// create a release notes file for the GH release draft
$.path(EndeWorkspace.rootDirPath)
  .join("./target/release/release-notes.md")
  .writeTextSync(workspace.getReleasesMdFile().getLatestReleaseText().fullText);
