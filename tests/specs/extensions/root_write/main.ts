import { useRoot } from "extension:@acme/writer";

using root = Deno.openRootSync("output", { write: true });
for (const line of await useRoot(root)) {
  console.log(line);
}
let escaped = false;
for (const name of ["escaped.txt", "output/../escaped.txt"]) {
  try {
    Deno.statSync(name);
    escaped = true;
  } catch {
    // not there
  }
}
console.log(`anything written outside output/: ${escaped}`);
console.log(`host root still usable: ${root.readTextFileSync("b.txt")}`);
