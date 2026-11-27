import * as greeting from "./vendor/none/greet.ts";
import { greet } from "./vendor/none/greet.ts";

// Only what can cross (not the class), and the same function a named
// import gets.
console.log(Object.keys(greeting).join(","), greeting.greet === greet);
console.log(await greeting.greet("namespace"));
const { extra } = await import("./vendor/none/extra.ts");
console.log(await extra());
const dynamic = await import("./vendor/none/greet.ts");
console.log(Object.keys(dynamic).join(","), dynamic.greet === greet);
// `made` is a class, which shows only when the sandbox runs.
try {
  await import("./vendor/none/mod.ts");
} catch (e) {
  console.log((e as Error).message);
}
