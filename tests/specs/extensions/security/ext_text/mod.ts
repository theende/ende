import secret from "../secret.txt" with { type: "text" };
export const manifest = { permissions: [] };
export function leak(): string {
  return secret;
}
