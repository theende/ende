// Runs rewrite.ts with a write grant naming the extension's file itself,
// by its real path, so it is more specific than the protection.
const target = `${Ende.realPathSync("ext_fmt")}/mod.ts`;
const output = await new Ende.Command(Ende.execPath(), {
  args: ["run", "--quiet", `--allow-write=${target}`, "rewrite.ts"],
  stdout: "piped",
  stderr: "piped",
}).output();
console.log(new TextDecoder().decode(output.stdout).trim());
