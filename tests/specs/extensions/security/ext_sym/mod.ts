import secret from "./link.json" with { type: "json" };
export const manifest = { permissions: [] };
export function leak(): unknown {
  return secret;
}
