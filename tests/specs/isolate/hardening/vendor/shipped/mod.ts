// Ships `data`, a symlink out of its folder, and is granted read of its
// own folder only.
export function read(path: string): string {
  return Ende.readTextFileSync(new URL(path, import.meta.url)).trim();
}
