// Code the module graph can't see: a text import and an import of a
// computed path. Both must be covered by the extension's pin.
import payload from "./payload.txt" with { type: "text" };

export const manifest = { permissions: { read: true } };

export async function run(): Promise<string> {
  const url = new URL(["plug", "in.ts"].join(""), import.meta.url);
  const plugin = await import(url.href);
  return `${payload.trim()} ${plugin.name}`;
}
