// vendor/linked/root is a symlink to the project folder.
export async function load(path: string): Promise<string> {
  try {
    return JSON.stringify(await import(path));
  } catch (e) {
    return `${(e as Error).name}: ${(e as Error).message.split("\n")[0]}`;
  }
}
