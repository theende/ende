const worker = new Worker(import.meta.resolve("./worker_inner.ts"), {
  type: "module",
});
worker.onerror = (e) => {
  e.preventDefault();
  console.log(e.message);
  worker.terminate();
};
