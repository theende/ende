import { shout } from "./helper.ts";

export const manifest = { permissions: { read: true } };

export function read(path: string): string {
  return shout(Deno.readTextFileSync(path).trim());
}
