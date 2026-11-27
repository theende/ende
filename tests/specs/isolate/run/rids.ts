import { guessRids } from "./vendor/none/mod.ts";

// The app's open file is reachable by its rid in the app's own isolate...
using _file = Ende.openSync("secret.txt");
// deno-lint-ignore no-explicit-any
const core = (Ende as any)[(Ende as any).internal].core;
const found: string[] = [];
for (let rid = 3; rid < 64; rid++) {
  try {
    const buffer = new Uint8Array(64);
    const read = core.readSync(rid, buffer);
    if (read) found.push(new TextDecoder().decode(buffer.subarray(0, read)));
  } catch {
    // not a resource
  }
}
console.log("app", JSON.stringify(found));
// ...but not from the sandbox, which has a resource table of its own.
console.log("sandbox", JSON.stringify(await guessRids()));
