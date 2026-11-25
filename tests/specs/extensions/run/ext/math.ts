export const manifest = { permissions: [] };

let count = 0;

export function add(a: number, b: number): number {
  return a + b;
}

export function counter(): number {
  count++;
  return count;
}

export function boom(): never {
  throw new TypeError("boom");
}

export function readFile(path: string): string {
  return Deno.readTextFileSync(path);
}

export function readThroughRoot(root: Deno.Root, name: string): string {
  return root.readTextFileSync(name).trim();
}

export function globals(): string {
  (globalThis as Record<string, unknown>).extensionSecret = "x";
  return typeof (globalThis as Record<string, unknown>).hostSecret;
}
