import { chdir, readStdin, signal, storage } from "./vendor/none/mod.ts";
import { show } from "./show.ts";

await show("chdir", () => chdir());
await show("stdin", () => readStdin());
const buffer = new Uint8Array(64);
const read = await Ende.stdin.read(buffer);
console.log(
  "app stdin:",
  new TextDecoder().decode(buffer.subarray(0, read ?? 0)).trim(),
);
await show("storage", () => storage());
console.log("app caches:", typeof caches);
await show("signal", () => signal());
