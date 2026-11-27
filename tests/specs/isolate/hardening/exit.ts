import { exit, ping } from "./vendor/none/mod.ts";
import { show } from "./show.ts";

await show("ping", () => ping());
await show("exit", () => exit());
await show("after", () => ping());
console.log("the app goes on");
