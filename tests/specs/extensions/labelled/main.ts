import {
  chdir,
  exit,
  ping,
  readStdin,
  signal,
  storage,
  write,
} from "extension:@acme/probe";

async function show(label: string, call: () => unknown) {
  try {
    console.log(`${label}: ${JSON.stringify(await call())}`);
  } catch (e) {
    console.log(`${label}: ${(e as Error).name}: ${(e as Error).message}`);
  }
}

// An extension's thread is held to the same limits as a sandbox's.
for (
  const path of [
    "ende.jsonc",
    "deno.json",
    "plugins/ende.j\u017Fon",
    "plugins/tool.ts",
    "plugins/data.json",
  ]
) {
  await show(path, () => write(path));
}
// The app never loads code from where an extension may write.
await show(
  "load plugins/tool.ts",
  async () => (await import("./plugins/tool.ts")).evil,
);
await show("chdir", () => chdir());
await show("stdin", () => readStdin());
await show("storage", () => storage());
await show("signal", () => signal());
await show("exit", () => exit());
await show("after", () => ping());
