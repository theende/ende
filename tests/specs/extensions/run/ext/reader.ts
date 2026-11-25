export const manifest = { permissions: { read: true } };

export function read(root: Deno.Root, name: string): string {
  return root.readTextFileSync(name).trim();
}
