import { readData } from "./vendor/m/mod.ts";

console.log(await readData());
const name = "./lib/" + "late.ts";
const { go } = await import(name);
console.log(await go());
