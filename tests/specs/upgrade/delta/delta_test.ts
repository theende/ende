// Integration test for delta (bsdiff) upgrades.
// A delta patch is only tried straight from the installed version
// (`ende-<target>.from-<installed>.bsdiff`), whatever the target version.

const currentVersion = Deno.version.ende;

async function runUpgrade(
  args: string[],
): Promise<{ stdout: string; stderr: string; code: number }> {
  const command = new Deno.Command(Deno.execPath(), {
    args: ["upgrade", ...args],
    env: {
      ...Deno.env.toObject(),
      ENDE_TESTING_UPGRADE: "1",
    },
    stdout: "piped",
    stderr: "piped",
  });

  const output = await command.output();
  return {
    stdout: new TextDecoder().decode(output.stdout),
    stderr: new TextDecoder().decode(output.stderr),
    code: output.code,
  };
}

// Test 1: any other version tries a delta from the installed version
console.log("=== Test 1: Another version triggers delta path ===");
{
  const result = await runUpgrade([
    "--version",
    "9999.12.31",
    "--dry-run",
  ]);
  const combined = result.stdout + result.stderr;
  if (!combined.includes("Attempting delta upgrade")) {
    console.log("FAIL: delta was not attempted");
    console.log(combined);
    Deno.exit(1);
  }
  console.log("PASS: delta attempted");
}

// Test 2: reinstalling the installed version skips delta (no patch exists
// from a version to itself)
console.log("\n=== Test 2: Same version skips delta ===");
{
  const result = await runUpgrade([
    "--version",
    currentVersion,
    "--force",
    "--dry-run",
  ]);
  const combined = result.stdout + result.stderr;
  if (combined.includes("Attempting delta upgrade")) {
    console.log("FAIL: delta was attempted for the installed version");
    Deno.exit(1);
  }
  console.log("PASS: delta skipped for the installed version");
}

// Test 3: --no-delta flag disables delta upgrades
console.log("\n=== Test 3: --no-delta flag disables delta ===");
{
  const result = await runUpgrade([
    "--version",
    "9999.12.31",
    "--dry-run",
    "--no-delta",
  ]);
  const combined = result.stdout + result.stderr;
  if (combined.includes("Attempting delta upgrade")) {
    console.log("FAIL: delta was attempted despite --no-delta");
    Deno.exit(1);
  }
  console.log("PASS: --no-delta disables delta path");
}
