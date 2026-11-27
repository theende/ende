import { hello } from "./vendor/then/mod.ts";
import * as all from "./vendor/then/mod.ts";

console.log(await hello(), Object.keys(all).join(","));
