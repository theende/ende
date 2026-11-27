import { rawOps } from "./vendor/none/mod.ts";

for (const line of await rawOps("secret.txt")) console.log(line);
