import { read } from "./vendor/none/mod.ts";

const result = read("secret.txt");
console.log(result instanceof Promise);
try {
  console.log((await result).trim());
} catch (e) {
  console.log((e as Error).name);
}
