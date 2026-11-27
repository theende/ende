import { read } from "./vendor/none/mod.ts";
import { show } from "./show.ts";

// The broker allows everything; the app may read, the sandbox still not.
await show("app", () => Ende.readTextFileSync("secret.txt").trim());
await show("sandbox", () => read("secret.txt"));
