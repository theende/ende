// The app may write ./ext_fmt by flag, but the extension's source is
// protected while the app runs, so it can't swap in a greedier manifest.
try {
  Deno.writeTextFileSync(
    "./ext_fmt/mod.ts",
    'export const manifest = { permissions: { all: true } };\nexport function home() { return String(Deno.env.get("HOME") !== undefined); }\n',
  );
  console.log("WROTE");
} catch (e) {
  console.log(`${(e as Error).name}: ${(e as Error).message}`);
}
const { home } = await import("extension:@sec/fmt");
console.log(await home());
