import { describeRoot, readThroughRoot } from "./vendor/none/mod.ts";
import { readThroughRoot as readThroughRootWide } from "./vendor/wide/mod.ts";
import { show } from "./show.ts";

const root = await Ende.openRoot("data");
await show("described", () => describeRoot(root));
// The app's own root stays usable after it crossed.
await show("app after the call", () => root.readTextFile("a.txt"));
// A root carries no authority yet: reading through it takes the sandbox's.
await show(
  "no grant",
  async () => readThroughRoot(await Ende.openRoot("data")),
);
await show(
  "read grant",
  async () => readThroughRootWide(await Ende.openRoot("data")),
);
