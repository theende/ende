import { write } from "./vendor/probe/mod.ts";
import { show } from "./show.ts";

// vendor/probe/link.ts points at this file: the denial follows it.
await show("through a link", () => write("vendor/probe/link.ts"));
console.log(Ende.readTextFileSync("write_link.ts").startsWith("import"));
