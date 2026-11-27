// Export names are data in the generated proxy, never code.
import * as names from "./vendor/names/mod.js";

console.log(Object.keys(names).join(" | "));
// deno-lint-ignore no-explicit-any
console.log(await (names as any)["a\"]; console.log('INJECTED'); //"]());
console.log(typeof (await import("./vendor/names/mod.js")));
