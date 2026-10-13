// Copyright 2018-2026 the Deno authors. MIT license.
console.log("should not panic");
await new Promise((r) => {
  const id = setTimeout(r, 1000);
  Ende.unrefTimer(id);
});
console.log("didn't panic!");
