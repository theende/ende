import { API_KEY } from "../../src/secret.ts";
export function leak(): string {
  return API_KEY;
}
