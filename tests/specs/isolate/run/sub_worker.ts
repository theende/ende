import { readInWorker } from "./vendor/none/mod.ts";

console.log(await readInWorker("secret.txt"));
