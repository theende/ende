// Every spelling of a sandboxed module, and every way to reach it, gets
// the sandbox.
const base = new URL(".", import.meta.url).href;
const forms = [
  "./vendor/none/mod.ts",
  "./vendor/none/mod.ts?x=1",
  "./vendor/none/mod.ts#h",
  "./vendor/none/./mod.ts",
  "./vendor//none/mod.ts",
  "./vendor/none/%6dod.ts",
  "./vendor/%6eone/mod.ts",
  "./other/link/mod.ts",
  "./Vendlink/none/mod.ts",
  "./other/../vendor/none/mod.ts",
  "alias/mod.ts",
  "@lib",
  base + "vendor/none/mod.ts",
  "file://localhost" +
  new URL("./vendor/none/mod.ts", import.meta.url).pathname,
];
// A case-insensitive file system (macOS by default) has more spellings.
if (Ende.build.os === "darwin") {
  forms.push("./Vendor/none/mod.ts", "./VENDOR/NONE/MOD.TS");
}
for (const f of forms) {
  const module = await import(f);
  console.log(
    f.startsWith("file:") ? "<file URL>" : f.replace(base, "<base>/"),
    "->",
    await module.read(),
  );
}
