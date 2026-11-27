const worker = new Worker(import.meta.resolve("./app_worker_inner.ts"), {
  type: "module",
});
worker.onerror = (event) => {
  event.preventDefault();
  console.log(event.message);
  worker.terminate();
};
