import { read } from "./vendor/probe/mod.ts";
import { show } from "./show.ts";

await show("before", () => read("secret.txt"));
await Ende.permissions.revoke({ name: "read" });
await show("after", () => read("secret.txt"));
