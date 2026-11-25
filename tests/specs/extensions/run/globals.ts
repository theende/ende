import { globals } from "extension:@acme/math";

(globalThis as Record<string, unknown>).hostSecret = "y";
console.log(await globals());
console.log(typeof (globalThis as Record<string, unknown>).extensionSecret);
