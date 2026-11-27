import greet, { LIMITS, VERSION } from "./vendor/none/mod.ts";

console.log(await greet("world"), VERSION, JSON.stringify(LIMITS));
