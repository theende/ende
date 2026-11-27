import { load } from "./vendor/linked/mod.ts";
import { show } from "./show.ts";

console.log(await load("./root/src/secret.ts"));
await show(
  "text",
  async () => (await import("./vendor/linked/text.ts")).text(),
);
