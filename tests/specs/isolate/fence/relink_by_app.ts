import { writeThroughLink } from "./sandboxed/mod.ts";
import { load } from "./show.ts";

// The app itself re-points its symlinked grant, twice: what the sandbox
// wrote through the link stays fenced wherever the link pointed.
function relink(target: string) {
  try {
    Ende.removeSync("link");
  } catch {
    // not there yet
  }
  Ende.symlinkSync(target, "link");
}
relink("t1");
console.log(await writeThroughLink());
relink("t2");
await new Promise((resolve) => setTimeout(resolve, 100));
console.log("t1/x.ts:", await load("./t1/x.ts"));
