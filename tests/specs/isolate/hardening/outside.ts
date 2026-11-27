import { show } from "./show.ts";

await show(
  "app module",
  async () => (await import("./vendor/none/leak.ts")).leak(),
);
await show(
  "remote module",
  async () => (await import("./vendor/none/remote.ts")).sum(),
);
const { startWorker } = await import("./vendor/none/mod.ts");
await show(
  "worker of an app module",
  () => startWorker(import.meta.resolve("./src/w.ts")),
);
