// Copyright 2026 the Ende authors. MIT license.
try {
  new Worker(import.meta.resolve("./main.ts"), {
    // deno-lint-ignore no-explicit-any
    type: "node" as any,
  });
} catch (e) {
  console.log(`${(e as Error).name}: ${(e as Error).message}`);
}
