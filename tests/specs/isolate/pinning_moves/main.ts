import { both } from "./vendor/m/mod.ts";
import { both as other } from "./vendor/other/mod.ts";

console.log("m:", await both(), "other:", await other());
