// A sandboxed package's manifest: version 1.0.0 asks for nothing.
export const manifest = { permissions: [] };

export function version(): string {
  return "1.0.0";
}

export function hasHome(): boolean {
  return Ende.env.get("HOME") !== undefined;
}
