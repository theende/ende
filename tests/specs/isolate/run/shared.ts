import { fromA } from "./vendor/pkg/a.ts";
import { fromB } from "./vendor/pkg/b.ts";

// Two entries of one sandbox share its worker, and so its modules.
console.log(await fromA(), await fromB(), await fromA());
