// The same specifier maps to different modules in different scopes.
import { value } from "value";
import "./sub/mod.ts";

const label: string = value;
console.log(label);
