// The X-TypeScript-Types header says `foo(text: number)`; @deno-types wins.
// @deno-types="http://localhost:4545/run/type_headers_deno_types.foo.d.ts"
import { foo } from "http://localhost:4545/run/type_headers_deno_types.foo.js";
import { foo as bar } from "./header.ts";

foo(1);
bar("a");
