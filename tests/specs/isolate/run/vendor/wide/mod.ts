// Granted read of ./data and all env vars; the app may lack them.
export function readData(): string {
  return Ende.readTextFileSync("data/a.txt").trim();
}

export function readEnv(): boolean {
  return Ende.env.get("HOME") !== undefined;
}

// deno-lint-ignore no-explicit-any
export async function readThroughRoot(root: any): Promise<string> {
  return (await root.readTextFile("a.txt")).trim();
}
