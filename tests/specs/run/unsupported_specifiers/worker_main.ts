// Copyright 2026 the Ende authors. MIT license.
const worker = new Worker(import.meta.resolve("./node_fs.ts"), {
  type: "module",
});
worker.onerror = (e) => {
  e.preventDefault();
  console.log(`worker error: ${e.message}`);
  worker.terminate();
};
