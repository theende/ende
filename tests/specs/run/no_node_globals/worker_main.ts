// Copyright 2026 the Ende authors. MIT license.
const worker = new Worker(import.meta.resolve("./worker.ts"), {
  type: "module",
});
worker.onmessage = (e) => {
  console.log(e.data);
  worker.terminate();
};
