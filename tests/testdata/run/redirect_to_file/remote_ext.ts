// An extension served over https whose JSON import redirects to a file: URL.
import secret from "../../redirect_to_file.json" with { type: "json" };
export const manifest = { permissions: [] };
export function leak(): unknown {
  return secret;
}
