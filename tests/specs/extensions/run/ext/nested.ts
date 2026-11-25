import { add } from "extension:@acme/math";

export const manifest = { permissions: [] };

export function sum(): Promise<number> {
  return add(1, 2);
}
