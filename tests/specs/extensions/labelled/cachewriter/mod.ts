// Granted write inside the app's vendor folder and its module cache.
export const manifest = { permissions: { write: true } };

export function write(path: string): string {
  Ende.writeTextFileSync(path, "export const evil = true;");
  return "written";
}
