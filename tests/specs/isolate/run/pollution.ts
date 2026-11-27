import { pollute, seesPollution } from "./vendor/none/mod.ts";

console.log(await pollute());
// deno-lint-ignore no-explicit-any
console.log(({} as any).polluted ?? "clean", [1].push(2));
// deno-lint-ignore no-explicit-any
(Object.prototype as any).fromApp = "by the app";
console.log(await seesPollution());
