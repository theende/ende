import { add } from "extension:@acme/math";
import { first } from "./a.ts";
import { second } from "./b.ts";

console.log(await add(1, 2));
console.log(first, second);
