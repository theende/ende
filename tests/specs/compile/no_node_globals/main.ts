// Copyright 2026 the Ende authors. MIT license.
const g = globalThis as Record<string, unknown>;
console.log(
  [
    typeof g.process,
    typeof g.Buffer,
    typeof g.global,
    typeof g.setImmediate,
    typeof g.clearImmediate,
  ].join(" "),
);
const id = setTimeout(() => console.log("not cancelled"), 0);
clearTimeout(id);
console.log(typeof id);
