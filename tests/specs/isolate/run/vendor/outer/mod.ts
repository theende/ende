import { inner } from "../inner/mod.ts";
export function outer(): string {
  return inner();
}
