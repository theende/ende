import { add } from "add";
import { greeting } from "lib/greet.ts";
import * as remote from "remote";

const sum: string = add(1, 2);
const length: number = greeting;
const letter: number = remote.a;
console.log(sum, length, letter);
