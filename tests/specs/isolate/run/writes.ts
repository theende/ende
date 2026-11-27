import { write } from "./vendor/probe/mod.ts";
import { show } from "./show.ts";

for (
  const path of [
    "ende.json",
    "ende.lock",
    "writes.ts",
    "show.ts",
    "vendor/none/mod.ts",
    "out/result.txt",
    "vendor/probe/scratch.txt",
  ]
) {
  await show(path, () => write(path));
}
console.log(Ende.readTextFileSync("ende.json").includes("isolate"));
