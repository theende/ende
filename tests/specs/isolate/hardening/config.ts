import { write } from "./vendor/writer/mod.ts";
import { show } from "./show.ts";

// A config or lockfile name, in any folder and any spelling a file system
// may treat as the same, can't be written by a sandbox even where it may
// write: Ende would find it (`nested/ende.json` for `ende run nested/x.ts`).
for (
  const path of [
    "out/ende.json",
    "out/ENDE.JSONC",
    "out/deno.json",
    "out/Deno.Jsonc",
    "out/ende.lock",
    "out/deno.lock",
    "out/ende.jſon",
    "out/ende.locK",
    "out/ende.json.",
    "out/ende.json ",
    "out/ende.json:stream",
    "nested/ende.json",
    "out/data.json",
  ]
) {
  await show(JSON.stringify(path), () => write(path));
}
