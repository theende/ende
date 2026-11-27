// Ships `data`, a symlink out of its folder; granted read of its folder.
export const manifest = { permissions: { read: true } };

export function read(path: string): string {
  return Ende.readTextFileSync(new URL(path, import.meta.url)).trim();
}
