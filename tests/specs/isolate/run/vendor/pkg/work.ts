export function inWorker(): Promise<string> {
  return new Promise((resolve) => {
    const worker = new Worker(import.meta.resolve("./job.ts"), {
      type: "module",
    });
    worker.onmessage = (event) => {
      worker.terminate();
      resolve(event.data);
    };
    worker.onerror = (event) => {
      event.preventDefault();
      worker.terminate();
      resolve(`error: ${event.message}`);
    };
  });
}
