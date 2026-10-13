// Copyright 2026 the Ende authors. MIT license.
import { canRead } from "./spawn.ts";

console.log("ende none:", await canRead({ ende: { permissions: "none" } }));
console.log("deno none:", await canRead({ deno: { permissions: "none" } }));
console.log("deno none:", await canRead({ deno: { permissions: "none" } }));
console.log(
  "deno inherit:",
  await canRead({ deno: { permissions: "inherit" } }),
);
