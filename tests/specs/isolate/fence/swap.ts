import { swapCase } from "./sandboxed/mod.ts";
import { load } from "./show.ts";

// The fence is matched by every spelling and resolution of the grant, as
// it is now, so swapping the folder doesn't get code past it.
for (const line of await swapCase()) console.log(line);
console.log("OUT/x.ts:", await load("./OUT/x.ts"));
