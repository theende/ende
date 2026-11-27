import { both } from "./vendor/m/mod.ts";
import { both as link } from "./link/mod.ts";

console.log("m:", await both(), "link:", await link());
