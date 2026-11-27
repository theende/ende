// Granted write access to config and code locations by name; it still
// can't write them.
export function write(
  path: string,
  content = "export const evil = true;",
): string {
  Ende.writeTextFileSync(path, content);
  return "written";
}

export function rename(from: string, to: string): string {
  Ende.renameSync(from, to);
  return "renamed";
}

export function remove(path: string): string {
  Ende.removeSync(path, { recursive: true });
  return "removed";
}
