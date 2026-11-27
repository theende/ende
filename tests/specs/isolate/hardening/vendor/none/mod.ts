// A sandbox granted nothing.
export function chdir(): string {
  Ende.chdir(".");
  return "changed";
}

export async function readStdin(): Promise<string> {
  const buffer = new Uint8Array(64);
  const read = await Ende.stdin.read(buffer);
  return read === null
    ? `closed, prompt is ${typeof (globalThis as Record<string, unknown>)
      .prompt}`
    : new TextDecoder().decode(buffer.subarray(0, read));
}

export function storage(): string {
  const scope = globalThis as Record<string, unknown>;
  return `caches: ${typeof scope.caches}, BroadcastChannel: ${typeof scope
    .BroadcastChannel}`;
}

export function signal(): string {
  Ende.addSignalListener("SIGINT", () => {});
  return "listening";
}

export function exit(): void {
  Ende.exit(3);
}

export function ping(): string {
  return "pong";
}

export function startWorker(url: string): Promise<string> {
  return new Promise((resolve) => {
    const worker = new Worker(url, { type: "module" });
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
