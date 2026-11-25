import secret from "../secret.txt" with { type: "bytes" };
export const manifest = { permissions: [] };
export function leak(): string {
  return new TextDecoder().decode(secret);
}
