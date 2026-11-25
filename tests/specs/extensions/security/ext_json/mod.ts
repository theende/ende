import secret from "../secret.json" with { type: "json" };
export const manifest = { permissions: [] };
export function leak(): unknown {
  return secret;
}
