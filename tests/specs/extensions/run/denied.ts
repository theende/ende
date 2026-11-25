import { readFile } from "extension:@acme/math";

console.log(Deno.readTextFileSync("data/a.txt").trim());
try {
  await readFile("data/a.txt");
} catch (e) {
  const error = e as Error;
  console.log(`${error.name}: ${error.message}`);
}
