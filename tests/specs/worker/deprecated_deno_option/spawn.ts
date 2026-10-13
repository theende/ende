// Copyright 2026 the Ende authors. MIT license.
export async function canRead(options: WorkerOptions): Promise<boolean> {
  const worker = new Worker(import.meta.resolve("./worker.ts"), {
    type: "module",
    ...options,
  });
  const { promise, resolve } = Promise.withResolvers<boolean>();
  worker.onmessage = (e) => resolve(e.data);
  return await promise;
}
