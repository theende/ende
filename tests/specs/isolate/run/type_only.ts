import { greet, type Parser } from "./vendor/none/greet.ts";
import type { Parser as AlsoParser } from "./vendor/none/mod.ts";

const parser: Parser | AlsoParser | undefined = undefined;
console.log(await greet("types"), parser);
