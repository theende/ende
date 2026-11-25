export const manifest = { permissions: [] };

// A worker the extension starts gets at most the extension's permissions.
export function escalate(): Promise<string> {
  return new Promise((resolve) => {
    try {
      const worker = new Worker(
        "data:application/javascript,self.postMessage(Deno.readTextFileSync('data/a.txt'))",
        { type: "module", ende: { permissions: { read: true } } },
      );
      worker.onerror = (e) => {
        e.preventDefault();
        resolve(`worker error: ${e.message}`);
      };
      worker.onmessage = (e) => resolve(`read: ${e.data}`);
    } catch (e) {
      resolve(`${(e as Error).name}: ${(e as Error).message}`);
    }
  });
}

// A dynamic import is checked against the extension's own permissions.
export async function dynamicImport(url: string): Promise<string> {
  try {
    await import(url);
    return "imported";
  } catch (e) {
    return `${(e as Error).name}: ${(e as Error).message}`;
  }
}
