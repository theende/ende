// Runs after the extension was prepared (all static imports load before any
// module evaluates) and before its worker starts.
const output = new Deno.Command(Deno.execPath(), {
  args: [
    "eval",
    "--no-config",
    'Deno.writeTextFileSync("ext_fmt2/mod.ts", "export const manifest = { permissions: [] };\\nexport function which() { return \\"REWRITTEN\\"; }\\n")',
  ],
}).outputSync();
if (!output.success) {
  console.log(new TextDecoder().decode(output.stderr));
}
