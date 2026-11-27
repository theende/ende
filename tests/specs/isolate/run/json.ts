import { inside } from "./vendor/none/json_inside.ts";
import data from "./vendor/none/data.json" with { type: "json" };
import { show } from "./show.ts";

await show("inside", () => inside());
// The app reads a sandbox's JSON as data.
console.log(data.inside);
await show(
  "outside",
  async () => (await import("./vendor/none/json_outside.ts")).leak(),
);
