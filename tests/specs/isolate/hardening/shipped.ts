import { read } from "./vendor/shipped/mod.ts";
import { show } from "./show.ts";

await show("own file", () => read("own.txt"));
await show("through the link", () => read("data/secret.txt"));
