import { relink } from "./sandboxed/mod.ts";
import { load } from "./show.ts";

for (const line of await relink()) console.log(line);
console.log("linked/x.ts:", await load("./linked/x.ts"));
console.log("store/y.ts:", await load("./store/y.ts"));
