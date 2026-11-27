export const manifest = {
  permissions: { write: true },
};

export function write(path: string): string {
  Ende.writeTextFileSync(path, "export const evil = true;");
  return "written";
}

export function chdir(): string {
  Ende.chdir(".");
  return "changed";
}

export async function readStdin(): Promise<string> {
  const buffer = new Uint8Array(64);
  const read = await Ende.stdin.read(buffer);
  return read === null
    ? "closed"
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
