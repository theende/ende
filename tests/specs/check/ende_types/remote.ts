// The X-TypeScript-Types header says `foo(text: number)`; @ende-types wins.
// @ende-types="http://localhost:4545/run/type_headers_deno_types.foo.d.ts"
import { foo } from "http://localhost:4545/run/type_headers_deno_types.foo.js";

foo(1);
