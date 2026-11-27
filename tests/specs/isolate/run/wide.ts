import { readData, readEnv } from "./vendor/wide/mod.ts";
import { show } from "./show.ts";

await show("read data", () => readData());
await show("env", () => readEnv());
