import { read, readEnv } from "./vendor/none/mod.ts";
import { show } from "./show.ts";

console.log(Ende.readTextFileSync("secret.txt").trim());
await show("read", () => read("secret.txt"));
await show("env", () => readEnv());
