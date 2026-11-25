import { read } from "extension:@acme/reader";
import { readThroughRoot } from "extension:@acme/math";

using root = Deno.openRootSync("data");
console.log(await read(root, "a.txt"));
try {
  await readThroughRoot(root, "a.txt");
} catch (e) {
  const error = e as Error;
  console.log(`${error.name}: ${error.message}`);
}
console.log(root.readTextFileSync("a.txt").trim());
