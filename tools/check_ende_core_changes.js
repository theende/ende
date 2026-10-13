// Copyright 2018-2026 the Deno authors. MIT license.

// deno-lint-ignore-file no-console

// Checks if any files changed relative to the base branch belong to
// ende_core-related package directories (or root Cargo files). Used in CI
// to decide whether to run ende_core tests.
//
// Usage: deno run --allow-run=git check_ende_core_changes.js <base_sha>
// Writes "skip_ende_core_test=true" or "skip_ende_core_test=false" to
// $GITHUB_OUTPUT when running in GitHub Actions.

const ENDE_CORE_PACKAGE_DIRS = [
  "libs/core_testing",
  "libs/core",
  "libs/core/examples/snapshot",
  "libs/ecore",
  "libs/ops",
  "libs/ops/compile_test_runner",
  "libs/serde_v8",
];

const baseSha = Deno.args[0];
if (!baseSha) {
  console.error("Usage: check_ende_core_changes.js <base_sha>");
  Deno.exit(1);
}

// Get list of changed files between the base SHA and HEAD
const { code, stdout, stderr } = await Deno.spawnAndWait("git", [
  "diff",
  "--name-only",
  `${baseSha}..HEAD`,
]);

if (code !== 0) {
  console.error("git diff failed:", stderr);
  console.log("Defaulting to running ende_core tests (skip=false)");
  await writeOutput(false);
  Deno.exit(0);
}

const stdoutText = new TextDecoder().decode(stdout);
const changedFiles = stdoutText.trim().split("\n").filter(Boolean);

const endeCoreChanged = changedFiles.some((file) =>
  ENDE_CORE_PACKAGE_DIRS.some((dir) => file.startsWith(dir + "/")) ||
  file === "Cargo.lock" ||
  file === "Cargo.toml"
);

const skip = !endeCoreChanged;
console.log(`Changed files (${changedFiles.length}):`);
for (const f of changedFiles) {
  console.log(`  ${f}`);
}
console.log(`ende_core changed: ${endeCoreChanged}`);
console.log(`skip_ende_core_test: ${skip}`);

await writeOutput(skip);

async function writeOutput(skip) {
  const outputFile = Deno.env.get("GITHUB_OUTPUT");
  if (outputFile) {
    await Deno.writeTextFile(outputFile, `skip_ende_core_test=${skip}\n`, {
      append: true,
    });
  }
}
