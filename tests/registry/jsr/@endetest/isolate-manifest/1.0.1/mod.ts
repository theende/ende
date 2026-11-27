// A sandboxed package's manifest: version 1.0.1 asks for nothing.
export const manifest = { permissions: [] };

export function version(): string {
  return "1.0.1";
}

export function hasHome(): boolean {
  return Ende.env.get("HOME") !== undefined;
}
