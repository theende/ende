// A sandboxed package's manifest: version 1.1.0 asks for env.
export const manifest = { permissions: ["env"] };

export function version(): string {
  return "1.1.0";
}

export function hasHome(): boolean {
  return Ende.env.get("HOME") !== undefined;
}
