export const manifest = { permissions: { net: ["example.com"] } };

export function ping(): string {
  return "pong";
}
