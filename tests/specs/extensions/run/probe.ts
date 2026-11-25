import { dynamicImport, escalate } from "extension:@acme/probe";

console.log(await escalate());
console.log(await dynamicImport("http://localhost:4545/run/002_hello.ts"));
// The op that starts extension workers isn't reachable from user code, and
// the start function only starts extensions the app already imported.
const internals = Deno[Deno.internal];
console.log(typeof internals.core.ops.op_create_extension_worker);
try {
  await internals.extensions.start("@acme/math");
} catch (e) {
  console.log((e as Error).message);
}
