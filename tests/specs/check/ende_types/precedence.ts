// The last comment before an import decides, whichever pragma it is. Every
// import below should get types/boolean.d.ts.
// @deno-types="./types/number.d.ts"
// @ende-types="./types/boolean.d.ts"
import { value as a } from "./impl/a.js";
// @ende-types="./types/number.d.ts"
// @deno-types="./types/boolean.d.ts"
import { value as b } from "./impl/b.js";
// @ts-types="./types/number.d.ts"
// @ende-types="./types/boolean.d.ts"
import { value as c } from "./impl/c.js";
// @ende-types="./types/number.d.ts"
// @ts-types="./types/boolean.d.ts"
import { value as d } from "./impl/d.js";

const values: string[] = [a, b, c, d];
console.log(values);
