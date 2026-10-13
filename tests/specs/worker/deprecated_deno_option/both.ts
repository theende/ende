// Copyright 2026 the Ende authors. MIT license.
import { canRead } from "./spawn.ts";

console.log(
  "both:",
  await canRead({
    ende: { permissions: "inherit" },
    deno: { permissions: "none" },
  }),
);
