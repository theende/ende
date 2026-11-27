import secret from "./root/secret.txt" with { type: "text" };
export function text(): string {
  return secret;
}
