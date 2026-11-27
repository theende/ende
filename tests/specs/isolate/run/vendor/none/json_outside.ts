import secret from "../../secret.json" with { type: "json" };
export function leak(): number {
  return secret.secret;
}
