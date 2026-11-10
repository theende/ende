#!/usr/bin/env -S deno run --allow-all --config=tests/config/deno.json
// Copyright 2018-2026 the Deno authors. MIT license.
import { join, ROOT_PATH } from "./util.js";

const subcommand = Deno.args.includes("--check") ? "check" : "fmt";
const configFile = join(ROOT_PATH, ".dprint.json");
const cmd = new Deno.Command(await fetchDprint(), {
  args: [subcommand, "--config=" + configFile],
  cwd: ROOT_PATH,
  stdout: "inherit",
  stderr: "inherit",
});

const { code } = await cmd.output();
Deno.exit(code);

/** Downloads the dprint pinned in tools/dprint_version.lock into
 * target/dprint/<version>/, or checks the copy already there, and returns the
 * binary's path. Exits if tools/release/fetch_dprint.sh fails. */
async function fetchDprint() {
  const lock = await Deno.readTextFile(
    join(ROOT_PATH, "tools", "dprint_version.lock"),
  );
  const version = lock.match(/^#\s*version:\s*(\S+)/m)?.[1];
  if (!version) {
    throw new Error("tools/dprint_version.lock has no '# version:'");
  }
  await Deno.mkdir(join(ROOT_PATH, "target", "dprint"), { recursive: true });
  // Relative paths with forward slashes, which bash on Windows also reads.
  const { code, stdout } = await new Deno.Command("bash", {
    args: [
      "tools/release/fetch_dprint.sh",
      Deno.build.target,
      `target/dprint/${version}`,
    ],
    cwd: ROOT_PATH,
    stdout: "piped",
    stderr: "inherit",
  }).output();
  if (code !== 0) {
    // fetch_dprint.sh has said why on stderr.
    Deno.exit(code);
  }
  const dir = new TextDecoder().decode(stdout).trim();
  return join(dir, Deno.build.os === "windows" ? "dprint.exe" : "dprint");
}
