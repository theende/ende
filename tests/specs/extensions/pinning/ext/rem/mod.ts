import { answer } from "https://localhost:5545/run/extension_lockfile/remote_ext.ts";

export const manifest = { permissions: [] };

export function get(): number {
  return answer();
}
