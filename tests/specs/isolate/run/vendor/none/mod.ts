// A sandbox granted nothing.
export function read(path: string): string {
  return Ende.readTextFileSync(path);
}

export function readEnv(): string | undefined {
  return Ende.env.get("HOME");
}

export default function greet(name: string): string {
  return `hello ${name}`;
}

export const VERSION = "1.2.3";
export const LIMITS = { rows: 10, columns: ["a", "b"] };

// None of these can leave the sandbox.
export class Parser {}
export let counter = 0;
export const made = (() => class Made {})();
export const withGetter = {
  get value() {
    return 1;
  },
};

// A worker the sandbox starts is held to the sandbox's permissions.
export function readInWorker(path: string): Promise<string> {
  return new Promise((resolve) => {
    const worker = new Worker(
      `data:application/javascript,try { postMessage(Ende.readTextFileSync(${
        JSON.stringify(path)
      })) } catch (e) { postMessage(e.name + ": " + e.message) }`,
      { type: "module" },
    );
    worker.onmessage = (event) => {
      worker.terminate();
      resolve(event.data);
    };
  });
}

// Guesses the app's resource ids: the sandbox has a table of its own.
export function guessRids(): string[] {
  // deno-lint-ignore no-explicit-any
  const core = (Ende as any)[(Ende as any).internal].core;
  const found: string[] = [];
  // 0-2 are the sandbox's own stdio
  for (let rid = 3; rid < 64; rid++) {
    try {
      const buffer = new Uint8Array(64);
      const read = core.readSync(rid, buffer);
      if (read) found.push(new TextDecoder().decode(buffer.subarray(0, read)));
    } catch {
      // not a resource of this sandbox
    }
  }
  return found;
}

// Raw ops: what's reachable is still checked against the sandbox.
export function rawOps(path: string): string[] {
  // deno-lint-ignore no-explicit-any
  const ende = Ende as any;
  const internal = ende[ende.internal];
  const gated = Object.keys(internal.core.ops).filter((name) =>
    /^op_(fs|net|env|run|spawn|ffi|os|read|write|open|http|fetch)/.test(name)
  );
  const results = [`gated ops reachable: ${gated.length}`];
  for (const symbol of Object.getOwnPropertySymbols(Ende)) {
    const ops = ende[symbol]?.core?.ops ?? {};
    const op = ops.op_fs_read_file_text_sync;
    if (typeof op !== "function") continue;
    try {
      results.push(`read: ${op(path)}`);
    } catch (e) {
      results.push(`${(e as Error).name}: ${(e as Error).message}`);
    }
  }
  try {
    results.push(`read: ${internal.core.ops.op_fs_read_file_text_sync(path)}`);
  } catch (e) {
    results.push((e as Error).name);
  }
  return results;
}

export function pollute(): string {
  // deno-lint-ignore no-explicit-any
  (Object.prototype as any).polluted = "by the sandbox";
  Array.prototype.push = () => {
    throw new Error("hijacked by the sandbox");
  };
  return "polluted";
}

export function seesPollution(): string {
  // deno-lint-ignore no-explicit-any
  return ({} as any).fromApp ?? "clean";
}

export function fail(kind: string): never {
  switch (kind) {
    case "NotFound":
      throw new Ende.errors.NotFound("no such thing");
    case "RangeError":
      throw new RangeError("too far");
    case "AbortError":
      throw new DOMException("stopped", "AbortError");
    default:
      throw new (class CustomError extends Error {
        override name = "CustomError";
      })("custom");
  }
}

// deno-lint-ignore no-explicit-any
export function describeRoot(root: any): string {
  return `${root instanceof Ende.Root} ${typeof root.readTextFile}`;
}

// deno-lint-ignore no-explicit-any
export function readThroughRoot(root: any): Promise<string> {
  return root.readTextFile("a.txt");
}
